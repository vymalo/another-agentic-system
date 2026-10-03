//! The resource API against `docs/api/chat-api.yaml`: every operation of the contract that this
//! crate serves (health, the agent list, the thread list, one thread, its export, cancel) is driven over real
//! HTTP against the in-memory stack, and each response is validated against the schemas of the
//! contract. The `/agui/*` operations belong to `orch-surface-agui`, whose own contract test
//! covers them.
//!
//! The event log is not an operation any more (the legacy `listEvents` and `streamEvents` were
//! removed on 2026-09-30, ADR 0012; AG-UI projects the log), but its `Event` schema stays in the
//! contract as the description of the log, which the goldens under `docs/api/examples` pin. The
//! events the stack writes, and the goldens, are validated against it here.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig, NewThread,
};
use orch_core::{AgentId, AgentTarget, ThreadId, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds, sample_releases};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const CAROL: &str = "carol@example.com";
const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    orch_auth_header::HeaderAuth,
>;

// ---- the contract -------------------------------------------------------------------------

struct Contract {
    doc: Value,
}

/// Rewrites OpenAPI 3.0 style `{type: T, nullable: true}` into `{type: [T, "null"]}` so a
/// future 3.0-flavoured edit of the contract cannot silently weaken validation.
fn normalize_nullable(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if map.get("nullable") == Some(&Value::Bool(true))
                && let Some(t) = map.get("type").cloned()
            {
                map.remove("nullable");
                map.insert("type".to_owned(), json!([t, "null"]));
            }
            for child in map.values_mut() {
                normalize_nullable(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_nullable),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        let text = std::fs::read_to_string(path).expect("contract file");
        let mut doc: Value = serde_norway::from_str(&text).expect("contract is valid YAML");
        normalize_nullable(&mut doc);
        Contract { doc }
    }

    /// Every `(operationId, path, method)` of the contract.
    fn operations(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (path, item) in self.doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                if let Some(id) = op.get("operationId").and_then(Value::as_str) {
                    out.push((id.to_owned(), path.clone(), method.clone()));
                }
            }
        }
        out
    }

    /// The operations this crate serves: all but the `/agui/*` ones.
    fn operation_ids(&self) -> BTreeSet<String> {
        self.operations()
            .into_iter()
            .filter(|(_, path, _)| !path.starts_with("/agui/"))
            .map(|(id, _, _)| id)
            .collect()
    }

    fn resolve<'a>(&'a self, node: &'a Value) -> &'a Value {
        match node.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let pointer = r.strip_prefix('#').expect("local refs only");
                self.doc.pointer(pointer).expect("dangling $ref")
            }
            None => node,
        }
    }

    /// The declared response of `operation` for `status`, as `(content type, schema)`.
    /// `Some(None)` means the status is documented without a body schema.
    fn response(&self, operation: &str, status: u16) -> Option<Option<(String, Value)>> {
        let (_, path, method) = self
            .operations()
            .into_iter()
            .find(|(id, _, _)| id == operation)
            .unwrap_or_else(|| panic!("unknown operation {operation}"));
        let responses = &self.doc["paths"][&path][&method]["responses"];
        let declared = self.resolve(responses.get(status.to_string())?);
        let content = declared.get("content").and_then(Value::as_object);
        Some(content.and_then(|c| {
            c.iter().next().map(|(ct, media)| {
                (
                    ct.clone(),
                    media.get("schema").cloned().unwrap_or_else(|| json!({})),
                )
            })
        }))
    }

    fn problem_schema(&self) -> Value {
        self.doc["components"]["responses"]["Problem"]["content"]["application/problem+json"]
            ["schema"]
            .clone()
    }

    /// Validates `instance` against `schema`, resolving `#/components/...` in the contract.
    fn validate(&self, schema: &Value, instance: &Value) {
        let mut root = schema.clone();
        let obj = root.as_object_mut().expect("schema object");
        obj.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        obj.insert("components".to_owned(), self.doc["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .expect("schema compiles");
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "instance does not match the contract schema:\n{}\ninstance: {instance}",
            errors.join("\n")
        );
    }

    /// Validates against a named component schema, e.g. `Event`.
    fn validate_component(&self, name: &str, instance: &Value) {
        self.validate(
            &json!({"$ref": format!("#/components/schemas/{name}")}),
            instance,
        );
    }
}

// ---- the harness --------------------------------------------------------------------------

struct Resp {
    status: u16,
    content_type: String,
    cache_control: String,
    location: String,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    server: JoinHandle<()>,
    dispatcher: JoinHandle<()>,
    token: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.token.cancel();
        self.server.abort();
        self.dispatcher.abort();
    }
}

fn fast_dispatcher() -> DispatcherConfig {
    DispatcherConfig {
        concurrency: 8,
        lease: Duration::from_millis(500),
        heartbeat: Duration::from_millis(100),
        poll_interval: Duration::from_millis(50),
        max_attempts: 5,
        backoff_base: Duration::from_millis(20),
        backoff_max: Duration::from_millis(200),
        poll_min: Duration::from_millis(20),
        poll_max: Duration::from_millis(100),
        max_poll_failures: 10,
        max_cancel_attempts: 10,
        cancel_retry_delay: Duration::from_millis(50),
        verify_watch: Duration::from_millis(50),
        live_flush: Duration::from_millis(20),
        live_refresh: Duration::from_millis(150),
        live_max_bytes: 64 * 1024,
    }
}

