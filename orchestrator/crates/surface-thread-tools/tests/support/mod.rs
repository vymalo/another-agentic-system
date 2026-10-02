//! Test harness: the real router on a real TCP port over the in-memory stack with the
//! thread-tools surface mounted, tokens minted the way the A2A adapter mints them, and an
//! in-process rmcp client that speaks to the endpoint over streamable HTTP.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig, Inbound, NewThread,
};
use orch_core::{
    AgentId, AgentTarget, Caller, Input, Origin, ThreadId, ThreadState, ToolsGrant, UiCatalogData,
    UserId, catalog_digest,
};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, IdGen, PortSet, SystemClock};
use orch_surface_thread_tools::{ThreadToolProvider, ThreadToolsConfig, ToolCtx};
use orch_thread_token::{Claims, ThreadToolsKeys, mint};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, JsonObject, Tool};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ErrorData, RoleClient, ServiceExt};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub type Ports = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    orch_auth_header::HeaderAuth,
>;
pub type Client = RunningService<RoleClient, ()>;

pub const ALICE: &str = "alice@example.com";
pub const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub const OLD_KEY: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
pub const OTHER_KEY: &str = "aaaabbbbccccddddeeeeffff0000111122223333444455556666777788889999";

pub fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

pub fn keys() -> ThreadToolsKeys {
    ThreadToolsKeys::new(secret(KEY), None).unwrap()
}

pub struct Harness {
    pub base: String,
    pub http: reqwest::Client,
    pub app: Arc<App<Ports>>,
    pub ids: SeqIds,
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
    pub async fn start() -> Self {
        Self::start_with(keys(), |config| config).await
    }

    /// The surface over `keys`, shaped by `shape` (providers, timeouts).
    pub async fn start_with(
        keys: ThreadToolsKeys,
        shape: impl FnOnce(ThreadToolsConfig) -> ThreadToolsConfig,
    ) -> Self {
        Self::start_full(keys, AppConfig::default(), |_, config| shape(config)).await
    }

    /// The surface over `keys` on an application with `cfg`, shaped by `shape`, which is given the
    /// application (a provider that records steps needs it).
    pub async fn start_full(
        keys: ThreadToolsKeys,
        cfg: AppConfig,
        shape: impl FnOnce(&Arc<App<Ports>>, ThreadToolsConfig) -> ThreadToolsConfig,
    ) -> Self {
        let entry = |id: &str, name: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
        };
        let ids = SeqIds::default();
        let directory = AgentDirectory::new(vec![entry("plain", "Plain"), entry("coder", "Coder")]);
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: ids.clone(),
                    model: orch_ports::NoModel,
                    auth: orch_auth_header::HeaderAuth::new(),
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    ..cfg
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
        let config = shape(
            &app,
            ThreadToolsConfig::new(keys, ["127.0.0.1", "localhost"]).unwrap(),
        );
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            ApiConfig::default(),
            vec![orch_surface_thread_tools::routes(Arc::clone(&app), config)],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            ids,
            server,
            dispatcher,
            token,
        }
    }

    /// The endpoint of `thread`.
    pub fn url(&self, thread: ThreadId) -> String {
        format!("{}/thread-tools/{thread}/mcp", self.base)
    }

    /// A thread of Alice's, addressed to `agent`, that has run to the end (`done`).
    pub async fn thread(&self, agent: &str) -> ThreadId {
        let id = ThreadId(self.ids.new_id());
        self.app
            .create_thread_as(
                &UserId::new(ALICE),
                id,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: None,
                    },
                    text: "echo hi".to_owned(),
                },
                Inbound::default(),
            )
            .await
            .unwrap();
        self.wait_done(id).await;
        id
    }

    /// A thread of Alice's, addressed to `agent`, whose agent is at work (`working`) until the
    /// thread is cancelled: a turn that is going on.
    pub async fn working_thread(&self, agent: &str) -> ThreadId {
        let id = ThreadId(self.ids.new_id());
        self.app
            .create_thread_as(
                &UserId::new(ALICE),
                id,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: None,
                    },
                    text: "slow work".to_owned(),
                },
                Inbound::default(),
            )
            .await
            .unwrap();
        orch_testsupport::eventually("the agent is working", || async {
            let thread = self.app.get_thread(&UserId::new(ALICE), id).await.unwrap();
            (thread.state == ThreadState::Working).then_some(())
        })
        .await;
        id
    }

    /// The thread's events, oldest first.
    pub async fn events(&self, id: ThreadId) -> Vec<orch_core::Event> {
        self.app
            .list_events(&UserId::new(ALICE), id, 0, 500)
            .await
            .unwrap()
    }

    pub async fn wait_done(&self, id: ThreadId) {
        orch_testsupport::eventually("the thread is done", || async {
            let thread = self.app.get_thread(&UserId::new(ALICE), id).await.unwrap();
            (thread.state == ThreadState::Done).then_some(())
        })
        .await;
    }

    /// Alice sends a message to `thread` that carries `catalog`, and the agent finishes it.
    pub async fn show(&self, thread: ThreadId, catalog: &UiCatalogData) {
        let alice = UserId::new(ALICE);
        let before = self
            .app
            .get_thread(&alice, thread)
            .await
            .unwrap()
            .job
            .number;
        self.app
            .submit(
                &alice,
                thread,
                Input::UserMessage {
                    user: alice.clone(),
                    text: "echo again".to_owned(),
                    message_id: None,
                    run_id: None,
                    origin: Origin::Agui,
                    catalog: Some(catalog.clone()),
                },
                None,
            )
            .await
            .unwrap();
        orch_testsupport::eventually("the next job is done", || async {
            let t = self.app.get_thread(&alice, thread).await.unwrap();
            (t.job.number > before && t.state == ThreadState::Done).then_some(())
        })
        .await;
    }

    /// One JSON-RPC `POST` to `url` as a bare HTTP client, with the given extra headers.
    pub async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> reqwest::Response {
        let mut req = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        req.send().await.unwrap()
    }
}

