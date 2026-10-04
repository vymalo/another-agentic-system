//! Roles and permissions over real HTTP (ADR 0033): who is who from the bearer token, what each
//! role may read and change, and the answers a person gets: 404 for what they may not read, 403 for
//! what they may read and not change or what their roles lack, and `GET /api/me` for what is
//! theirs to do. The responses are checked against `docs/api/chat-api.yaml`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, App, AppConfig, NewThread, Permission, Policy,
    RoleGrant,
};
use orch_core::{AgentId, AgentTarget, ThreadId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, ArtifactMeta, ArtifactStore, FixedRegistry, NoModel, PortSet, Principal, Role,
    SystemClock, ThreadStore,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const ROOT: &str = "root@example.com";
const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    MemoryArtifacts,
>;

// ---- the contract, for the answers this file checks ------------------------------------------

struct Contract(Value);

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        Contract(serde_norway::from_str(&text).unwrap())
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

    fn component(&self, name: &str, instance: &Value) {
        self.validate(
            &json!({"$ref": format!("#/components/schemas/{name}")}),
            instance,
        );
    }

    fn problem(&self, instance: &Value) {
        let schema =
            self.0["components"]["responses"]["Problem"]["content"]["application/problem+json"]
                ["schema"]
                .clone();
        self.validate(&schema, instance);
    }

    /// Whether the contract documents `status` for `operation`.
    fn documents(&self, path: &str, method: &str, status: u16) -> bool {
        self.0["paths"][path][method]["responses"]
            .get(status.to_string())
            .is_some()
    }
}

// ---- the harness -----------------------------------------------------------------------------

struct Resp {
    status: u16,
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
    app: Arc<App<Stack>>,
    auth: MemoryAuth,
    artifacts: MemoryArtifacts,
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
        ..Principal::of(orch_core::UserId::new(email))
    }
}

impl Harness {
    /// The default policy: `user` and `admin`, and a person without a role is a `user`. Tokens:
    /// `alice` and `bob` are users, `root` an administrator, `stranger` has a role nobody defined.
    async fn start() -> Self {
        Self::with_policy(Policy::default()).await
    }

