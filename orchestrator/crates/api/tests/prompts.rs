//! `/api/prompts` (ADR 0070) over real HTTP against the in-memory stack: every answer is validated against
//! `docs/api/chat-api.yaml`, the vectors of `docs/api/examples/prompts/` (the ones the web's mock server passes
//! too) are asked of it, and the rules that make the library safe are held: only the owner writes, a share needs
//! `prompts.share` and is read by its audience only, the administrator has no bypass, a prompt never names its
//! author. (There is no test that "a prompt is in no export": no route or port joins the library to a thread's log, so a
//! test that created both and looked for one in the other could not fail. What puts a prompt's words in a thread is the
//! person sending the rendered text as an ordinary message, which is then the thread's, by design.)
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, App, AppConfig, Permission, Policy, RoleGrant,
};
use orch_core::{AgentId, UserId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{AgentEndpoint, NoModel, PortSet, Principal, Role, SystemClock};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const CAROL: &str = "carol@example.com";
const ROOT: &str = "root@example.com";

const REFERENCES_PROMPTS: &str = include_str!("../../../../docs/api/examples/prompts/prompts.json");
const RENDER: &str = include_str!("../../../../docs/api/examples/prompts/render.json");
const REQUESTS: &str = include_str!("../../../../docs/api/examples/prompts/requests.json");

// ---- the vectors' templates -----------------------------------------------------------------

/// `{"$concat": [piece, ...]}` is the pieces joined (a piece: a string or `{"$repeat", "times"}`).
fn expand(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(expand).collect()),
        Value::Object(o) => {
            if let Some(Value::Array(pieces)) = o.get("$concat") {
                let mut out = String::new();
                for p in pieces {
                    match p {
                        Value::String(s) => out.push_str(s),
                        other => out.push_str(&repeat(other)),
                    }
                }
                Value::String(out)
            } else if o.contains_key("$repeat") {
                Value::String(repeat(v))
            } else {
                Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x))).collect())
            }
        }
        other => other.clone(),
    }
}

fn repeat(v: &Value) -> String {
    let unit = v["$repeat"].as_str().unwrap();
    unit.repeat(usize::try_from(v["times"].as_u64().unwrap()).unwrap())
}

// ---- the contract ---------------------------------------------------------------------------

struct Contract(Value);

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        Contract(serde_norway::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
    }

    fn validate(&self, schema: &Value, instance: &Value) {
        let mut root = schema.clone();
        let object = root.as_object_mut().unwrap();
        object.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        object.insert("components".to_owned(), self.0["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .unwrap();
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}\n{instance}", errors.join("\n"));
    }

    /// A body of `operation` for `status`: documented, and valid against what it documents.
    fn answer(&self, operation: &str, path: &str, method: &str, status: u16, body: &Value) {
        let responses = &self.0["paths"][path][method.to_lowercase()]["responses"];
        let declared = responses
            .get(status.to_string())
            .unwrap_or_else(|| panic!("the contract documents no {status} for {operation}"));
        let declared = match declared.get("$ref").and_then(Value::as_str) {
            Some(r) => self
                .0
                .pointer(r.strip_prefix('#').unwrap())
                .unwrap()
                .clone(),
            None => declared.clone(),
        };
        if let Some(content) = declared.get("content").and_then(Value::as_object) {
            let (_, media) = content.iter().next().unwrap();
            self.validate(&media["schema"], body);
        }
    }
}

// ---- the harness -----------------------------------------------------------------------------

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    contract: Contract,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

fn grant(permissions: &[Permission]) -> RoleGrant {
    RoleGrant {
        permissions: permissions.iter().copied().collect::<BTreeSet<_>>(),
        agents: AgentScope::from_patterns(["*"]),
        max_tier: None,
    }
}

fn policy() -> Policy {
    let base: Vec<Permission> = Permission::ALL
        .into_iter()
        .filter(|p| *p != Permission::PromptsShare && *p != Permission::Admin)
        .collect();
    let mut with_share = base.clone();
    with_share.push(Permission::PromptsShare);
    Policy::new(
        [
            (Role::new("user"), grant(&base)),
            (Role::new("team"), grant(&base)),
            (Role::new("sharer"), grant(&with_share)),
            (
                Role::new("admin"),
                grant(&[Permission::Admin, Permission::ThreadRead]),
            ),
        ]
        .into_iter()
        .collect(),
        None,
    )
    .unwrap()
}