impl Harness {
    /// The resource API with no interaction surface mounted, and a dispatcher over a scripted agent.
    async fn start() -> Self {
        let agent = ScriptedAgent::new().with_releases("coder", sample_releases());
        let entry = |id: &str, name: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
        };
        let directory = AgentDirectory::new(vec![
            entry("coder", "Coder"),
            entry("plain", "Plain"),
            // Hosted in-process (ADR 0015): the only agent without a card URL.
            AgentEntry {
                endpoint: AgentEndpoint::local(AgentId::new("helper"), "echo"),
                name: "Helper".to_owned(),
            },
        ]);
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: agent,
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: orch_ports::NoModel,
                    auth: orch_auth_header::HeaderAuth::new(),
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    // sharing by a revocable link (ADR 0040), up to the world
                    sharing: orch_app::SharingSettings::new(
                        orch_app::SharingMode::Public,
                        Some(
                            orch_app::ShareKeys::new(
                                secrecy::SecretString::from(
                                    "0123456789abcdef0123456789abcdef".to_owned(),
                                ),
                                None,
                            )
                            .unwrap(),
                        ),
                        orch_app::PublicView::default(),
                    )
                    .unwrap(),
                    // the servers a person may attach (ADR 0024): the second one is the coder's
                    tool_servers: vec![
                        orch_app::ToolServerInfo {
                            description: Some("Search the web.".to_owned()),
                            icon: Some("data:image/svg+xml;base64,PHN2Zy8+".to_owned()),
                            ..orch_app::ToolServerInfo::new("websearch", "Web search")
                        },
                        orch_app::ToolServerInfo {
                            agents: Some(vec![AgentId::new("coder")]),
                            ..orch_app::ToolServerInfo::new("repos", "Repositories")
                        },
                        orch_app::ToolServerInfo::new("docs", "Documentation"),
                    ],
                    ..AppConfig::default()
                },
            )
            .expect("a valid gate"),
        );
        let token = CancellationToken::new();
        let dispatcher = tokio::spawn(
            Dispatcher::new(Arc::clone(&app), fast_dispatcher(), "test-dispatcher")
                .run(token.clone()),
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
            server,
            dispatcher,
            token,
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, user: Option<&str>) -> Resp {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let cache_control = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        Resp {
            status,
            content_type,
            cache_control,
            location,
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get(&self, path: &str, user: Option<&str>) -> Resp {
        self.send(reqwest::Method::GET, path, user).await
    }

    /// A `PATCH` with a body (`None` sends none).
    async fn patch(&self, path: &str, user: Option<&str>, body: Option<&str>) -> Resp {
        let mut req = self
            .client
            .request(reqwest::Method::PATCH, format!("{}{path}", self.base));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(body.to_owned());
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let cache_control = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        Resp {
            status,
            content_type,
            cache_control,
            location,
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    /// A `PUT` with a body (`None` sends none).
    async fn put(&self, path: &str, user: Option<&str>, body: Option<&str>) -> Resp {
        let mut req = self
            .client
            .request(reqwest::Method::PUT, format!("{}{path}", self.base));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(body.to_owned());
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        };
        let (content_type, cache_control, location) = (
            header("content-type"),
            header("cache-control"),
            header("location"),
        );
        Resp {
            status,
            content_type,
            cache_control,
            location,
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    /// A `POST` with a body (`None` sends none).
    async fn post(&self, path: &str, user: Option<&str>, body: Option<&str>) -> Resp {
        let mut req = self
            .client
            .request(reqwest::Method::POST, format!("{}{path}", self.base));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(body.to_owned());
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        };
        let (content_type, cache_control, location) = (
            header("content-type"),
            header("cache-control"),
            header("location"),
        );
        Resp {
            status,
            content_type,
            cache_control,
            location,
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    /// Creates a thread through the application (the resource API cannot create one) and
    /// returns its id.
    async fn create(
        &self,
        user: &str,
        title: Option<&str>,
        agent: &str,
        release: Option<&str>,
        text: &str,
    ) -> String {
        let thread = self
            .app
            .create_thread(
                &UserId::new(user),
                NewThread {
                    title: title.map(str::to_owned),
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: release.map(str::to_owned),
                    },
                    text: text.to_owned(),
                },
            )
            .await
            .unwrap();
        thread.id.to_string()
    }

    async fn wait_state(&self, user: &str, id: &str, want: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let r = self.get(&format!("/api/threads/{id}"), Some(user)).await;
            let got = r.json()["state"].as_str().unwrap().to_owned();
            if got == want {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {want}, still {got}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The thread's event log, as the core wrote it.
    async fn events(&self, user: &str, id: &str) -> Vec<Value> {
        let id: ThreadId = id.parse().unwrap();
        self.app
            .list_events(&UserId::new(user), id, 0, 500)
            .await
            .unwrap()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    }
}

fn shape(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            let kind = e["kind"].as_str().unwrap();
            match kind {
                "agent_status" => format!("{kind}:{}", e["data"]["status"].as_str().unwrap()),
                "thread_state" => format!("{kind}:{}", e["data"]["state"].as_str().unwrap()),
                _ => kind.to_owned(),
            }
        })
        .collect()
}

/// Ties the contract to real responses.
struct Conformance {
    contract: Contract,
    exercised: BTreeSet<String>,
    /// `(operation, status)` for responses the contract does not document.
    undocumented: BTreeSet<(String, u16)>,
}

impl Conformance {
    fn new() -> Self {
        Conformance {
            contract: Contract::load(),
            exercised: BTreeSet::new(),
            undocumented: BTreeSet::new(),
        }
    }

    /// Checks a response of `operation`: the status is documented (or, if not, at least a
    /// well-formed problem), the media type matches, and the body validates against the schema.
    /// No response of the resource API is deprecated: none carries `Deprecation`.
    fn check(&mut self, operation: &str, resp: &Resp) {
        self.exercised.insert(operation.to_owned());
        match self.contract.response(operation, resp.status) {
            Some(Some((content_type, schema))) => {
                assert!(
                    resp.content_type.starts_with(&content_type),
                    "{operation} {}: content-type {:?}, contract says {content_type}",
                    resp.status,
                    resp.content_type
                );
                if content_type.contains("json") {
                    self.contract.validate(&schema, &resp.json());
                }
            }
            Some(None) => {}
            None => {
                self.undocumented
                    .insert((operation.to_owned(), resp.status));
                assert!(
                    resp.content_type.starts_with("application/problem+json"),
                    "undocumented {operation} {} must be a problem, got {:?}",
                    resp.status,
                    resp.content_type
                );
                self.contract
                    .validate(&self.contract.problem_schema(), &resp.json());
            }
        }
    }
}

// ---- the tests ----------------------------------------------------------------------------

#[tokio::test]
async fn every_operation_of_the_resource_api_conforms_to_the_contract() {
    let h = Harness::start().await;
    let mut c = Conformance::new();

    // getHealth / getReady
    for (op, path) in [("getHealth", "/healthz"), ("getReady", "/readyz")] {
        let r = h.get(path, None).await;
        assert_eq!(r.status, 200);
        c.check(op, &r);
    }

    // 401 on every operation that requires identity.
    let auth_ops: [(&str, reqwest::Method, String); 18] = [
        ("getMe", reqwest::Method::GET, "/api/me".into()),
        ("listAgents", reqwest::Method::GET, "/api/agents".into()),
        ("getRegistry", reqwest::Method::GET, "/api/registry".into()),
        ("getConfig", reqwest::Method::GET, "/api/config".into()),
        (
            "listToolServers",
            reqwest::Method::GET,
            "/api/tool-servers".into(),
        ),
        ("listThreads", reqwest::Method::GET, "/api/threads".into()),
        (
            "getThread",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}"),
        ),
        (
            "exportThread",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}/export"),
        ),
        (
            "putThreadTools",
            reqwest::Method::PUT,
            format!("/api/threads/{RANDOM}/tools"),
        ),
        (
            "cancelThread",
            reqwest::Method::POST,
            format!("/api/threads/{RANDOM}/cancel"),
        ),
        (
            "forkThread",
            reqwest::Method::POST,
            format!("/api/threads/{RANDOM}/fork"),
        ),
        (
            "listBranches",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}/branches"),
        ),
        (
            "getArtifact",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}/artifacts/{}", "ab".repeat(32)),
        ),
        (
            "shareThread",
            reqwest::Method::PUT,
            format!("/api/threads/{RANDOM}/share"),
        ),
        (
            "rotateThreadShare",
            reqwest::Method::POST,
            format!("/api/threads/{RANDOM}/share/rotate"),
        ),
        (
            "unshareThread",
            reqwest::Method::DELETE,
            format!("/api/threads/{RANDOM}/share"),
        ),
        (
            "getSharedThread",
            reqwest::Method::GET,
            format!("/api/shared/{}", "A".repeat(43)),
        ),
        (
            "getSharedArtifact",
            reqwest::Method::GET,
            format!(
                "/api/shared/{}/artifacts/{}",
                "A".repeat(43),
                "ab".repeat(32)
            ),
        ),
    ];
    for (op, method, path) in auth_ops {
        let r = h.send(method, &path, None).await;
        assert_eq!(r.status, 401, "{op}");
        c.check(op, &r);
    }

    // getMe: who the proxy says, with the roles of the default role (the header carries none)
    let r = h.get("/api/me", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.cache_control, "no-store");
    c.check("getMe", &r);
    let me = r.json();
    assert_eq!(me["user"], ALICE);
    assert_eq!(me["roles"], json!(["user"]));
    assert_eq!(
        me["permissions"],
        json!([
            {"permission": "agent.read"},
            {"permission": "agent.invoke"},
            {"permission": "thread.read", "scope": "own"},
            {"permission": "thread.write", "scope": "own"},
            {"permission": "thread.share"},
            {"permission": "artifact.read", "scope": "own"},
        ])
    );
    assert_eq!(
        me["sharing"], "public",
        "this deployment shares up to the world"
    );
    assert_eq!(me["agents"], json!({"read": ["*"], "invoke": ["*"]}));

    // listAgents
    let r = h.get("/api/agents", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("listAgents", &r);
    let agents = r.json();
    let by_id = |id: &str| {
        agents
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap()
            .clone()
    };
    let coder = by_id("coder");
    // Every agent of this deployment is in its own list (ADR 0022); `tags` is absent, not empty.
    assert_eq!(coder["source"], "static");
    assert!(coder.get("tags").is_none(), "{coder}");
    assert_eq!(coder["releases"]["defaultChannel"], "stable");
    assert_eq!(coder["releases"]["channels"]["staging"], "rev-2");
    let plain = by_id("plain");
    assert!(plain.get("releases").is_none());
    // `cardUrl` is optional in the contract: an in-process agent has none, and the key is absent
    // (not null); every A2A agent keeps it.
    let helper = by_id("helper");
    assert!(helper.get("cardUrl").is_none(), "{helper}");
    assert_eq!(helper["name"], "Helper");
    assert_eq!(
        plain["cardUrl"],
        "https://plain.example.com/.well-known/agent-card.json"
    );

    // getRegistry: the deployment's own list is a source that is always there, and the answer is
    // never cached.
    let r = h.get("/api/registry", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("getRegistry", &r);
    assert_eq!(
        r.json(),
        json!({"sources": [{"name": "static", "status": "ok"}]})
    );
    assert_eq!(r.cache_control, "no-store");

    // getConfig: exactly `{ui}` with every key at its effective value, behind the identity layer
    // like every /api route (the 401 is checked with the others above)
    let r = h.get("/api/config", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("getConfig", &r);
    assert_eq!(r.json(), json!({"ui": {"showDescriptions": true}}));

    // listToolServers (ADR 0024): the deployment's list, in its order, with no URL, header,
    // credential or allow-list; `agents` only where the deployment limits a server.
    let r = h.get("/api/tool-servers", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.cache_control, "no-store");
    c.check("listToolServers", &r);
    assert_eq!(
        r.json(),
        json!([
            {"id": "websearch", "name": "Web search", "description": "Search the web.",
             "icon": "data:image/svg+xml;base64,PHN2Zy8+"},
            {"id": "repos", "name": "Repositories", "agents": ["coder"]},
            {"id": "docs", "name": "Documentation"},
        ])
    );

    // putThreadTools: 200 with the set after (sorted), the same set again is the same 200 and writes
    // nothing, `Thread.tools` says it, and the log has the events.
    let tooled = h
        .create(CAROL, Some("Tools"), "plain", None, "echo tools")
        .await;
    h.wait_state(CAROL, &tooled, "done").await;
    let before = h.events(CAROL, &tooled).await.len();
    let path = format!("/api/threads/{tooled}/tools");
    let r = h
        .put(
            &path,
            Some(CAROL),
            Some(r#"{"servers":["websearch","docs"]}"#),
        )
        .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    c.check("putThreadTools", &r);
    assert_eq!(r.json(), json!({"servers": ["docs", "websearch"]}));
    let r = h
        .put(
            &path,
            Some(CAROL),
            Some(r#"{"servers":["websearch","docs","docs"]}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("putThreadTools", &r);
    assert_eq!(r.json(), json!({"servers": ["docs", "websearch"]}));
    let log = h.events(CAROL, &tooled).await;
    assert_eq!(
        shape(&log[before..]),
        ["tools_attached"],
        "one event, not two"
    );
    assert_eq!(
        log[before]["data"],
        json!({"servers": ["docs", "websearch"]})
    );
    c.contract.validate_component("Event", &log[before]);
    let r = h.get(&format!("/api/threads/{tooled}"), Some(CAROL)).await;
    c.check("getThread", &r);
    assert_eq!(r.json()["tools"], json!(["docs", "websearch"]));
    // a detach, and the empty set: `tools` is absent when there are none
    let r = h
        .put(&path, Some(CAROL), Some(r#"{"servers":["docs"]}"#))
        .await;
    assert_eq!(r.json(), json!({"servers": ["docs"]}));
    let log = h.events(CAROL, &tooled).await;
    assert_eq!(log.last().unwrap()["kind"], "tools_detached");
    assert_eq!(
        log.last().unwrap()["data"],
        json!({"servers": ["websearch"]})
    );
    let r = h.put(&path, Some(CAROL), Some(r#"{"servers":[]}"#)).await;
    assert_eq!(r.status, 200);
    c.check("putThreadTools", &r);
    assert_eq!(r.json(), json!({"servers": []}));
    let r = h.get(&format!("/api/threads/{tooled}"), Some(CAROL)).await;
    assert!(r.json().get("tools").is_none(), "{}", r.json());
    // 400: what is not exactly `{servers: [string]}`, and an id that is not one; nothing is written
    let written = h.events(CAROL, &tooled).await.len();
    for body in [
        None,
        Some(r#"not json"#),
        Some(r#"[]"#),
        Some(r#"{}"#),
        Some(r#"{"servers":"docs"}"#),
        Some(r#"{"servers":[1]}"#),
        Some(r#"{"servers":["docs"],"extra":1}"#),
        Some(r#"{"servers":["Web Search"]}"#),
        Some(r#"{"servers":[""]}"#),
    ] {
        let r = h.put(&path, Some(CAROL), body).await;
        assert_eq!(
            r.status,
            400,
            "{body:?}: {}",
            String::from_utf8_lossy(&r.body)
        );
        c.check("putThreadTools", &r);
    }
    // 404: a thread that is not the caller's, and one that does not exist, alike
    for (user, id) in [
        (BOB, tooled.as_str()),
        (CAROL, RANDOM),
        (CAROL, "not-a-uuid"),
    ] {
        let r = h
            .put(
                &format!("/api/threads/{id}/tools"),
                Some(user),
                Some(r#"{"servers":["docs"]}"#),
            )
            .await;
        assert_eq!(r.status, 404, "{user} {id}");
        c.check("putThreadTools", &r);
    }
    // 422: a server nobody lists, one that is not for this thread's agent, too many; and a server
    // the plain agent may not have is one the coder may
    for body in [
        r#"{"servers":["nosuch"]}"#.to_owned(),
        r#"{"servers":["repos"]}"#.to_owned(),
        serde_json::to_string(
            &json!({"servers": (0..17).map(|n| format!("s{n}")).collect::<Vec<_>>()}),
        )
        .unwrap(),
    ] {
        let r = h.put(&path, Some(CAROL), Some(&body)).await;
        assert_eq!(r.status, 422, "{body}");
        c.check("putThreadTools", &r);
        let text = String::from_utf8_lossy(&r.body).to_string();
        assert!(!text.contains("http"), "no URL in a refusal: {text}");
    }
    assert_eq!(
        h.events(CAROL, &tooled).await.len(),
        written,
        "the refusals wrote nothing"
    );
    let coders = h
        .create(CAROL, Some("Repos"), "coder", None, "echo repos")
        .await;
    let r = h
        .put(
            &format!("/api/threads/{coders}/tools"),
            Some(CAROL),
            Some(r#"{"servers":["repos"]}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("putThreadTools", &r);
    // (403 is the roles' business, in `rbac.rs`: the proxy header carries none)

    // getThread
    let id = h
        .create(
            ALICE,
            Some("Contract"),
            "coder",
            Some("staging"),
            "echo contract",
        )
        .await;
    h.wait_state(ALICE, &id, "done").await;
    let r = h.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("getThread", &r);
    let thread = r.json();
    assert_eq!(thread["state"], "done");
    assert_eq!(thread["title"], "Contract");
    assert_eq!(
        thread["target"],
        json!({"agentId": "coder", "release": "staging"})
    );
    assert_eq!(thread["lastSeq"], 5);
    for path in [
        format!("/api/threads/{RANDOM}"),
        "/api/threads/not-a-uuid".to_owned(),
    ] {
        let r = h.get(&path, Some(ALICE)).await;
        assert_eq!(r.status, 404);
        c.check("getThread", &r);
    }
    // Someone else's thread is a 404 too, not a 403.
    let r = h.get(&format!("/api/threads/{id}"), Some(BOB)).await;
    assert_eq!(r.status, 404);
    c.check("getThread", &r);

    // exportThread: the finished thread, with the whole log and the agent's binding.
    let r = h
        .get(&format!("/api/threads/{id}/export"), Some(ALICE))
        .await;
    assert_eq!(r.status, 200);
    c.check("exportThread", &r);
    let export = r.json();
    assert_eq!(export["thread"], thread, "`thread` is what getThread says");
    assert_eq!(
        export["job"]["number"], 1,
        "the ledger says which job it is"
    );
    assert_eq!(export["events"].as_array().unwrap().len(), 5);
    assert_eq!(export["events"][4]["data"]["state"], "done");
    assert_eq!(export["binding"]["agentId"], "coder");
    assert_eq!(export["binding"]["revision"], "rev-2");
    assert_eq!(export["binding"]["taskState"], "completed");
    assert!(export["binding"]["taskId"].is_string());
    for path in [
        format!("/api/threads/{RANDOM}/export"),
        "/api/threads/not-a-uuid/export".to_owned(),
    ] {
        let r = h.get(&path, Some(ALICE)).await;
        assert_eq!(r.status, 404);
        c.check("exportThread", &r);
    }
    let r = h.get(&format!("/api/threads/{id}/export"), Some(BOB)).await;
    assert_eq!(r.status, 404);
    c.check("exportThread", &r);

    // getArtifact (ADR 0032): this stack has no artifact store, so a file is a 404 whoever asks, and
    // a `download` that is not 1 is a 400; `tests/artifacts.rs` drives the route against stores
    // that hold files.
    let sha = "ab".repeat(32);
    for (user, path) in [
        (ALICE, format!("/api/threads/{id}/artifacts/{sha}")),
        (BOB, format!("/api/threads/{id}/artifacts/{sha}")),
        (ALICE, format!("/api/threads/{RANDOM}/artifacts/{sha}")),
        (ALICE, format!("/api/threads/{id}/artifacts/nope")),
    ] {
        let r = h.get(&path, Some(user)).await;
        assert_eq!(r.status, 404, "{user} {path}");
        c.check("getArtifact", &r);
    }
    let r = h
        .get(
            &format!("/api/threads/{id}/artifacts/{sha}?download=2"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status, 400);
    c.check("getArtifact", &r);

    // listThreads
    for i in 0..3 {
        h.create(ALICE, None, "plain", None, &format!("echo list {i}"))
            .await;
    }
    let r = h.get("/api/threads", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("listThreads", &r);
    let all = r.json();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 4);
    let ids: Vec<&str> = all.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids[3], id, "newest first: the first thread is last");
    let r = h.get("/api/threads?limit=2", Some(ALICE)).await;
    c.check("listThreads", &r);
    assert_eq!(r.json().as_array().unwrap().len(), 2);
    let r = h
        .get(
            &format!("/api/threads?limit=2&before={}", ids[1]),
            Some(ALICE),
        )
        .await;
    c.check("listThreads", &r);
    let page: Vec<String> = r
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page, [ids[2], ids[3]]);
    for bad in ["limit=0", "limit=101", "limit=abc", "before=not-a-uuid"] {
        let r = h.get(&format!("/api/threads?{bad}"), Some(ALICE)).await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("listThreads", &r);
    }
    assert!(
        h.get("/api/threads", Some(BOB))
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty()
    );

    // The log of the finished thread, which the contract still describes as `Event`.
    let events = h.events(ALICE, &id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    for event in &events {
        c.contract.validate_component("Event", event);
    }
    let seqs: Vec<i64> = events.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    // The release echo: agent events name the revision that served them.
    assert_eq!(
        events[1]["actor"],
        json!({"type": "agent", "name": "coder", "revision": "rev-2"})
    );
    assert_eq!(events[0]["actor"], json!({"type": "user", "name": ALICE}));
    assert_eq!(
        events[4]["actor"],
        json!({"type": "system", "name": "orchestrator"})
    );

    // cancelThread: 202 while running; 404.
    let slow = h.create(ALICE, None, "plain", None, "slow please").await;
    h.wait_state(ALICE, &slow, "working").await;
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("/api/threads/{slow}/cancel"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status, 202);
    c.check("cancelThread", &r);
    assert!(r.body.is_empty());
    h.wait_state(ALICE, &slow, "cancelled").await;
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("/api/threads/{slow}/cancel"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status, 202, "cancelling a finished thread is a no-op");
    c.check("cancelThread", &r);
    let r = h
        .send(
            reqwest::Method::POST,
            &format!("/api/threads/{RANDOM}/cancel"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("cancelThread", &r);

    // patchThread: a rename is 200 with the thread; 400 for what cannot be a title; 404; 401.
    let r = h
        .patch(
            &format!("/api/threads/{RANDOM}"),
            None,
            Some(r#"{"title":"x"}"#),
        )
        .await;
    assert_eq!(r.status, 401);
    c.check("patchThread", &r);
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"title":"  A better name  "}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("patchThread", &r);
    let renamed = r.json();
    assert_eq!(renamed["title"], "A better name", "the title is trimmed");
    assert_eq!(renamed["id"], id.as_str());
    assert_eq!(renamed["state"], "done", "a finished thread can be renamed");
    let r = h.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    assert_eq!(r.json()["title"], "A better name");
    let listed = h.get("/api/threads", Some(ALICE)).await.json();
    let titles: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"A better name"), "{titles:?}");
    let events = h.events(ALICE, &id).await;
    let last = events.last().unwrap();
    assert_eq!(last["kind"], "thread_titled");
    assert_eq!(
        last["data"],
        json!({"title": "A better name", "source": "user"})
    );
    assert_eq!(last["actor"], json!({"type": "user", "name": ALICE}));
    c.contract.validate_component("Event", last);
    // the same title again writes nothing
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"title":"A better name"}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(h.events(ALICE, &id).await.len(), events.len());
    let long = format!(r#"{{"title":"{}"}}"#, "x".repeat(201));
    for bad in [
        r#"{"title":""}"#,
        r#"{"title":"   "}"#,
        r#"{"title":"two\nlines"}"#,
        r#"{"title":"nul\u0000"}"#,
        r#"{"title":3}"#,
        r#"{"title":null}"#,
        r#"{}"#,
        r#"{"title":"x","state":"done"}"#,
        r#"["title"]"#,
        "not json",
        long.as_str(),
    ] {
        let r = h
            .patch(&format!("/api/threads/{id}"), Some(ALICE), Some(bad))
            .await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("patchThread", &r);
    }
    let r = h
        .patch(&format!("/api/threads/{id}"), Some(ALICE), None)
        .await;
    assert_eq!(r.status, 400, "no body");
    c.check("patchThread", &r);
    assert_eq!(
        h.get(&format!("/api/threads/{id}"), Some(ALICE))
            .await
            .json()["title"],
        "A better name",
        "a refused rename changes nothing"
    );
    for (user, path) in [(ALICE, RANDOM), (BOB, id.as_str()), (ALICE, "not-a-uuid")] {
        let r = h
            .patch(
                &format!("/api/threads/{path}"),
                Some(user),
                Some(r#"{"title":"Mine"}"#),
            )
            .await;
        assert_eq!(r.status, 404, "{user} {path}");
        c.check("patchThread", &r);
    }
    assert_eq!(
        h.get(&format!("/api/threads/{id}"), Some(ALICE))
            .await
            .json()["title"],
        "A better name",
        "someone else's rename changes nothing"
    );

    // patchThread with a description (ADR 0035): 200 with the thread, which says it; the event is
    // the person's; empty clears it; the same again writes nothing; 400 changes nothing, even
    // when the other member is good; a title and a description go together.
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"description":"  Moving the build to Rust.  "}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("patchThread", &r);
    assert_eq!(r.json()["description"], "Moving the build to Rust.");
    assert_eq!(r.json()["title"], "A better name", "the title is untouched");
    let r = h.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    c.check("getThread", &r);
    assert_eq!(r.json()["description"], "Moving the build to Rust.");
    let listed = h.get("/api/threads", Some(ALICE)).await;
    c.check("listThreads", &listed);
    assert!(
        listed
            .json()
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["description"] == "Moving the build to Rust."),
        "the listing says it"
    );
    let events = h.events(ALICE, &id).await;
    let last = events.last().unwrap();
    assert_eq!(last["kind"], "thread_described");
    assert_eq!(
        last["data"],
        json!({"description": "Moving the build to Rust.", "source": "user"})
    );
    assert_eq!(last["actor"], json!({"type": "user", "name": ALICE}));
    c.contract.validate_component("Event", last);
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"description":"Moving the build to Rust."}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        h.events(ALICE, &id).await.len(),
        events.len(),
        "the same again writes nothing"
    );
    let long = format!(r#"{{"description":"{}"}}"#, "x".repeat(501));
    for bad in [
        r#"{"description":"two\nlines"}"#,
        r#"{"description":3}"#,
        r#"{"description":null}"#,
        r#"{"description":"x","state":"done"}"#,
        // one bad member refuses both: nothing is written
        r#"{"title":"Fine","description":"two\nlines"}"#,
        r#"{"title":"","description":"Fine"}"#,
        long.as_str(),
    ] {
        let r = h
            .patch(&format!("/api/threads/{id}"), Some(ALICE), Some(bad))
            .await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("patchThread", &r);
    }
    let after = h
        .get(&format!("/api/threads/{id}"), Some(ALICE))
        .await
        .json();
    assert_eq!(after["title"], "A better name");
    assert_eq!(after["description"], "Moving the build to Rust.");
    assert_eq!(
        h.events(ALICE, &id).await.len(),
        events.len(),
        "a refused patch writes nothing"
    );
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"title":"Both","description":"And this."}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("patchThread", &r);
    assert_eq!(
        (r.json()["title"].as_str(), r.json()["description"].as_str()),
        (Some("Both"), Some("And this."))
    );
    // empty clears it: the thread has none, and the response has no member for it
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(ALICE),
            Some(r#"{"description":""}"#),
        )
        .await;
    assert_eq!(r.status, 200);
    c.check("patchThread", &r);
    assert!(r.json().get("description").is_none(), "{}", r.json());
    let last = h.events(ALICE, &id).await.pop().unwrap();
    assert_eq!(last["data"], json!({"description": "", "source": "user"}));
    c.contract.validate_component("Event", &last);
    // someone else's thread is not found
    let r = h
        .patch(
            &format!("/api/threads/{id}"),
            Some(BOB),
            Some(r#"{"description":"Mine"}"#),
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("patchThread", &r);

    // forkThread and listBranches ([ADR 0029]).
    let parent = h
        .create(ALICE, Some("Forkable"), "plain", None, "echo fork me")
        .await;
    h.wait_state(ALICE, &parent, "done").await;
    let fork_path = format!("/api/threads/{parent}/fork");
    // fork from here: 201, a Location, a finished thread that says where it came from
    let r = h
        .post(&fork_path, Some(ALICE), Some(r#"{"after":1}"#))
        .await;
    assert_eq!(r.status, 201);
    c.check("forkThread", &r);
    let forked = r.json();
    let fork_id = forked["id"].as_str().unwrap().to_owned();
    assert_ne!(fork_id, parent);
    assert_eq!(r.location, format!("/api/threads/{fork_id}"));
    assert_eq!(forked["state"], "done");
    assert_eq!(forked["title"], "Forkable");
    assert_eq!(
        forked["forkedFrom"],
        json!({"threadId": parent, "seq": 5, "kind": "fork"})
    );
    assert_eq!(forked["lastSeq"], 6);
    assert_eq!(forked["target"], json!({"agentId": "plain"}));
    let fork_events = h.events(ALICE, &fork_id).await;
    assert_eq!(
        shape(&fork_events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done",
            "thread_forked"
        ]
    );
    for event in &fork_events {
        c.contract.validate_component("Event", event);
    }
    assert_eq!(
        fork_events[5]["data"],
        json!({
            "from": {"threadId": parent, "seq": 5},
            "kind": "fork",
            "title": "Forkable",
            "target": {"agentId": "plain"}
        })
    );
    // the thread reads back the same, and a fork of a thread that is not a fork has no `forkedFrom`
    let r = h.get(&format!("/api/threads/{fork_id}"), Some(ALICE)).await;
    c.check("getThread", &r);
    assert_eq!(r.json(), forked);
    let r = h.get(&format!("/api/threads/{parent}"), Some(ALICE)).await;
    assert!(r.json().get("forkedFrom").is_none());

    // a repeat with the same id answers the fork it made, and writes nothing
    let chosen = "0190bbbb-0000-7000-8000-000000000001";
    let body = format!(
        r#"{{"after":3,"id":"{chosen}","target":{{"agentId":"coder","release":"staging"}}}}"#
    );
    let r = h.post(&fork_path, Some(ALICE), Some(&body)).await;
    assert_eq!(r.status, 201);
    c.check("forkThread", &r);
    let chosen_fork = r.json();
    assert_eq!(chosen_fork["id"], chosen);
    assert_eq!(
        chosen_fork["target"],
        json!({"agentId": "coder", "release": "staging"})
    );
    let r = h.post(&fork_path, Some(ALICE), Some(&body)).await;
    assert_eq!(r.status, 200);
    c.check("forkThread", &r);
    assert_eq!(r.json(), chosen_fork);
    assert_eq!(r.location, format!("/api/threads/{chosen}"));

    // a fork made with its first message (ADR 0042): `after` with `text`, queued at once, the
    // message after `thread_forked`, and the same request again is the fork it made
    let lazy = "0190bbbb-0000-7000-8000-000000000002";
    let lazy_body =
        format!(r#"{{"after":1,"text":"echo from the fork","messageId":"m-lazy","id":"{lazy}"}}"#);
    let r = h.post(&fork_path, Some(ALICE), Some(&lazy_body)).await;
    assert_eq!(r.status, 201);
    c.check("forkThread", &r);
    let lazy_fork = r.json();
    assert_eq!(lazy_fork["id"], lazy);
    assert_eq!(lazy_fork["state"], "queued");
    assert_eq!(
        lazy_fork["forkedFrom"],
        json!({"threadId": parent, "seq": 5, "kind": "fork"})
    );
    h.wait_state(ALICE, lazy, "done").await;
    let lazy_events = h.events(ALICE, lazy).await;
    assert_eq!(
        shape(&lazy_events)[5..8],
        ["thread_forked", "user_message", "job_started"]
    );
    assert_eq!(lazy_events[6]["data"]["text"], "echo from the fork");
    assert_eq!(lazy_events[6]["data"]["messageId"], "m-lazy");
    for event in &lazy_events {
        c.contract.validate_component("Event", event);
    }
    let r = h.post(&fork_path, Some(ALICE), Some(&lazy_body)).await;
    assert_eq!(r.status, 200, "the same request again");
    c.check("forkThread", &r);
    assert_eq!(r.json()["id"], lazy);
    assert_eq!(h.events(ALICE, lazy).await.len(), lazy_events.len());
    // another message with that id is another thread's id: a conflict, nothing is written
    let other = format!(r#"{{"after":1,"text":"echo something else","id":"{lazy}"}}"#);
    let r = h.post(&fork_path, Some(ALICE), Some(&other)).await;
    assert_eq!(r.status, 409);
    c.check("forkThread", &r);
    assert_eq!(h.events(ALICE, lazy).await.len(), lazy_events.len());

    // an edit: queued at once, and a branch (left out of the list, found by `listBranches`)
    let r = h
        .post(
            &fork_path,
            Some(ALICE),
            Some(r#"{"replace":1,"text":"echo edited","messageId":"m-edit"}"#),
        )
        .await;
    assert_eq!(r.status, 201);
    c.check("forkThread", &r);
    let edit = r.json();
    let edit_id = edit["id"].as_str().unwrap().to_owned();
    assert_eq!(
        edit["forkedFrom"],
        json!({"threadId": parent, "seq": 0, "kind": "edit"})
    );
    h.wait_state(ALICE, &edit_id, "done").await;
    let edit_events = h.events(ALICE, &edit_id).await;
    assert_eq!(
        shape(&edit_events)[..3],
        ["thread_forked", "user_message", "job_started"]
    );
    assert_eq!(edit_events[1]["data"]["text"], "echo edited");
    assert_eq!(edit_events[1]["data"]["messageId"], "m-edit");
    for event in &edit_events {
        c.contract.validate_component("Event", event);
    }
    let r = h.get("/api/threads?limit=100", Some(ALICE)).await;
    c.check("listThreads", &r);
    let listed: Vec<String> = r
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        listed.contains(&fork_id) && listed.iter().any(|t| t == chosen) && listed.contains(&parent)
    );
    assert!(!listed.contains(&edit_id), "a branch is not listed");
    let r = h
        .get("/api/threads?limit=100&branches=include", Some(ALICE))
        .await;
    c.check("listThreads", &r);
    assert!(
        r.json()
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == edit_id.as_str())
    );
    let r = h.get("/api/threads?branches=all", Some(ALICE)).await;
    assert_eq!(r.status, 400);
    c.check("listThreads", &r);

    let r = h
        .get(&format!("/api/threads/{edit_id}/branches"), Some(ALICE))
        .await;
    assert_eq!(r.status, 200);
    c.check("listBranches", &r);
    assert_eq!(
        r.json(),
        json!({
            "root": parent,
            "points": [{
                "seq": 2,
                "index": 1,
                "siblings": [
                    {"threadId": parent, "seq": 1, "title": "Forkable"},
                    {"threadId": edit_id, "seq": 2, "title": "Forkable"}
                ]
            }]
        })
    );
    let r = h
        .get(&format!("/api/threads/{parent}/branches"), Some(ALICE))
        .await;
    c.check("listBranches", &r);
    assert_eq!(r.json()["points"][0]["index"], 0);
    assert_eq!(r.json()["points"][0]["seq"], 1);
    let r = h
        .get(&format!("/api/threads/{fork_id}/branches"), Some(ALICE))
        .await;
    c.check("listBranches", &r);
    assert_eq!(r.json(), json!({"root": fork_id, "points": []}));
    for (user, path) in [
        (ALICE, RANDOM),
        (ALICE, "not-a-uuid"),
        (BOB, parent.as_str()),
    ] {
        let r = h
            .get(&format!("/api/threads/{path}/branches"), Some(user))
            .await;
        assert_eq!(r.status, 404, "{user} {path}");
        c.check("listBranches", &r);
    }

    // a body that cannot be used
    let long = format!(r#"{{"replace":1,"text":"{}"}}"#, "x".repeat(100_001));
    let long_id = format!(
        r#"{{"replace":1,"text":"x","messageId":"{}"}}"#,
        "m".repeat(257)
    );
    let long_after = format!(r#"{{"after":1,"text":"{}"}}"#, "x".repeat(100_001));
    let long_after_id = format!(
        r#"{{"after":1,"text":"x","messageId":"{}"}}"#,
        "m".repeat(257)
    );
    for bad in [
        r#"{}"#,
        r#"{"after":1,"replace":1,"text":"x"}"#,
        r#"{"after":1,"messageId":"m"}"#,
        r#"{"after":1,"text":""}"#,
        r#"{"after":1,"text":"   "}"#,
        long_after.as_str(),
        long_after_id.as_str(),
        r#"{"replace":1}"#,
        r#"{"replace":1,"text":""}"#,
        r#"{"replace":1,"text":"   "}"#,
        long.as_str(),
        long_id.as_str(),
        r#"{"after":"1"}"#,
        r#"{"after":1.5}"#,
        r#"{"after":null}"#,
        r#"{"after":1,"id":"nope"}"#,
        r#"{"after":1,"target":{"agentId":"nobody"}}"#,
        r#"{"after":1,"target":{"agentId":"plain","release":"stable"}}"#,
        r#"{"after":1,"state":"done"}"#,
        r#"[1]"#,
        "not json",
    ] {
        let r = h.post(&fork_path, Some(ALICE), Some(bad)).await;
        assert_eq!(r.status, 400, "{}", &bad[..bad.len().min(60)]);
        c.check("forkThread", &r);
    }
    let r = h.post(&fork_path, Some(ALICE), None).await;
    assert_eq!(r.status, 400, "no body");
    c.check("forkThread", &r);
    // a point that is not there, or not a person's message
    for bad in [
        r#"{"after":99}"#,
        r#"{"after":0}"#,
        r#"{"after":99,"text":"x"}"#,
        r#"{"replace":99,"text":"x"}"#,
        r#"{"replace":2,"text":"x"}"#,
    ] {
        let r = h.post(&fork_path, Some(ALICE), Some(bad)).await;
        assert_eq!(r.status, 422, "{bad}");
        c.check("forkThread", &r);
    }
    // an id that is another thread's
    let taken = format!(r#"{{"after":1,"id":"{parent}"}}"#);
    let r = h.post(&fork_path, Some(ALICE), Some(&taken)).await;
    assert_eq!(r.status, 409);
    c.check("forkThread", &r);
    assert!(r.json().get("code").is_none());
    // somebody else's thread, one that is missing, and an id that is not one
    for (user, path) in [
        (BOB, parent.as_str()),
        (ALICE, RANDOM),
        (ALICE, "not-a-uuid"),
    ] {
        let r = h
            .post(
                &format!("/api/threads/{path}/fork"),
                Some(user),
                Some(r#"{"after":1}"#),
            )
            .await;
        assert_eq!(r.status, 404, "{user} {path}");
        c.check("forkThread", &r);
    }
    // the turn that is going on cannot be copied: 409, and a client can tell why
    let busy = h.create(ALICE, None, "plain", None, "slow please").await;
    h.wait_state(ALICE, &busy, "working").await;
    let r = h
        .post(
            &format!("/api/threads/{busy}/fork"),
            Some(ALICE),
            Some(r#"{"after":1}"#),
        )
        .await;
    assert_eq!(r.status, 409);
    c.check("forkThread", &r);
    assert_eq!(r.json()["code"], "turn_open");
    // with a message too: the turn is the same
    let r = h
        .post(
            &format!("/api/threads/{busy}/fork"),
            Some(ALICE),
            Some(r#"{"after":1,"text":"echo instead"}"#),
        )
        .await;
    assert_eq!(r.status, 409);
    c.check("forkThread", &r);
    assert_eq!(r.json()["code"], "turn_open");
    // but a message before it can be edited
    let r = h
        .post(
            &format!("/api/threads/{busy}/fork"),
            Some(ALICE),
            Some(r#"{"replace":1,"text":"echo instead"}"#),
        )
        .await;
    assert_eq!(r.status, 201);
    c.check("forkThread", &r);
    // none of the refusals made a thread
    let r = h
        .get("/api/threads?limit=100&branches=include", Some(ALICE))
        .await;
    let count = r.json().as_array().unwrap().len();
    let r = h
        .post(&fork_path, Some(ALICE), Some(r#"{"after":99}"#))
        .await;
    assert_eq!(r.status, 422);
    let r = h
        .get("/api/threads?limit=100&branches=include", Some(ALICE))
        .await;
    assert_eq!(r.json().as_array().unwrap().len(), count);

    // Sharing by a revocable link (ADR 0040): the owner shares, a signed-in reader and anybody
    // read, a new link kills the old one, the owner takes it down.
    let shared = h.create(ALICE, None, "plain", None, "share me").await;
    let share_path = format!("/api/threads/{shared}/share");
    let r = h
        .put(&share_path, Some(ALICE), Some(r#"{"visibility":"public"}"#))
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(r.cache_control, "no-store");
    c.check("shareThread", &r);
    let r = h
        .put(
            &share_path,
            Some(ALICE),
            Some(r#"{"visibility":"private"}"#),
        )
        .await;
    assert_eq!(r.status, 400);
    c.check("shareThread", &r);
    let r = h
        .put(&share_path, Some(BOB), Some(r#"{"visibility":"public"}"#))
        .await;
    assert_eq!(r.status, 404);
    c.check("shareThread", &r);
    let r = h
        .post(&format!("{share_path}/rotate"), Some(ALICE), None)
        .await;
    assert_eq!(r.status, 200);
    c.check("rotateThreadShare", &r);
    let token = r.json()["url"]
        .as_str()
        .unwrap()
        .trim_start_matches("/s/")
        .to_owned();
    let r = h
        .post(
            &format!("/api/threads/{RANDOM}/share/rotate"),
            Some(ALICE),
            None,
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("rotateThreadShare", &r);
    let r = h.get(&format!("/api/shared/{token}"), Some(BOB)).await;
    assert_eq!(r.status, 200);
    c.check("getSharedThread", &r);
    let r = h
        .get(&format!("/api/shared/{}", "A".repeat(43)), Some(BOB))
        .await;
    assert_eq!(r.status, 404);
    c.check("getSharedThread", &r);
    let r = h.get(&format!("/api/public/shared/{token}"), None).await;
    assert_eq!(r.status, 200);
    c.check("getPublicSharedThread", &r);
    let r = h
        .get(&format!("/api/public/shared/{}", "A".repeat(43)), None)
        .await;
    assert_eq!(r.status, 404);
    c.check("getPublicSharedThread", &r);
    // this deployment keeps no files: a file of a shared thread is the one 404, signed in or not
    let file = "ab".repeat(32);
    let r = h
        .get(&format!("/api/shared/{token}/artifacts/{file}"), Some(BOB))
        .await;
    assert_eq!(r.status, 404);
    c.check("getSharedArtifact", &r);
    let r = h
        .get(
            &format!("/api/public/shared/{token}/artifacts/{file}"),
            None,
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("getPublicSharedArtifact", &r);
    let r = h
        .send(reqwest::Method::DELETE, &share_path, Some(ALICE))
        .await;
    assert_eq!(r.status, 204);
    c.check("unshareThread", &r);
    let r = h
        .send(
            reqwest::Method::DELETE,
            &format!("/api/threads/{RANDOM}/share"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("unshareThread", &r);
    assert_eq!(
        h.get(&format!("/api/public/shared/{token}"), None)
            .await
            .status,
        404,
        "taken down"
    );

    // Every operation this crate serves was driven, and the contract has no other.
    assert_eq!(c.exercised, c.contract.operation_ids());
    // The contract documents every status this suite saw (the 400 of `listThreads` since ADR 0039).
    assert_eq!(c.undocumented, BTreeSet::new());
    // Problems carry `about:blank`, a title and the status.
    let r = h.get("/api/threads?limit=0", Some(ALICE)).await;
    let p: Value = r.json();
    assert_eq!(p["type"], "about:blank");
    assert_eq!(p["status"], 400);
    assert_eq!(p["title"], "Bad Request");
}

/// The golden transcripts (`docs/api/examples/*.events.json`, written by `orch-e2e`'s `golden`
/// test from the real stack) are events of the contract: their placeholders aside, each event
/// validates against `Event`, and they run in order from `seq` 1.
#[test]
fn the_golden_transcripts_are_events_of_the_contract() {
    let contract = Contract::load();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.ends_with(".events.json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let events: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert!(!events.is_empty(), "{name}");
        for (n, mut event) in events.into_iter().enumerate() {
            // The goldens replace ids and clocks with placeholders.
            event["threadId"] = json!(RANDOM);
            event["at"] = json!("2026-09-30T10:00:00Z");
            // and a fork names the thread it was cut from
            if event["kind"] == "thread_forked" {
                event["data"]["from"]["threadId"] = json!(RANDOM);
            }
            contract.validate_component("Event", &event);
            assert_eq!(event["seq"], n + 1, "{name}: seq runs from 1");
        }
        seen += 1;
    }
    assert!(seen >= 7, "the goldens were not found in {}", dir.display());
}

/// The validator must actually bite: these instances violate the contract on purpose.
mod validator_bites {
    use serde_json::json;

    use super::{Contract, normalize_nullable};

    fn good_thread() -> serde_json::Value {
        json!({
            "id": "0190aaaa-0000-7000-8000-000000000123",
            "owner": "alice@example.com",
            "title": "t",
            "target": {"agentId": "a"},
            "state": "working",
            "createdAt": "2026-09-29T10:00:00Z",
            "updatedAt": "2026-09-29T10:00:00Z",
            "lastSeq": 1
        })
    }

    #[test]
    fn a_valid_thread_passes() {
        Contract::load().validate_component("Thread", &good_thread());
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn unknown_thread_state_is_rejected() {
        let mut t = good_thread();
        t["state"] = json!("canceled");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn non_uuid_id_is_rejected() {
        let mut t = good_thread();
        t["id"] = json!("123");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn bad_timestamp_is_rejected() {
        let mut t = good_thread();
        t["createdAt"] = json!("yesterday");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn missing_required_field_is_rejected() {
        let mut t = good_thread();
        t.as_object_mut().unwrap().remove("lastSeq");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn null_for_an_optional_field_is_rejected() {
        let mut t = good_thread();
        t["target"]["release"] = json!(null);
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn event_with_a_bad_kind_is_rejected() {
        Contract::load().validate_component(
            "Event",
            &json!({
                "seq": 1, "threadId": "0190aaaa-0000-7000-8000-000000000123",
                "at": "2026-09-29T10:00:00Z", "kind": "banana",
                "actor": {"type": "user", "name": "a"}, "data": {}
            }),
        );
    }

    #[test]
    fn nullable_is_normalised() {
        let mut v = json!({"type": "string", "nullable": true});
        normalize_nullable(&mut v);
        assert_eq!(v, json!({"type": ["string", "null"]}));
    }
}
