//! Sharing a thread by a revocable link over real HTTP (ADR 0040): the owner's routes, the signed-in
//! and the public reads, the one 404 for every link that does not work, what a reader is told and
//! not told, the headers, the rate limit, and the token kept out of the request span.
//!
//! Every answer that has a shape is checked against `docs/api/chat-api.yaml`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use orch_api::{ApiConfig, PublicLimits};
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, NewThread, PublicView, ShareKeys, SharingMode,
    SharingSettings,
};
use orch_core::{
    Actor, AgentId, AgentStepData, AgentTarget, ArtifactData, EventBody, FileRef, StepKind,
    StepOutput, StepPhase, StepState, ThreadId, UserId,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, Arrangement, ArtifactKey, ArtifactMeta, ArtifactStore, Commit, FixedRegistry,
    NewEvent, NoModel, PortSet, Principal, Role, SystemClock, ThreadStore,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const ROOT: &str = "root@example.com";
const SECRET: &str = "0123456789abcdef0123456789abcdef";

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

    /// Whether the contract documents `status` for `method path`.
    fn documents(&self, path: &str, method: &str, status: u16) -> bool {
        self.0["paths"][path][method]["responses"]
            .get(status.to_string())
            .is_some()
    }
}

// ---- the harness ----------------------------------------------------------------------------

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    store: MemoryStore,
    artifacts: MemoryArtifacts,
    contract: Contract,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn settings(mode: SharingMode, public: PublicView) -> SharingSettings {
    SharingSettings::new(
        mode,
        Some(ShareKeys::new(SecretString::from(SECRET.to_owned()), None).unwrap()),
        public,
    )
    .unwrap()
}

impl Harness {
    async fn start(mode: SharingMode) -> Self {
        Self::configured(
            settings(mode, PublicView::default()),
            Some(PublicLimits::default()),
        )
        .await
    }

    async fn configured(sharing: SharingSettings, limits: Option<PublicLimits>) -> Self {
        Self::configured_over(MemoryStore::new(), sharing, limits).await
    }