impl Harness {
    async fn start() -> Self {
        let entry = AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("plain"),
                "https://plain.example.com/.well-known/agent-card.json".to_owned(),
                None,
            ),
            name: "plain".to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry]);
        let auth = MemoryAuth::new();
        // alice shares; `alice-lost` is the same person after the operator took `prompts.share` away
        auth.allow("alice", principal(ALICE, &["sharer", "user"]));
        auth.allow("alice-lost", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        auth.allow("carol", principal(CAROL, &["user", "team"]));
        auth.allow("root", principal(ROOT, &["admin"]));
        auth.allow("nobody", principal("nobody@example.com", &["unknown-role"]));
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: MemoryArtifacts::new(),
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
                    user_tools: orch_ports::NoUserTools::off(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    policy: policy(),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router =
            orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            contract: Contract::load(),
            server,
        }
    }

    async fn raw(
        &self,
        token: &str,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Option<Vec<u8>>,
    ) -> Resp {
        let mut req = self
            .client
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(ct) = content_type {
            req = req.header("Content-Type", ct);
        }
        if let Some(body) = body {
            req = req.body(body);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn call(&self, token: &str, method: &str, path: &str, body: Option<&Value>) -> Resp {
        let ct = body.map(|_| "application/json");
        let bytes = body.map(|b| serde_json::to_vec(b).unwrap());
        self.raw(token, method, path, ct, bytes).await
    }

    /// The operation a request is, for the contract.
    fn operation(method: &str, path: &str) -> (&'static str, &'static str) {
        let path = path.split('?').next().unwrap();
        match (method, path.matches('/').count(), path.ends_with("/render")) {
            ("GET", 2, _) => ("listPrompts", "/api/prompts"),
            ("POST", 2, _) => ("createPrompt", "/api/prompts"),
            ("GET", 3, _) => ("getPrompt", "/api/prompts/{promptId}"),
            ("PATCH", 3, _) => ("updatePrompt", "/api/prompts/{promptId}"),
            ("DELETE", 3, _) => ("deletePrompt", "/api/prompts/{promptId}"),
            ("POST", 4, true) => ("renderPrompt", "/api/prompts/{promptId}/render"),
            other => panic!("not a route of the library: {other:?}"),
        }
    }

    /// A request whose answer is checked against the contract (the status is documented, the body matches),
    /// and which is never cached.
    async fn checked(&self, token: &str, method: &str, path: &str, body: Option<&Value>) -> Resp {
        let r = self.call(token, method, path, body).await;
        self.check(method, path, &r);
        r
    }

    fn check(&self, method: &str, path: &str, r: &Resp) {
        let (operation, template) = Self::operation(method, path);
        assert_eq!(
            r.headers.get("cache-control").and_then(|v| v.to_str().ok()),
            Some("no-store"),
            "{operation} {}",
            r.status
        );
        if r.status == 204 {
            self.contract
                .answer(operation, template, method, r.status, &Value::Null);
        } else {
            self.contract
                .answer(operation, template, method, r.status, &r.json());
        }
    }

    async fn make(&self, token: &str, body: &Value) -> Value {
        let r = self
            .checked(token, "POST", "/api/prompts", Some(body))
            .await;
        assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
        r.json()
    }
}

fn greet(name: &str, audience: &[&str]) -> Value {
    json!({
        "name": name,
        "title": format!("Title {name}"),
        "body": "Hello {{who}}",
        "variables": [{"id": "who", "question": "Who?", "type": "text"}],
        "audience": audience,
    })
}

// ---- the vectors -------------------------------------------------------------------------------

/// Every whole save of `prompts.json`: a person who holds `prompts.share` sends it, and the answer is the
/// status, the `code`, `at` and `offset` the file says, and a problem of the contract.
#[tokio::test]
async fn every_whole_save_of_prompts_json() {
    let h = Harness::start().await;
    let file: Value = serde_json::from_str(REFERENCES_PROMPTS).unwrap();
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 100);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let create = expand(&c["create"]);
        let r = h
            .checked("alice", "POST", "/api/prompts", Some(&create))
            .await;
        let expect = &c["expect"];
        if expect == "ok" {
            assert_eq!(
                r.status,
                201,
                "{name}: {}",
                String::from_utf8_lossy(&r.body)
            );
            let id = r.json()["id"].as_str().unwrap().to_owned();
            let gone = h
                .checked("alice", "DELETE", &format!("/api/prompts/{id}"), None)
                .await;
            assert_eq!(gone.status, 204, "{name}");
            continue;
        }
        assert_eq!(
            u64::from(r.status),
            expect["status"].as_u64().unwrap(),
            "{name}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(r.code().as_deref(), expect["code"].as_str(), "{name}");
        let body = r.json();
        assert_eq!(body["at"].as_str(), expect["at"].as_str(), "{name}");
        assert_eq!(body["offset"].as_u64(), expect["offset"].as_u64(), "{name}");
    }
}