    async fn with_policy(policy: Policy) -> Self {
        let entry = |id: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: id.to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry("coder"), entry("plain")]);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        auth.allow("root", principal(ROOT, &["admin"]));
        auth.allow("stranger", principal("stranger@example.com", &["wizard"]));
        auth.allow("plain-token", principal("pat@example.com", &[]));
        let artifacts = MemoryArtifacts::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: artifacts.clone(),
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth: auth.clone(),
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    policy,
                    tool_servers: vec![orch_app::ToolServerInfo::new("docs", "Documentation")],
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
            app,
            auth,
            artifacts,
            contract: Contract::load(),
            server,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> Resp {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(body.to_owned());
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get(&self, path: &str, token: &str) -> Resp {
        self.send(reqwest::Method::GET, path, Some(token), None)
            .await
    }

    /// `expected` for `method path`, a problem of the contract, and documented there.
    async fn refused(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&str>,
        expected: u16,
        code: Option<&str>,
    ) {
        let label = format!("{method} {path} as {token}");
        let r = self.send(method.clone(), path, Some(token), body).await;
        assert_eq!(
            r.status,
            expected,
            "{label}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(r.code().as_deref(), code, "{label}");
        self.contract.problem(&r.json());
        // The contract says so for the operation, for its path as the contract writes it.
        let contract_path = path
            .split('/')
            .map(|part| {
                if part.len() == 36 && part.chars().filter(|c| *c == '-').count() == 4 {
                    "{threadId}"
                } else if part.len() == 64 {
                    "{sha256}"
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let contract_path = contract_path.split('?').next().unwrap().to_owned();
        assert!(
            self.contract
                .documents(&contract_path, &method.as_str().to_lowercase(), expected),
            "the contract documents no {expected} for {label} ({contract_path})"
        );
    }

    /// Ends the thread's turn, as a finished agent would: `done`, so that it can be deleted.
    async fn finish(&self, id: ThreadId) {
        let store = &self.app.ports().store;
        let record = store.get_thread(None, id).await.unwrap().unwrap();
        let commit = orch_ports::Commit {
            new_state: orch_core::ThreadState::Done,
            job: None,
            events: vec![],
            outbox: vec![],
            binding: None,
            now: jiff::Timestamp::now(),
            lease: None,
            watches: vec![],
            timers: vec![],
            inbox: None,
            finishes_outbox: None,
            title: None,
            description: None,
            sharing: None,
            skip_unsent_delegates: false,
        };
        store.commit(id, record.version, commit).await.unwrap();
    }

    async fn thread(&self, who: &str, text: &str) -> ThreadId {
        self.app
            .create_thread(
                &principal(who, &[]),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: text.to_owned(),
                },
            )
            .await
            .unwrap()
            .id
    }
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect()
}

// ---- the tests -------------------------------------------------------------------------------

#[tokio::test]
async fn me_says_who_you_are_what_your_roles_are_and_what_they_grant() {
    let h = Harness::start().await;
    for (token, user, roles, admin) in [
        ("alice", ALICE, json!(["user"]), false),
        // The administrator reaches no more threads than a user: `own`, as ADR 0039 says.
        ("root", ROOT, json!(["admin"]), true),
        // A role nobody defined grants nothing; the default role (`user`) is what is left.
        ("stranger", "stranger@example.com", json!(["user"]), false),
        // A credential with no role at all (the proxy header's) is the default role's too.
        ("plain-token", "pat@example.com", json!(["user"]), false),
    ] {
        let r = h.get("/api/me", token).await;
        assert_eq!(r.status, 200, "{token}");
        let me = r.json();
        h.contract.component("Me", &me);
        assert_eq!(me["user"], user);
        assert_eq!(me["roles"], roles, "{token}");
        let scope = |permission: &str| {
            me["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["permission"] == permission)
                .map(|p| p.get("scope").cloned().unwrap_or(Value::Null))
        };
        for permission in ["thread.read", "thread.write", "artifact.read"] {
            assert_eq!(
                scope(permission),
                Some(json!("own")),
                "{token} {permission}"
            );
        }
        assert_eq!(
            scope("agent.invoke"),
            Some(Value::Null),
            "{token}: no scope on an agent permission"
        );
        assert_eq!(scope("admin").is_some(), admin, "{token}");
        assert_eq!(me["agents"], json!({"read": ["*"], "invoke": ["*"]}));
    }
    // The name and the address the token says are passed on, as it wrote them.
    h.auth.allow(
        "named",
        Principal {
            email: Some("Dana@Example.com".to_owned()),
            name: Some("Dana".to_owned()),
            ..principal("dana@example.com", &["user"])
        },
    );
    let me = h.get("/api/me", "named").await.json();
    assert_eq!(me["email"], "Dana@Example.com");
    assert_eq!(me["name"], "Dana");
    // Without a token it is a 401 like everything else.
    let r = h.send(reqwest::Method::GET, "/api/me", None, None).await;
    assert_eq!(r.status, 401);
}

#[tokio::test]
async fn me_lists_the_agents_of_a_role_and_answers_a_person_who_may_do_nothing() {
    // `chat` may use one agent and read no thread at all; `nobody` grants nothing; no default.
    let chat = RoleGrant {
        permissions: BTreeSet::from([Permission::AgentRead, Permission::AgentInvoke]),
        agents: AgentScope::from_patterns(["plain"]),
    };
    let policy = Policy::new(
        [
            (Role::new("chat"), chat),
            (Role::new("nobody"), RoleGrant::none()),
        ]
        .into(),
        None,
    )
    .unwrap();
    let h = Harness::with_policy(policy).await;
    h.auth
        .allow("chatter", principal("chatter@example.com", &["chat"]));
    h.auth
        .allow("nobody", principal("nobody@example.com", &["nobody"]));

    let me = h.get("/api/me", "chatter").await.json();
    h.contract.component("Me", &me);
    assert_eq!(me["roles"], json!(["chat"]));
    assert_eq!(
        me["permissions"],
        json!([{"permission": "agent.read"}, {"permission": "agent.invoke"}])
    );
    assert_eq!(
        me["agents"],
        json!({"read": ["plain"], "invoke": ["plain"]})
    );
    // The agent list is that role's.
    let list = h.get("/api/agents", "chatter").await.json();
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].clone())
            .collect::<Vec<_>>(),
        [json!("plain")]
    );

    // A person whose roles grant nothing is told so, by `/api/me`, and refused by the rest.
    for token in ["nobody", "stranger", "alice"] {
        let r = h.get("/api/me", token).await;
        assert_eq!(r.status, 200, "{token}");
        let me = r.json();
        h.contract.component("Me", &me);
        // A defined role that grants nothing still counts as a role of the person.
        let roles = if token == "nobody" {
            json!(["nobody"])
        } else {
            json!([])
        };
        assert_eq!(me["roles"], roles, "{token}");
        assert_eq!(me["permissions"], json!([]), "{token}");
        assert_eq!(me["agents"], json!({"read": [], "invoke": []}), "{token}");
        for path in [
            "/api/agents",
            "/api/registry",
            "/api/config",
            "/api/tool-servers",
            "/api/threads",
            "/api/threads/0190aaaa-0000-7000-8000-000000000123",
            "/api/nonsense",
        ] {
            let r = h.get(path, token).await;
            // An unknown path is the router's 404 after the identity, as before.
            let expected = if path == "/api/nonsense" { 404 } else { 403 };
            assert_eq!(r.status, expected, "{token} {path}");
            if expected == 403 {
                assert_eq!(r.code().as_deref(), Some("no_access"));
                h.contract.problem(&r.json());
            }
        }
    }
    // Bad credentials are still 401, not 403.
    let r = h.get("/api/me", "nonsense").await;
    assert_eq!(r.status, 401);
}

#[tokio::test]
async fn threads_belong_to_their_owner_and_not_even_an_administrator_reads_another_s() {
    let h = Harness::start().await;
    let alices = h.thread(ALICE, "alice here").await;
    let bobs = h.thread(BOB, "bob here").await;
    let roots = h.thread(ROOT, "root here").await;
    let (alices, bobs, roots) = (alices.to_string(), bobs.to_string(), roots.to_string());

    // The list is one's own, for everyone, administrators included.
    for (token, own) in [("alice", &alices), ("bob", &bobs), ("root", &roots)] {
        let list = h.get("/api/threads", token).await.json();
        assert_eq!(ids(&list), vec![own.clone()], "{token}");
        assert_eq!(
            list[0]["owner"],
            match token {
                "alice" => ALICE,
                "bob" => BOB,
                _ => ROOT,
            }
        );
        h.contract.component("Thread", &list[0]);
    }

    // A user reads their own, and for someone else's it is as if it did not exist.
    let r = h.get(&format!("/api/threads/{alices}"), "alice").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["owner"], ALICE);
    h.contract.component("Thread", &r.json());
    for path in [
        format!("/api/threads/{alices}"),
        format!("/api/threads/{alices}/export"),
        format!("/api/threads/{alices}/branches"),
        // The id of nothing, which answers the same.
        "/api/threads/0190aaaa-0000-7000-8000-000000000123".to_owned(),
    ] {
        h.refused(reqwest::Method::GET, &path, "bob", None, 404, None)
            .await;
    }
    for (method, path, body) in [
        (
            reqwest::Method::POST,
            format!("/api/threads/{alices}/cancel"),
            None,
        ),
        (
            reqwest::Method::PATCH,
            format!("/api/threads/{alices}"),
            Some(r#"{"title":"Mine now"}"#),
        ),
        (
            reqwest::Method::POST,
            format!("/api/threads/{alices}/fork"),
            Some(r#"{"after":1}"#),
        ),
        // the order of the list is the owner's (ADR 0042)
        (
            reqwest::Method::PATCH,
            format!("/api/threads/{alices}/rail"),
            Some(r#"{"pinned":true}"#),
        ),
    ] {
        h.refused(method, &path, "bob", body, 404, None).await;
    }

    // The administrator is no exception (ADR 0039): Alice's and Bob's threads do not exist for
    // them, to read in any way or to change, and nothing is written.
    for path in [
        format!("/api/threads/{alices}"),
        format!("/api/threads/{bobs}"),
        format!("/api/threads/{alices}/export"),
        format!("/api/threads/{alices}/branches"),
    ] {
        h.refused(reqwest::Method::GET, &path, "root", None, 404, None)
            .await;
    }
    for (method, path, body) in [
        (
            reqwest::Method::POST,
            format!("/api/threads/{alices}/cancel"),
            None,
        ),
        (
            reqwest::Method::PATCH,
            format!("/api/threads/{alices}"),
            Some(r#"{"title":"Root's now"}"#),
        ),
        (
            reqwest::Method::PATCH,
            format!("/api/threads/{alices}"),
            Some(r#"{"description":"x"}"#),
        ),
        (
            reqwest::Method::POST,
            format!("/api/threads/{alices}/fork"),
            Some(r#"{"after":1}"#),
        ),
        (
            reqwest::Method::POST,
            format!("/api/threads/{bobs}/fork"),
            Some(r#"{"replace":1,"text":"again"}"#),
        ),
        // the servers attached to a thread are the owner's to set (ADR 0024)
        (
            reqwest::Method::PUT,
            format!("/api/threads/{alices}/tools"),
            Some(r#"{"servers":["docs"]}"#),
        ),
        // so is the place of a thread in the list (ADR 0042)
        (
            reqwest::Method::PATCH,
            format!("/api/threads/{alices}/rail"),
            Some(r#"{"archived":true}"#),
        ),
    ] {
        h.refused(method, &path, "root", body, 404, None).await;
    }
    let after = h
        .get(&format!("/api/threads/{alices}"), "alice")
        .await
        .json();
    assert_eq!(after["title"], "alice here");
    assert_eq!(after["lastSeq"], 1);
    // The administrator's own thread is theirs to change.
    let r = h
        .send(
            reqwest::Method::PATCH,
            &format!("/api/threads/{roots}"),
            Some("root"),
            Some(r#"{"title":"Renamed by root"}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["title"], "Renamed by root");
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("/api/threads/{roots}/fork"),
            Some("root"),
            Some(r#"{"after":1}"#),
        )
        .await;
    assert!(matches!(r.status, 201 | 409 | 422), "{}", r.status);
}

#[tokio::test]
async fn there_is_no_listing_by_owner_for_anyone() {
    // ADR 0039: the list is the caller's own. `owner` was the administrator's way to list another
    // person's threads or everyone's; it is refused, for every role, in every form.
    let h = Harness::start().await;
    let a = h.thread(ALICE, "a").await.to_string();
    let b = h.thread(BOB, "b").await.to_string();
    let r = h.thread(ROOT, "r").await.to_string();
    let r2 = h.thread(ROOT, "r2").await.to_string();

    for (token, own) in [("alice", vec![a.clone()]), ("bob", vec![b.clone()])] {
        let list = h.get("/api/threads", token).await.json();
        assert_eq!(ids(&list), own, "{token}");
    }
    // The administrator's list is their own, newest first, with a cursor.
    let list = h.get("/api/threads", "root").await.json();
    assert_eq!(ids(&list), [r2.clone(), r.clone()]);
    let list = h
        .get(&format!("/api/threads?before={r2}"), "root")
        .await
        .json();
    assert_eq!(ids(&list), vec![r.clone()]);

    // `owner` is a 400 that says why, not a quiet list of one's own: for everyone, naming oneself
    // included, and whatever the value.
    for token in ["alice", "root"] {
        for query in [
            format!("owner={ALICE}"),
            format!("owner={ROOT}"),
            "owner=*".to_owned(),
            "owner=".to_owned(),
            "owner=alice".to_owned(),
            "owner=%20".to_owned(),
        ] {
            h.refused(
                reqwest::Method::GET,
                &format!("/api/threads?{query}"),
                token,
                None,
                400,
                None,
            )
            .await;
        }
    }
    let r = h.get("/api/threads?owner=*", "root").await;
    assert_eq!(r.json()["detail"], "owner is not supported (ADR 0039)");
}

#[tokio::test]
async fn files_are_read_by_artifact_read() {
    let h = Harness::start().await;
    let thread = h.thread(ALICE, "files").await;
    let bytes = Bytes::from_static(b"just text");
    let meta = ArtifactMeta::of("text/plain", Some("note.txt".to_owned()), &bytes);
    h.artifacts
        .put(&meta.key(thread), bytes, &meta)
        .await
        .unwrap();
    let hash = meta.key(thread).sha256_hex();
    let path = format!("/api/threads/{thread}/artifacts/{hash}");
    // The owner reads it; another user and the administrator cannot tell it is there (ADR 0039).
    let r = h.get(&path, "alice").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"just text");
    for token in ["bob", "root"] {
        h.refused(reqwest::Method::GET, &path, token, None, 404, None)
            .await;
    }

    // A role without artifact.read is a 403, whatever it asks for.
    let reader = RoleGrant {
        permissions: BTreeSet::from([Permission::ThreadRead, Permission::AgentRead]),
        ..RoleGrant::user()
    };
    let h = Harness::with_policy(
        Policy::new(
            [(Role::new("user"), reader)].into(),
            Some(Role::new("user")),
        )
        .unwrap(),
    )
    .await;
    let thread = ThreadId(uuid::Uuid::from_u128(0x77));
    let path = format!("/api/threads/{thread}/artifacts/{}", "ab".repeat(32));
    h.refused(
        reqwest::Method::GET,
        &path,
        "alice",
        None,
        403,
        Some("forbidden"),
    )
    .await;
    // ...and one without thread.write may neither list the servers to attach nor attach one
    h.refused(
        reqwest::Method::GET,
        "/api/tool-servers",
        "alice",
        None,
        403,
        Some("forbidden"),
    )
    .await;
    h.refused(
        reqwest::Method::PUT,
        &format!("/api/threads/{thread}/tools"),
        "alice",
        Some(r#"{"servers":["docs"]}"#),
        403,
        Some("forbidden"),
    )
    .await;
}

#[tokio::test]
async fn agents_are_filtered_and_a_role_without_agent_invoke_cannot_start_on_one() {
    // Alice's role names one agent; the thread-level permissions are the user's.
    let chat = RoleGrant {
        agents: AgentScope::from_patterns(["plain"]),
        ..RoleGrant::user()
    };
    let policy = Policy::new(
        [
            (Role::new("chat"), chat),
            (Role::new("user"), RoleGrant::user()),
        ]
        .into(),
        Some(Role::new("user")),
    )
    .unwrap();
    let h = Harness::with_policy(policy).await;
    h.auth
        .allow("chatter", principal("chatter@example.com", &["chat"]));

    let all = h.get("/api/agents", "alice").await.json();
    assert_eq!(all.as_array().unwrap().len(), 2);
    let some = h.get("/api/agents", "chatter").await.json();
    assert_eq!(some.as_array().unwrap().len(), 1);
    assert_eq!(some[0]["id"], "plain");
    // The registry's status is about the list, and open to whoever may read it.
    assert_eq!(h.get("/api/registry", "chatter").await.status, 200);
    // A fork of the chatter's own thread onto the agent they may not invoke is a 403.
    let own = h
        .app
        .create_thread(
            &principal("chatter@example.com", &["chat"]),
            NewThread {
                title: None,
                target: AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
                text: "hi".to_owned(),
            },
        )
        .await
        .unwrap();
    h.refused(
        reqwest::Method::POST,
        &format!("/api/threads/{}/fork", own.id),
        "chatter",
        Some(r#"{"replace":1,"text":"again","target":{"agentId":"coder"}}"#),
        403,
        Some("forbidden"),
    )
    .await;
}

#[tokio::test]
async fn a_credential_that_is_not_a_person_is_refused_before_any_role_counts() {
    let h = Harness::start().await;
    for token in ["", "nobody-has-this"] {
        let r = h
            .send(reqwest::Method::GET, "/api/threads", Some(token), None)
            .await;
        assert_eq!(r.status, 401, "{token:?}");
    }
    h.auth.set_down(true);
    let r = h.get("/api/threads", "alice").await;
    assert_eq!(r.status, 503, "an issuer that cannot be read is not a 403");
    h.auth.set_down(false);
    assert_eq!(h.get("/api/threads", "alice").await.status, 200);
}

#[tokio::test]
async fn arranging_the_list_needs_thread_read_and_not_thread_write() {
    // ADR 0042, decision 10: pin, archive, move and eject change nothing in the conversation and
    // nobody else sees them, so a person who may only read their threads may arrange them, and one
    // who may not read them may not.
    let reader = RoleGrant {
        permissions: BTreeSet::from([Permission::ThreadRead]),
        agents: AgentScope::from_patterns(["*"]),
    };
    let writer = RoleGrant {
        permissions: BTreeSet::from([Permission::ThreadWrite, Permission::AgentInvoke]),
        agents: AgentScope::from_patterns(["*"]),
    };
    let policy = Policy::new(
        [
            (Role::new("reader"), reader),
            (Role::new("writer"), writer),
            (Role::new("user"), RoleGrant::user()),
        ]
        .into(),
        Some(Role::new("user")),
    )
    .unwrap();
    let h = Harness::with_policy(policy).await;
    h.auth
        .allow("reader", principal("reader@example.com", &["reader"]));
    h.auth
        .allow("writer", principal("writer@example.com", &["writer"]));
    let own = h.thread("reader@example.com", "mine").await.to_string();
    let theirs = h.thread("writer@example.com", "theirs").await.to_string();

    // a reader cannot rename, and can pin
    h.refused(
        reqwest::Method::PATCH,
        &format!("/api/threads/{own}"),
        "reader",
        Some(r#"{"title":"x"}"#),
        403,
        Some("forbidden"),
    )
    .await;
    let r = h
        .send(
            reqwest::Method::PATCH,
            &format!("/api/threads/{own}/rail"),
            Some("reader"),
            Some(r#"{"pinned":true}"#),
        )
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["pinned"], json!(true));
    h.contract.component("Thread", &r.json());
    // a person who cannot read the list cannot arrange it, whatever the thread
    h.refused(
        reqwest::Method::PATCH,
        &format!("/api/threads/{theirs}/rail"),
        "writer",
        Some(r#"{"pinned":true}"#),
        403,
        Some("forbidden"),
    )
    .await;
}

#[tokio::test]
async fn deleting_needs_thread_delete_and_not_thread_write_and_no_role_reaches_another_s_thread() {
    // ADR 0043, decision 6: erasing one's own data does not need the right to write; a role that
    // lists its permissions without `thread.delete` cannot, whatever the id; and nobody, an
    // administrator included, deletes another person's thread.
    let reader = RoleGrant {
        permissions: BTreeSet::from([Permission::ThreadRead, Permission::ThreadDelete]),
        agents: AgentScope::from_patterns(["*"]),
    };
    let keeper = RoleGrant {
        permissions: BTreeSet::from([
            Permission::ThreadRead,
            Permission::ThreadWrite,
            Permission::AgentInvoke,
        ]),
        agents: AgentScope::from_patterns(["*"]),
    };
    let policy = Policy::new(
        [
            (Role::new("reader"), reader),
            (Role::new("keeper"), keeper),
            (Role::new("user"), RoleGrant::user()),
            (Role::new("admin"), RoleGrant::admin()),
        ]
        .into(),
        Some(Role::new("user")),
    )
    .unwrap();
    let h = Harness::with_policy(policy).await;
    h.auth
        .allow("reader", principal("reader@example.com", &["reader"]));
    h.auth
        .allow("keeper", principal("keeper@example.com", &["keeper"]));
    let own = h.thread("reader@example.com", "mine").await;
    let kept = h.thread("keeper@example.com", "kept").await;
    let alices = h.thread(ALICE, "alice's").await;
    for id in [own, kept, alices] {
        h.finish(id).await;
    }
    let (own, kept, alices) = (own.to_string(), kept.to_string(), alices.to_string());

    // `GET /api/me` says who may: the built-in roles do, and so does a role that lists it
    let me = h.get("/api/me", "reader").await.json();
    assert!(
        me["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!({"permission": "thread.delete"})),
        "{me}"
    );
    let me = h.get("/api/me", "keeper").await.json();
    assert!(
        !me["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!({"permission": "thread.delete"})),
        "{me}"
    );
    h.contract.component("Me", &me);

    // a role without it is refused for its own thread and for any id alike
    for id in [kept.as_str(), alices.as_str(), RANDOM] {
        h.refused(
            reqwest::Method::DELETE,
            &format!("/api/threads/{id}"),
            "keeper",
            None,
            403,
            Some("forbidden"),
        )
        .await;
    }
    // another person's thread is a 404 for a user, a reader and an administrator
    for token in ["bob", "reader", "root"] {
        h.refused(
            reqwest::Method::DELETE,
            &format!("/api/threads/{alices}"),
            token,
            None,
            404,
            None,
        )
        .await;
    }
    h.refused(
        reqwest::Method::DELETE,
        &format!("/api/threads/{RANDOM}"),
        "alice",
        None,
        404,
        None,
    )
    .await;
    assert!(
        h.get(&format!("/api/threads/{alices}"), "alice")
            .await
            .status
            == 200
    );
    // a person who may only read erases their own
    let r = h
        .send(
            reqwest::Method::DELETE,
            &format!("/api/threads/{own}"),
            Some("reader"),
            None,
        )
        .await;
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    h.refused(
        reqwest::Method::DELETE,
        &format!("/api/threads/{own}"),
        "reader",
        None,
        404,
        None,
    )
    .await;
    // and so does a user, and an administrator their own
    assert_eq!(
        h.send(
            reqwest::Method::DELETE,
            &format!("/api/threads/{alices}"),
            Some("alice"),
            None
        )
        .await
        .status,
        204
    );
}