/// A version of the screen's catalog with its real digest; `tag` tells two of a version apart.
pub fn catalog(version: u32, tag: &str) -> UiCatalogData {
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    let catalog = json!({
        "catalogId": id,
        "components": {"Note": {
            "type": "object",
            "title": format!("{tag}-{version}"),
            "properties": {"component": {"const": "Note"}},
        }},
    });
    UiCatalogData {
        catalog_id: id.to_owned(),
        version,
        digest: catalog_digest(&catalog).unwrap(),
        catalog,
    }
}

pub fn now() -> Timestamp {
    Timestamp::from_second(Timestamp::now().as_second()).unwrap()
}

/// The claims of the thread's addressed agent, valid now for two hours.
pub fn claims(thread: ThreadId, agent: &str) -> Claims {
    let issued_at = now();
    Claims {
        thread,
        job: 1,
        agent: AgentId::new(agent),
        caller: Caller::Main,
        depth: 0,
        message_id: "message-1".to_owned(),
        issued_at,
        expires_at: Timestamp::from_second(issued_at.as_second() + 7200).unwrap(),
    }
}

/// A token for `claims`, signed with `keys`.
pub fn token(keys: &ThreadToolsKeys, claims: &Claims) -> String {
    mint(keys, claims).unwrap().expose_secret().to_owned()
}

/// The token of the thread's addressed agent, as the adapter would mint it.
pub fn grant_token(thread: ThreadId, agent: &str) -> String {
    token(&keys(), &claims(thread, agent))
}

#[allow(dead_code)]
pub fn grant(thread: ThreadId, agent: &str) -> ToolsGrant {
    ToolsGrant::main(thread, 1, AgentId::new(agent))
}

/// A client for `url` with the bearer `token` (the MCP handshake included).
pub async fn connect(url: &str, token: &str) -> Client {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the MCP handshake")
}

/// What a tool call gave back.
#[derive(Debug)]
pub struct Outcome {
    pub is_error: bool,
    pub value: Value,
    pub text: String,
}

fn outcome_of(result: CallToolResult) -> Outcome {
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Outcome {
        is_error: result.is_error == Some(true),
        value: result.structured_content.unwrap_or(Value::Null),
        text,
    }
}

pub async fn try_call(
    client: &Client,
    tool: &str,
    args: Value,
) -> Result<Outcome, rmcp::ServiceError> {
    let Value::Object(map) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(map))
        .await?;
    Ok(outcome_of(result))
}

pub async fn call(client: &Client, tool: &str, args: Value) -> Outcome {
    try_call(client, tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} failed at the protocol level: {e}"))
}

pub async fn tool_names(client: &Client) -> Vec<String> {
    client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// What a [`Probe`] saw of one call.
#[derive(Debug, Clone)]
pub struct Seen {
    pub what: &'static str,
    pub tool: String,
    pub owner: String,
    pub claims: Claims,
    pub meta: Option<serde_json::Map<String, Value>>,
}

/// A provider with one tool `name`: it records what each list and call carried, and answers with
/// the caller and the arguments it was given. `owns` says whether it claims calls of the tools
/// in `also` (names it does not list).
#[derive(Clone)]
pub struct Probe {
    pub name: &'static str,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl Probe {
    pub fn new(name: &'static str) -> Self {
        Probe {
            name,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

impl ThreadToolProvider for Probe {
    async fn list(&self, ctx: &ToolCtx) -> Vec<Tool> {
        self.seen.lock().unwrap().push(Seen {
            what: "list",
            tool: String::new(),
            owner: ctx.owner.to_string(),
            claims: ctx.claims.clone(),
            meta: ctx.meta.clone(),
        });
        let schema: JsonObject = json!({"type": "object"}).as_object().unwrap().clone();
        vec![Tool::new(self.name, "a probe", Arc::new(schema))]
    }

    async fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        if name != self.name {
            return None;
        }
        self.seen.lock().unwrap().push(Seen {
            what: "call",
            tool: name.to_owned(),
            owner: ctx.owner.to_string(),
            claims: ctx.claims.clone(),
            meta: ctx.meta.clone(),
        });
        Some(Ok(CallToolResult::structured(json!({
            "by": self.name,
            "caller": ctx.claims.caller.to_string(),
            "args": args,
        }))))
    }
}