/// Every render of `render.json`.
#[tokio::test]
async fn every_render_of_render_json() {
    let h = Harness::start().await;
    let file: Value = serde_json::from_str(RENDER).unwrap();
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 70);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let mut prompt = expand(&c["prompt"]);
        prompt["name"] = json!("p");
        prompt["title"] = json!("T");
        let made = h.make("alice", &prompt).await;
        let id = made["id"].as_str().unwrap();
        let request = expand(&c["request"]);
        let r = h
            .checked(
                "alice",
                "POST",
                &format!("/api/prompts/{id}/render"),
                Some(&request),
            )
            .await;
        let expect = expand(&c["expect"]);
        if let Some(text) = expect.get("text") {
            assert_eq!(
                r.status,
                200,
                "{name}: {}",
                String::from_utf8_lossy(&r.body)
            );
            assert_eq!(
                r.json(),
                json!({"text": text, "revision": made["revision"]}),
                "{name}"
            );
        } else if expect.get("rule").is_some() {
            assert_eq!(r.status, 422, "{name}");
            let body = r.json();
            assert_eq!(body["code"], "invalid_answer", "{name}");
            assert_eq!(body["question"], expect["question"], "{name}");
            assert_eq!(body["rule"], expect["rule"], "{name}");
        } else {
            assert_eq!(r.status, 422, "{name}");
            assert_eq!(r.code().as_deref(), expect["code"].as_str(), "{name}");
        }
        h.checked("alice", "DELETE", &format!("/api/prompts/{id}"), None)
            .await;
    }
}

/// Every raw request of `requests.json`: the media type, the shape, the permission, the revision and which
/// error wins.
#[tokio::test]
async fn every_request_of_requests_json() {
    let h = Harness::start().await;
    let file: Value = serde_json::from_str(REQUESTS).unwrap();
    let mut ids = Vec::new();
    let mut revisions = Vec::new();
    for seed in file["seed"].as_array().unwrap() {
        let made = h.make("alice", seed).await;
        ids.push(made["id"].as_str().unwrap().to_owned());
        revisions.push(made["revision"].as_str().unwrap().to_owned());
    }
    let fill = |s: &str| {
        let mut out = s.to_owned();
        for (n, id) in ids.iter().enumerate() {
            out = out.replace(&format!("$id{n}"), id);
        }
        for (n, rev) in revisions.iter().enumerate() {
            out = out.replace(&format!("$rev{n}"), rev);
        }
        out
    };
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 60);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let token = if c["as"] == "plain" { "bob" } else { "alice" };
        let method = c["method"].as_str().unwrap();
        let path = fill(c["path"].as_str().unwrap());
        let body = c.get("body").map(|b| match expand(b) {
            Value::String(s) => fill(&s).into_bytes(),
            other => panic!("{name}: a body is text: {other}"),
        });
        let content_type = match c.get("headers") {
            None => body.as_ref().map(|_| "application/json".to_owned()),
            Some(h) => h["content-type"].as_str().map(str::to_owned),
        };
        let r = h
            .raw(token, method, &path, content_type.as_deref(), body)
            .await;
        h.check(method, &path, &r);
        let expect = &c["expect"];
        assert_eq!(
            u64::from(r.status),
            expect["status"].as_u64().unwrap(),
            "{name}: {}",
            String::from_utf8_lossy(&r.body)
        );
        if let Some(code) = expect["code"].as_str() {
            assert_eq!(r.code().as_deref(), Some(code), "{name}");
        }
        if let Some(text) = expect["text"].as_str() {
            assert_eq!(r.json()["text"], text, "{name}");
        }
        if let Some(revision) = expect["revision"].as_str() {
            assert_eq!(r.json()["revision"], fill(revision), "{name}");
        }
    }
}

// ---- the rules ------------------------------------------------------------------------------------