    /// A process over a store that already exists: a restart.
    async fn configured_over(
        store: MemoryStore,
        sharing: SharingSettings,
        limits: Option<PublicLimits>,
    ) -> Self {
        let entry = |id: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: id.to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry("plain")]);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        auth.allow("root", principal(ROOT, &["admin"]));
        auth.allow("stranger", principal("stranger@example.com", &["wizard"]));
        let artifacts = MemoryArtifacts::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: artifacts.clone(),
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    sharing,
                    // no role is defined by the deployment, so a person with an unknown role is a user
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = ApiConfig {
            public_limits: limits,
            ..ApiConfig::default()
        };
        let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            store,
            artifacts,
            contract: Contract::load(),
            server,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> Resp {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = bearer {
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
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> Resp {
        self.send(reqwest::Method::GET, path, bearer, None).await
    }

    async fn put_share(&self, id: &str, bearer: &str, visibility: &str) -> Resp {
        self.send(
            reqwest::Method::PUT,
            &format!("/api/threads/{id}/share"),
            Some(bearer),
            Some(&format!(r#"{{"visibility":"{visibility}"}}"#)),
        )
        .await
    }

    async fn thread(&self, owner: &str, text: &str) -> String {
        self.app
            .create_thread(
                &UserId::new(owner),
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
            .to_string()
    }

    /// Appends events to a thread straight in the store.
    async fn append(&self, id: &str, events: Vec<(Actor, EventBody)>) {
        let id: ThreadId = id.parse().unwrap();
        let record = self.store.get_thread(None, id).await.unwrap().unwrap();
        self.store
            .commit(
                id,
                record.version,
                Commit {
                    new_state: record.state,
                    job: None,
                    events: events
                        .into_iter()
                        .map(|(actor, body)| NewEvent {
                            at: jiff::Timestamp::now(),
                            actor,
                            body,
                            idempotency_key: None,
                        })
                        .collect(),
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
                },
            )
            .await
            .unwrap();
    }

    /// Shares as Alice and returns the token of the link.
    async fn share(&self, id: &str, visibility: &str) -> String {
        let r = self.put_share(id, "alice", visibility).await;
        assert_eq!(r.status, 200, "{}", r.text());
        let url = r.json()["url"].as_str().unwrap().to_owned();
        url.strip_prefix("/s/").unwrap().to_owned()
    }
}

fn step() -> AgentStepData {
    let mut input = serde_json::Map::new();
    input.insert("command".into(), json!("cat notes.txt"));
    AgentStepData {
        id: "T/1".into(),
        path: vec![],
        kind: StepKind::Tool,
        label: "Read the notes".into(),
        state: StepState::Completed,
        phase: StepPhase::End,
        icon: None,
        detail: Some("ok".into()),
        input: Some(input),
        output: Some(StepOutput {
            text: "the notes".into(),
            truncated: false,
            bytes: None,
            error: false,
        }),
        io_dropped: false,
    }
}

fn agent() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
}

// ---- the owner's routes -----------------------------------------------------------------------

#[tokio::test]
async fn the_owner_shares_widens_rotates_and_stops_and_the_answers_match_the_contract() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let path = format!("/api/threads/{id}/share");

    // a private thread says nothing of sharing
    let r = h.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert_eq!(r.status, 200);
    assert!(r.json().get("share").is_none());

    // share: 200, the link, never cached
    let r = h.put_share(&id, "alice", "internal").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("cache-control"), "no-store");
    h.contract.component("ThreadShareLink", &r.json());
    let shared = r.json();
    assert_eq!(shared["visibility"], "internal");
    assert_eq!(shared["effective"], "internal");
    let url = shared["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/s/") && url.len() == 3 + 43, "{url}");
    assert!(shared["sharedAt"].as_str().unwrap().ends_with('Z'));

    // idempotent: the same level is the same link
    let again = h.put_share(&id, "alice", "internal").await;
    assert_eq!(again.json()["url"], url);

    // the owner's thread and list say so; only the thread has the link
    let r = h.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert_eq!(r.header("cache-control"), "no-store");
    h.contract.component("Thread", &r.json());
    assert_eq!(
        r.json()["share"],
        json!({"visibility": "internal", "effective": "internal", "url": url})
    );
    let r = h.get("/api/threads", Some("alice")).await;
    let listed = r.json();
    let item = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id.as_str())
        .unwrap();
    h.contract.component("Thread", item);
    assert_eq!(
        item["share"],
        json!({"visibility": "internal", "effective": "internal"}),
        "no link in a list"
    );

    // widen: same link
    let r = h.put_share(&id, "alice", "public").await;
    assert_eq!(r.json()["visibility"], "public");
    assert_eq!(r.json()["url"], url);

    // a new link kills the old one
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("{path}/rotate"),
            Some("alice"),
            None,
        )
        .await;
    assert_eq!(r.status, 200);
    h.contract.component("ThreadShareLink", &r.json());
    assert_ne!(r.json()["url"], url);
    assert_eq!(r.json()["visibility"], "public", "at the level it had");
    let old = url.strip_prefix("/s/").unwrap();
    let r = h.get(&format!("/api/public/shared/{old}"), None).await;
    assert_eq!(r.status, 404);

    // stop: 204, again is 204, and the thread says nothing of it
    let r = h
        .send(reqwest::Method::DELETE, &path, Some("alice"), None)
        .await;
    assert_eq!(r.status, 204);
    assert!(r.body.is_empty());
    let r = h
        .send(reqwest::Method::DELETE, &path, Some("alice"), None)
        .await;
    assert_eq!(r.status, 204);
    let r = h.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert!(r.json().get("share").is_none());
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("{path}/rotate"),
            Some("alice"),
            None,
        )
        .await;
    assert_eq!((r.status, r.code().as_deref()), (409, Some("not_shared")));
}