/// A share is read by its audience and nobody else; an administrator has no bypass; the author is never named;
/// the audience cannot write.
#[tokio::test]
async fn a_share_is_read_by_its_audience_only_and_never_names_its_author() {
    let h = Harness::start().await;
    let shared = h.make("alice", &greet("shared", &["team"])).await;
    let id = shared["id"].as_str().unwrap();
    let one = format!("/api/prompts/{id}");
    assert_eq!(shared["audience"], json!(["team"]));
    let mut seen = Vec::new();
    // carol has `team`
    let r = h.checked("carol", "GET", &one, None).await;
    assert_eq!(r.status, 200);
    seen.push(r.body.clone());
    let read = r.json();
    assert_eq!(read["own"], false);
    assert!(read.get("audience").is_none());
    let list = h
        .checked("carol", "GET", "/api/prompts?scope=shared", None)
        .await;
    seen.push(list.body.clone());
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 1);
    assert!(list.json()["items"][0].get("audience").is_none());
    let answers = json!({"answers": [{"id": "who", "values": [], "text": "there"}]});
    let r = h
        .checked("carol", "POST", &format!("{one}/render"), Some(&answers))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["text"], "Hello there");
    seen.push(r.body.clone());
    // she cannot write
    let edit = json!({"revision": shared["revision"], "title": "Mine"});
    for resp in [
        h.checked("carol", "PATCH", &one, Some(&edit)).await,
        h.checked("carol", "DELETE", &one, None).await,
    ] {
        assert_eq!(resp.status, 404);
        seen.push(resp.body.clone());
    }
    // no body she was given holds the author
    for body in &seen {
        let text = String::from_utf8_lossy(body);
        assert!(!text.contains(ALICE), "the author is in {text}");
        assert!(!text.contains("alice"), "the author is in {text}");
    }
    // bob has no role of it, and the administrator has no bypass: the same 404 as for an id that never existed
    for token in ["bob", "root"] {
        for (method, path, body) in [
            ("GET", one.clone(), None),
            ("POST", format!("{one}/render"), Some(answers.clone())),
        ] {
            let r = h.checked(token, method, &path, body.as_ref()).await;
            assert_eq!(r.status, 404, "{token} {method}");
        }
        let list = h.checked(token, "GET", "/api/prompts", None).await;
        assert_eq!(list.json()["items"], json!([]), "{token}");
    }
    // the author withdraws it: gone at once
    let unshare = json!({"revision": shared["revision"], "audience": []});
    assert_eq!(
        h.checked("alice", "PATCH", &one, Some(&unshare))
            .await
            .status,
        200
    );
    assert_eq!(h.checked("carol", "GET", &one, None).await.status, 404);
}

/// `prompts.share` gates a share and nothing else: without it a private prompt is saved, a shared one is 403
/// before the values are read, and a person who lost it can unshare and delete but not change what stays shared.
#[tokio::test]
async fn prompts_share_gates_sharing_and_a_share_of_a_person_who_lost_it_is_frozen() {
    let h = Harness::start().await;
    let r = h
        .checked(
            "bob",
            "POST",
            "/api/prompts",
            Some(&json!({"name": "A", "title": "T", "body": "x", "audience": ["*"]})),
        )
        .await;
    assert_eq!((r.status, r.code().as_deref()), (403, Some("forbidden")));
    let private = h.make("bob", &greet("mine", &[])).await;
    let path = format!("/api/prompts/{}", private["id"].as_str().unwrap());
    let share = json!({"revision": private["revision"], "audience": ["team"]});
    let r = h.checked("bob", "PATCH", &path, Some(&share)).await;
    assert_eq!((r.status, r.code().as_deref()), (403, Some("forbidden")));
    // alice shares, then the permission is taken away (the same person, other roles)
    let shared = h.make("alice", &greet("shared", &["team"])).await;
    let path = format!("/api/prompts/{}", shared["id"].as_str().unwrap());
    let change = json!({"revision": shared["revision"], "title": "New"});
    let r = h.checked("alice-lost", "PATCH", &path, Some(&change)).await;
    assert_eq!((r.status, r.code().as_deref()), (403, Some("forbidden")));
    let withdraw = json!({"revision": shared["revision"], "audience": []});
    let r = h
        .checked("alice-lost", "PATCH", &path, Some(&withdraw))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["audience"], json!([]));
    let r = h.checked("alice-lost", "DELETE", &path, None).await;
    assert_eq!(r.status, 204);
}