#[tokio::test]
async fn what_the_owners_routes_refuse_and_say() {
    let h = Harness::start(SharingMode::Internal).await;
    let id = h.thread(ALICE, "hello").await;
    let path = format!("/api/threads/{id}/share");
    let refused = |r: &Resp, status: u16, code: Option<&str>| {
        assert_eq!(r.status, status, "{}", r.text());
        assert_eq!(r.code().as_deref(), code, "{}", r.text());
        h.contract.problem(&r.json());
    };

    // above the cap
    let r = h.put_share(&id, "alice", "public").await;
    refused(&r, 409, Some("over_cap"));
    assert!(
        h.contract
            .documents("/api/threads/{threadId}/share", "put", 409)
    );
    // bodies that cannot be used, `private` among them: stop sharing is DELETE
    for body in [
        None,
        Some("{}"),
        Some(r#"{"visibility":"private"}"#),
        Some(r#"{"visibility":"world"}"#),
        Some(r#"{"visibility":3}"#),
        Some(r#"{"visibility":"internal","extra":1}"#),
        Some("[1]"),
        Some("not json"),
    ] {
        let r = h
            .send(reqwest::Method::PUT, &path, Some("alice"), body)
            .await;
        refused(&r, 400, None);
    }
    // somebody else's thread, an administrator included, and one that is not there
    for (token, target) in [
        ("bob", id.as_str()),
        ("root", id.as_str()),
        ("alice", "0190aaaa-0000-7000-8000-000000000123"),
        ("alice", "not-a-uuid"),
    ] {
        let p = format!("/api/threads/{target}/share");
        let r = h
            .send(
                reqwest::Method::PUT,
                &p,
                Some(token),
                Some(r#"{"visibility":"internal"}"#),
            )
            .await;
        refused(&r, 404, None);
        let r = h
            .send(
                reqwest::Method::POST,
                &format!("{p}/rotate"),
                Some(token),
                None,
            )
            .await;
        refused(&r, 404, None);
        let r = h.send(reqwest::Method::DELETE, &p, Some(token), None).await;
        refused(&r, 404, None);
    }
    // no identity
    let r = h
        .send(
            reqwest::Method::PUT,
            &path,
            None,
            Some(r#"{"visibility":"internal"}"#),
        )
        .await;
    assert_eq!(r.status, 401);
    // nothing of the refusals shared anything
    let r = h.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert!(r.json().get("share").is_none());

    // under a disabled cap: nothing can be shared or given a new link...
    let off = Harness::start(SharingMode::Disabled).await;
    let id = off.thread(ALICE, "hello").await;
    let path = format!("/api/threads/{id}/share");
    let r = off.put_share(&id, "alice", "internal").await;
    refused(&r, 403, Some("sharing_disabled"));
    let r = off
        .send(
            reqwest::Method::POST,
            &format!("{path}/rotate"),
            Some("alice"),
            None,
        )
        .await;
    refused(&r, 403, Some("sharing_disabled"));
    // ...and taking a link down is never refused for it, nor is a person without `thread.share`
    let r = off
        .send(reqwest::Method::DELETE, &path, Some("alice"), None)
        .await;
    assert_eq!(r.status, 204);
    assert!(
        off.contract
            .documents("/api/threads/{threadId}/share", "delete", 204)
    );
}

#[tokio::test]
async fn me_says_what_the_person_may_share_as() {
    for (mode, want) in [
        (SharingMode::Disabled, "disabled"),
        (SharingMode::Internal, "internal"),
        (SharingMode::Public, "public"),
    ] {
        let h = Harness::start(mode).await;
        let r = h.get("/api/me", Some("alice")).await;
        assert_eq!(r.status, 200);
        h.contract.component("Me", &r.json());
        assert_eq!(r.json()["sharing"], want);
        // `thread.share` is listed beside the others, and takes no scope
        assert!(
            r.json()["permissions"]
                .as_array()
                .unwrap()
                .contains(&json!({"permission": "thread.share"})),
            "{}",
            r.text()
        );
    }
}

// ---- reading ----------------------------------------------------------------------------------

#[tokio::test]
async fn a_signed_in_reader_reads_and_the_public_cannot_read_an_internal_link() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    h.append(&id, vec![(agent(), EventBody::AgentStep(step()))])
        .await;
    let token = h.share(&id, "internal").await;
    let signed_in = format!("/api/shared/{token}");
    let public = format!("/api/public/shared/{token}");

    // identity required on the signed-in route
    let r = h.get(&signed_in, None).await;
    assert_eq!(r.status, 401);
    // read by another user, an administrator, and the owner, who is told so
    for (who, owner) in [("bob", false), ("root", false), ("alice", true)] {
        let r = h.get(&signed_in, Some(who)).await;
        assert_eq!(r.status, 200, "{who}: {}", r.text());
        h.contract.component("SharedThread", &r.json());
        let body = r.json();
        assert_eq!(body["id"], id.as_str());
        assert_eq!(body["title"], "hello");
        assert_eq!(body["visibility"], "internal");
        assert_eq!(body["isOwner"], owner);
        // never the owner's address, a parent, or tools
        assert!(!r.text().contains(ALICE), "{}", r.text());
        for hidden in ["owner", "forkedFrom", "tools", "share"] {
            assert!(body.get(hidden).is_none(), "{hidden}: {body}");
        }
        assert_eq!(r.header("cache-control"), "no-store");
        assert_eq!(r.header("x-robots-tag"), "noindex, nofollow");
    }
    // a role nobody defined falls to the default role, and reads like a user
    assert_eq!(h.get(&signed_in, Some("stranger")).await.status, 200);
    // the public route does not serve an internal link, whoever asks
    for bearer in [None, Some("bob"), Some("garbage")] {
        let r = h.get(&public, bearer).await;
        assert_eq!(r.status, 404, "{bearer:?}");
        assert_eq!(r.header("cache-control"), "no-store");
        assert_eq!(r.header("x-robots-tag"), "noindex, nofollow");
    }
}

#[tokio::test]
async fn a_public_link_reads_without_an_identity_and_ignores_one() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    let public = format!("/api/public/shared/{token}");

    let anonymous = h.get(&public, None).await;
    assert_eq!(anonymous.status, 200, "{}", anonymous.text());
    h.contract.component("SharedThread", &anonymous.json());
    assert_eq!(anonymous.json()["visibility"], "public");
    assert_eq!(anonymous.json()["isOwner"], false);
    assert!(!anonymous.text().contains(ALICE));
    assert_eq!(anonymous.header("cache-control"), "no-store");
    assert_eq!(anonymous.header("x-robots-tag"), "noindex, nofollow");
    assert!(anonymous.headers.get("set-cookie").is_none());

    // an Authorization header is ignored: a bad one is not a 401, a good one is not an identity
    for bearer in ["garbage", "alice", "bob"] {
        let r = h.get(&public, Some(bearer)).await;
        assert_eq!(r.status, 200, "{bearer}");
        assert_eq!(r.json(), anonymous.json(), "{bearer} reads as nobody");
    }
    // a signed-in reader may read a public link by the signed-in route too
    let r = h.get(&format!("/api/shared/{token}"), Some("bob")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["visibility"], "public");
}

#[tokio::test]
async fn every_link_that_does_not_work_is_the_same_404_with_the_same_body() {
    let h = Harness::start(SharingMode::Public).await;
    let shared = h.thread(ALICE, "hello").await;
    let token = h.share(&shared, "public").await;
    let other = h.thread(ALICE, "other").await;
    let other_token = h.share(&other, "public").await;
    let private = h.thread(ALICE, "private").await;
    // revoked
    let revoked = h.thread(ALICE, "revoked").await;
    let revoked_token = h.share(&revoked, "public").await;
    h.send(
        reqwest::Method::DELETE,
        &format!("/api/threads/{revoked}/share"),
        Some("alice"),
        None,
    )
    .await;
    // the MAC of another thread's link on this nonce
    let swapped = {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut a = engine.decode(&token).unwrap();
        let b = engine.decode(&other_token).unwrap();
        a[16..].copy_from_slice(&b[16..]);
        engine.encode(a)
    };
    let tampered = format!(
        "{}{}",
        &token[..42],
        if token.ends_with('A') { "B" } else { "A" }
    );

    let causes = [
        ("a token nobody has", "A".repeat(43)),
        ("no token", "x".to_owned()),
        ("a thread id", private.clone()),
        ("a revoked link", revoked_token),
        ("another thread's mac", swapped),
        ("a tampered mac", tampered),
        ("a padded token", format!("{token}=")),
    ];
    let mut bodies = Vec::new();
    for (what, bad) in &causes {
        for (path, bearer) in [
            (format!("/api/public/shared/{bad}"), None),
            (format!("/api/shared/{bad}"), Some("bob")),
        ] {
            let r = h.get(&path, bearer).await;
            assert_eq!(r.status, 404, "{what}: {path}");
            h.contract.problem(&r.json());
            assert_eq!(r.header("cache-control"), "no-store", "{what}");
            bodies.push((path.contains("/public/"), r.body));
        }
    }
    for public in [true, false] {
        let mut same = bodies.iter().filter(|(p, _)| *p == public).map(|(_, b)| b);
        let first = same.next().unwrap();
        assert!(same.all(|b| b == first), "one body for every cause");
    }
    // a private thread has no link to a reader at all; and the good ones read
    assert_eq!(
        h.get(&format!("/api/public/shared/{token}"), None)
            .await
            .status,
        200
    );
    // a file that is not the thread's, by the same answer
    for path in [
        format!("/api/shared/{token}/artifacts/{}", "ab".repeat(32)),
        format!("/api/public/shared/{token}/artifacts/{}", "ab".repeat(32)),
    ] {
        let r = h.get(&path, Some("bob")).await;
        assert_eq!(r.status, 404, "{path}");
    }
}

#[tokio::test]
async fn the_public_projection_hides_steps_and_files_unless_the_deployment_says_so() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    // the thread view is the same for both: what differs is the log (AG-UI, tested with it); here the
    // files: a file is the thread's, kept, and named by its log
    let bytes = Bytes::from_static(b"hello, file");
    let meta = ArtifactMeta::of("text/plain", Some("n.txt".to_owned()), &bytes);
    let key = ArtifactKey::new(id.parse().unwrap(), meta.sha256);
    h.artifacts.put(&key, bytes, &meta).await.unwrap();
    let sha = key.sha256_hex();
    h.append(
        &id,
        vec![(
            agent(),
            EventBody::Artifact(ArtifactData {
                name: "n.txt".into(),
                mime_type: Some("text/plain".into()),
                uri: None,
                text: None,
                file: Some(FileRef {
                    sha256: sha.clone(),
                    size: meta.size,
                    filename: Some("n.txt".into()),
                }),
            }),
        )],
    )
    .await;

    // a signed-in reader opens it, as a file is sent, and it is never cached
    let r = h
        .get(&format!("/api/shared/{token}/artifacts/{sha}"), Some("bob"))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.body, b"hello, file");
    assert_eq!(r.header("cache-control"), "no-store");
    assert_eq!(r.header("x-content-type-options"), "nosniff");
    assert!(r.header("content-security-policy").contains("sandbox"));
    assert!(r.header("content-disposition").starts_with("inline"));
    let r = h
        .get(&format!("/api/shared/{token}/artifacts/{sha}"), None)
        .await;
    assert_eq!(r.status, 401);
    // the owner's own route still has its immutable cache
    let r = h
        .get(&format!("/api/threads/{id}/artifacts/{sha}"), Some("alice"))
        .await;
    assert!(r.header("cache-control").contains("immutable"));

    // the public has no files by default
    let r = h
        .get(&format!("/api/public/shared/{token}/artifacts/{sha}"), None)
        .await;
    assert_eq!(r.status, 404);
    // and has them when the deployment says so
    let generous = Harness::configured(
        settings(
            SharingMode::Public,
            PublicView {
                step_io: false,
                files: true,
            },
        ),
        Some(PublicLimits::default()),
    )
    .await;
    let id = generous.thread(ALICE, "hello").await;
    let token = generous.share(&id, "public").await;
    let bytes = Bytes::from_static(b"hello, file");
    let meta = ArtifactMeta::of("text/plain", Some("n.txt".to_owned()), &bytes);
    let key = ArtifactKey::new(id.parse().unwrap(), meta.sha256);
    generous.artifacts.put(&key, bytes, &meta).await.unwrap();
    generous
        .append(
            &id,
            vec![(
                agent(),
                EventBody::Artifact(ArtifactData {
                    name: "n.txt".into(),
                    mime_type: Some("text/plain".into()),
                    uri: None,
                    text: None,
                    file: Some(FileRef {
                        sha256: key.sha256_hex(),
                        size: meta.size,
                        filename: None,
                    }),
                }),
            )],
        )
        .await;
    let r = generous
        .get(
            &format!("/api/public/shared/{token}/artifacts/{}", key.sha256_hex()),
            None,
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.body, b"hello, file");
    assert_eq!(r.header("cache-control"), "no-store");
    assert_eq!(r.header("x-robots-tag"), "noindex, nofollow");
}

#[tokio::test]
async fn a_cap_below_the_share_narrows_the_link_and_raising_it_brings_it_back() {
    // shared as public; the deployment restarts as internal, then disabled, then public again
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    let public = format!("/api/public/shared/{token}");
    let signed_in = format!("/api/shared/{token}");
    assert_eq!(h.get(&public, None).await.status, 200);

    let internal = Harness::configured_over(
        h.store.clone(),
        settings(SharingMode::Internal, PublicView::default()),
        Some(PublicLimits::default()),
    )
    .await;
    // the public route says 404 (the one body), the signed-in one reads, as internal
    let r = internal.get(&public, None).await;
    assert_eq!(r.status, 404);
    internal.contract.problem(&r.json());
    let r = internal.get(&signed_in, Some("bob")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["visibility"], "internal");
    // the owner is told what the thread is stored as and what is served
    let r = internal
        .get(&format!("/api/threads/{id}"), Some("alice"))
        .await;
    assert_eq!(
        r.json()["share"]["visibility"],
        "public",
        "nothing was rewritten"
    );
    assert_eq!(r.json()["share"]["effective"], "internal");

    // disabled: nothing is served, and the owner is told the link is paused (no secret, no url)
    let off = Harness::configured_over(
        h.store.clone(),
        SharingSettings::default(),
        Some(PublicLimits::default()),
    )
    .await;
    assert_eq!(off.get(&signed_in, Some("bob")).await.status, 404);
    assert_eq!(off.get(&public, None).await.status, 404);
    let r = off.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert_eq!(
        r.json()["share"],
        json!({"visibility": "public", "effective": "private"})
    );
    // ...and the owner can still take it down
    // (not here: the next restart brings it back first)

    let again = Harness::configured_over(
        h.store.clone(),
        settings(SharingMode::Public, PublicView::default()),
        Some(PublicLimits::default()),
    )
    .await;
    assert_eq!(again.get(&public, None).await.status, 200, "the same link");
    let r = again
        .send(
            reqwest::Method::DELETE,
            &format!("/api/threads/{id}/share"),
            Some("alice"),
            None,
        )
        .await;
    assert_eq!(r.status, 204);
    assert_eq!(again.get(&public, None).await.status, 404);
}

// ---- the metrics --------------------------------------------------------------------------------

#[tokio::test]
async fn the_counters_say_what_happened_and_not_who() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    h.get(&format!("/api/public/shared/{token}"), None).await;
    h.get(&format!("/api/shared/{token}"), Some("bob")).await;
    h.put_share(&id, "alice", "internal").await;
    let r = h.get("/metrics", None).await;
    assert_eq!(r.status, 200);
    let text = r.text();
    assert!(
        text.contains("share_changes_total{action=\"share\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("share_changes_total{action=\"narrow\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("shared_reads_total{visibility=\"public\"} 2"),
        "{text}"
    );
    // nothing per person, per address or per link
    for hidden in [ALICE, BOB, "127.0.0.1", token.as_str(), id.as_str()] {
        assert!(!text.contains(hidden), "{hidden} is in /metrics");
    }
}

// ---- the rate limit ------------------------------------------------------------------------------

#[tokio::test]
async fn the_public_routes_are_rate_limited_and_failures_count_against_all_of_them() {
    let limits = PublicLimits {
        per_link_per_second: 3,
        total_per_second: 6,
        streams_per_link: 5,
        streams_total: 50,
    };
    let h = Harness::configured(
        settings(SharingMode::Public, PublicView::default()),
        Some(limits),
    )
    .await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    let public = format!("/api/public/shared/{token}");

    // one link: its own bucket
    let mut statuses = Vec::new();
    for _ in 0..4 {
        statuses.push(h.get(&public, None).await.status);
    }
    assert_eq!(statuses, [200, 200, 200, 429]);
    let r = h.get(&public, None).await;
    assert_eq!(r.status, 429);
    h.contract.problem(&r.json());
    assert!(r.header("retry-after").parse::<u64>().unwrap() >= 1);
    assert_eq!(r.header("cache-control"), "no-store");
    assert_eq!(r.header("x-robots-tag"), "noindex, nofollow");
    assert!(
        h.contract
            .documents("/api/public/shared/{token}", "get", 429)
    );

    // the signed-in routes are not subject to it
    for _ in 0..10 {
        assert_eq!(
            h.get(&format!("/api/shared/{token}"), Some("bob"))
                .await
                .status,
            200
        );
    }
}

#[tokio::test]
async fn guessing_tokens_is_throttled_by_the_failures() {
    let limits = PublicLimits {
        per_link_per_second: 100,
        total_per_second: 20,
        streams_per_link: 5,
        streams_total: 50,
    };
    let h = Harness::configured(
        settings(SharingMode::Public, PublicView::default()),
        Some(limits),
    )
    .await;
    let mut seen = Vec::new();
    for n in 0..30 {
        let guess = format!("{n:0>43}");
        seen.push(
            h.get(&format!("/api/public/shared/{guess}"), None)
                .await
                .status,
        );
    }
    let not_found = seen.iter().filter(|s| **s == 404).count();
    assert!(
        (3..=6).contains(&not_found) && seen.contains(&429),
        "20 tokens, 5 for a failure: {seen:?}"
    );
}

#[tokio::test]
async fn without_a_limiter_the_public_routes_answer_the_one_404_and_public_is_refused() {
    let h = Harness::configured(settings(SharingMode::Public, PublicView::default()), None).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    let r = h.get(&format!("/api/public/shared/{token}"), None).await;
    assert_eq!(r.status, 404, "fails closed");
    // the composition is refused before it serves
    let cfg = ApiConfig {
        public_limits: None,
        ..ApiConfig::default()
    };
    assert_eq!(
        cfg.check(&h.app),
        Err(orch_api::ApiConfigError::PublicWithoutLimiter)
    );
    assert_eq!(ApiConfig::default().check(&h.app), Ok(()));
    // a deployment that is not public needs none
    let internal = Harness::start(SharingMode::Internal).await;
    assert_eq!(cfg.check(&internal.app), Ok(()));
}

/// What the owner did to their list (ADR 0042) is on the thread row for the owner's routes and is
/// nowhere in what a reader is given, whichever link and whoever reads: the projection is an
/// allow-list. Archiving is tidying: the link keeps working, and the owner's view keeps its share.
#[tokio::test]
async fn the_owners_list_is_not_in_what_a_reader_is_given_and_archiving_keeps_the_link() {
    let h = Harness::start(SharingMode::Public).await;
    let id = h.thread(ALICE, "hello").await;
    let token = h.share(&id, "public").await;
    let paths = [
        (format!("/api/shared/{token}"), Some("bob")),
        (format!("/api/shared/{token}"), Some("alice")),
        (format!("/api/public/shared/{token}"), None),
    ];
    let mut plain = Vec::new();
    for (path, bearer) in &paths {
        let r = h.get(path, *bearer).await;
        assert_eq!(r.status, 200);
        plain.push(r.json());
    }

    let thread: ThreadId = id.parse().unwrap();
    for change in [
        Arrangement {
            pinned: Some(true),
            ..Arrangement::default()
        },
        Arrangement {
            archived: Some(true),
            ..Arrangement::default()
        },
    ] {
        h.app
            .arrange_thread(&UserId::new(ALICE), thread, change)
            .await
            .unwrap();
    }
    for ((path, bearer), before) in paths.iter().zip(&plain) {
        let r = h.get(path, *bearer).await;
        assert_eq!(r.status, 200, "an archived thread's link works: {path}");
        let body = r.json();
        assert_eq!(&body, before, "{path}: the reader reads what they read");
        let text = body.to_string().to_lowercase();
        for word in ["pinned", "archived", "nested", "rail"] {
            assert!(!text.contains(word), "{word} in {text}");
        }
    }
    // the owner's view of the thread says both, and still says it is shared
    let r = h.get(&format!("/api/threads/{id}"), Some("alice")).await;
    assert_eq!(r.status, 200);
    let own = r.json();
    assert_eq!(own["pinned"], true);
    assert_eq!(own["archived"], true);
    assert_eq!(own["share"]["visibility"], "public");
    // and so does the list, which keeps the share badge for the archived
    let r = h.get("/api/threads?archived=only", Some("alice")).await;
    assert_eq!(r.json()[0]["share"]["effective"], "public");
}