/// A person whose roles grant nothing is refused every route with `no_access`.
#[tokio::test]
async fn a_person_whose_roles_grant_nothing_is_refused_with_no_access() {
    let h = Harness::start().await;
    let r = h.call("nobody", "GET", "/api/prompts", None).await;
    assert_eq!((r.status, r.code().as_deref()), (403, Some("no_access")));
    let r = h
        .call("nobody", "POST", "/api/prompts", Some(&greet("x", &[])))
        .await;
    assert_eq!((r.status, r.code().as_deref()), (403, Some("no_access")));
}

/// Every answer under `/api/prompts` is `Cache-Control: no-store`, the ones that no handler of the library gives
/// included: the identity layer's 401, the guard's `no_access`, a path that is no route (404), a verb that is not
/// one (405) and a media type that is not JSON (415). The layer is outside the timeout and the guard by its place in
/// `router_with_surfaces`, so the timeout's own 503 has it too (a 503 cannot be made here: the in-memory store never
/// waits). What is not the library's is not touched by the layer.
#[tokio::test]
async fn every_answer_under_api_prompts_is_never_cached() {
    let h = Harness::start().await;
    let no_store = |r: &Resp, what: &str| {
        assert_eq!(
            r.headers.get("cache-control").and_then(|v| v.to_str().ok()),
            Some("no-store"),
            "{what}: {}",
            r.status
        );
    };
    let cases: [(&str, &str, &str, u16); 7] = [
        ("a token nobody has", "GET", "/api/prompts", 401),
        ("nobody", "GET", "/api/prompts", 403),
        ("alice", "PUT", "/api/prompts", 405),
        ("alice", "DELETE", "/api/prompts", 405),
        ("alice", "GET", "/api/prompts/p-aaaaaaaaaaaa/nothing", 404),
        ("alice", "PUT", "/api/prompts/p-aaaaaaaaaaaa", 405),
        ("alice", "GET", "/api/prompts/p-aaaaaaaaaaaa", 404),
    ];
    for (token, method, path, status) in cases {
        let r = h.raw(token, method, path, None, None).await;
        assert_eq!(r.status, status, "{method} {path} as {token}");
        no_store(&r, &format!("{method} {path} as {token}"));
    }
    let r = h
        .raw(
            "alice",
            "POST",
            "/api/prompts",
            Some("text/plain"),
            Some(b"{}".to_vec()),
        )
        .await;
    assert_eq!(r.status, 415);
    no_store(&r, "a media type");
    // the layer is for the library: another path is not given the header by it
    let r = h.raw("alice", "GET", "/api/agents", None, None).await;
    assert_eq!(r.status, 200);
    assert!(
        r.headers
            .get("cache-control")
            .is_none_or(|v| v != "no-store"),
        "the layer is for /api/prompts alone"
    );
}

/// A page is 100: with exactly 100 visible there is no `nextCursor`, the 101st gives one that is the last id of the
/// page, and the next page holds the rest and no cursor.
#[tokio::test]
async fn a_page_has_a_cursor_only_when_more_follow() {
    let h = Harness::start().await;
    for n in 0..100 {
        let r = h
            .call(
                "bob",
                "POST",
                "/api/prompts",
                Some(&greet(&format!("p{n}"), &[])),
            )
            .await;
        assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    }
    let full = h.checked("bob", "GET", "/api/prompts", None).await.json();
    assert_eq!(full["items"].as_array().unwrap().len(), 100);
    assert_eq!(full["limit"], 100);
    assert!(full.get("nextCursor").is_none(), "exactly 100 is one page");
    // one more is visible to bob: a prompt of alice's for everybody
    h.make("alice", &greet("everybody", &["*"])).await;
    let first = h.checked("bob", "GET", "/api/prompts", None).await.json();
    let items = first["items"].as_array().unwrap();
    assert_eq!(items.len(), 100);
    let cursor = first["nextCursor"].as_str().expect("a cursor");
    assert_eq!(cursor, items[99]["id"].as_str().unwrap());
    let second = h
        .checked("bob", "GET", &format!("/api/prompts?cursor={cursor}"), None)
        .await
        .json();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second.get("nextCursor").is_none());
    // the listing says how many bob holds, whatever the page
    assert_eq!(second["held"], 100);
}
