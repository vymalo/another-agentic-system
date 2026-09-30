//! Test harness: the real router on a real TCP port over the in-memory stack with the MCP surface
//! mounted, and an in-process rmcp client that speaks to it over streamable HTTP.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_api::{ApiConfig, AuthConfig};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_core::{AgentId, GatePolicy, ThreadId, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, ThreadStore};
use orch_surface_mcp::{McpConfig, TokenTable};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, ProgressNotificationParam};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, Peer, RoleClient, ServiceExt};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub type Ports = PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds>;
pub type Client = RunningService<RoleClient, Progress>;

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";
pub const ALICE_TOKEN: &str = "alice-token";
pub const BOB_TOKEN: &str = "bob-token";
pub const T: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Options {
    pub gate: GatePolicy,
    pub public_url: Option<&'static str>,
    pub dev_user: Option<&'static str>,
    pub heartbeat: Option<Duration>,
    pub wait_max: Option<Duration>,
}

pub struct Harness {
    pub base: String,
    pub mcp_url: String,
    pub http: reqwest::Client,
    pub store: MemoryStore,
    pub agent: ScriptedAgent,
    pub app: Arc<App<Ports>>,
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

pub fn fast_dispatcher() -> DispatcherConfig {
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
    }
}

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

pub fn tokens() -> TokenTable {
    TokenTable::new([
        (UserId::new(ALICE), secret(ALICE_TOKEN)),
        (UserId::new(BOB), secret(BOB_TOKEN)),
    ])
    .unwrap()
}

impl Harness {
    pub async fn start() -> Self {
        Self::start_with(Options::default()).await
    }

    pub async fn start_with(options: Options) -> Self {
        let store = MemoryStore::new();
        let agent = ScriptedAgent::new();
        let entry = |id: &str, name: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
        };
        let app = Arc::new(
            App::new(
                PortSet {
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: agent.clone(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                },
                AgentDirectory::new(vec![entry("plain", "Plain"), entry("coder", "Coder")]),
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    gate: options.gate,
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
        let mut config = McpConfig::new(tokens(), ["127.0.0.1", "localhost"]).unwrap();
        if let Some(url) = options.public_url {
            config = config.with_public_url(url).unwrap();
        }
        if let Some(heartbeat) = options.heartbeat {
            config = config.with_heartbeat(heartbeat).unwrap();
        }
        if let Some(max) = options.wait_max {
            config = config.with_wait_max(max).unwrap();
        }
        let api = ApiConfig {
            auth: AuthConfig {
                dev_user: options.dev_user.map(UserId::new),
            },
            ..ApiConfig::default()
        };
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![orch_surface_mcp::routes(Arc::clone(&app), config)],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            mcp_url: format!("http://{addr}/mcp"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            store,
            agent,
            app,
            server,
            dispatcher,
            token,
        }
    }

    /// An MCP client (legacy `initialize` handshake) authenticated with `token`.
    pub async fn client(&self, token: &str) -> Client {
        connect(&self.mcp_url, token).await
    }

    /// One JSON-RPC `POST /mcp` as a bare HTTP client, with the given extra headers.
    pub async fn post(&self, headers: &[(&str, &str)], body: &Value) -> reqwest::Response {
        let mut req = self
            .http
            .post(&self.mcp_url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(body);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        req.send().await.unwrap()
    }

    /// The jobs of `user`, newest first.
    pub async fn threads_of(&self, user: &str) -> Vec<orch_core::ThreadRecord> {
        self.store
            .list_threads(&UserId::new(user), None, 100)
            .await
            .unwrap()
    }

    /// Nothing was written for anyone, and no agent was called.
    pub async fn assert_nothing_written(&self) {
        assert!(self.threads_of(ALICE).await.is_empty());
        assert!(self.threads_of(BOB).await.is_empty());
        assert!(self.agent.calls().is_empty(), "{:?}", self.agent.calls());
    }
}

/// A client for `url` with the bearer `token`.
pub async fn connect(url: &str, token: &str) -> Client {
    connect_recording(url, token).await.0
}

/// A client that keeps the progress notifications it receives, as `(progress, message)`.
#[derive(Clone, Default)]
pub struct Progress {
    seen: Arc<Mutex<Vec<(f64, String)>>>,
}

impl ClientHandler for Progress {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.seen
            .lock()
            .unwrap()
            .push((params.progress, params.message.unwrap_or_default()));
    }
}

impl Progress {
    pub fn all(&self) -> Vec<(f64, String)> {
        self.seen.lock().unwrap().clone()
    }

    /// The `seq` of each event notification (`#7 ...`), heartbeats left out.
    pub fn seqs(&self) -> Vec<i64> {
        self.all()
            .iter()
            .filter_map(|(_, m)| m.strip_prefix('#'))
            .filter_map(|m| m.split(' ').next()?.parse().ok())
            .collect()
    }

    pub fn heartbeats(&self) -> usize {
        self.all()
            .iter()
            .filter(|(_, m)| m.starts_with("still waiting"))
            .count()
    }
}

/// A client for `url` that records its progress notifications in the returned [`Progress`].
pub async fn connect_recording(
    url: &str,
    token: &str,
) -> (RunningService<RoleClient, Progress>, Progress) {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    let transport = StreamableHttpClientTransport::from_config(config);
    let progress = Progress::default();
    let client = progress
        .clone()
        .serve(transport)
        .await
        .expect("the MCP handshake");
    (client, progress)
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

/// Calls a tool and returns its result; a protocol error panics (see [`try_call`]).
pub async fn call(client: &Client, tool: &'static str, args: Value) -> Outcome {
    try_call(client, tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} failed at the protocol level: {e}"))
}

pub async fn try_call(
    client: &Client,
    tool: &'static str,
    args: Value,
) -> Result<Outcome, rmcp::ServiceError> {
    let Value::Object(map) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(map))
        .await?;
    Ok(outcome_of(result))
}

/// Calls a tool through a peer, which a spawned task can own (a client cannot be cloned).
pub async fn call_peer(peer: &Peer<RoleClient>, tool: &'static str, args: Value) -> Outcome {
    let Value::Object(map) = args else {
        panic!("arguments must be an object");
    };
    let result = peer
        .call_tool(CallToolRequestParams::new(tool).with_arguments(map))
        .await
        .unwrap_or_else(|e| panic!("{tool} failed at the protocol level: {e}"));
    outcome_of(result)
}

/// A `start_job` that must be accepted; its `job_id`.
pub async fn start(client: &Client, text: &str) -> String {
    let out = call(client, "start_job", json!({ "text": text })).await;
    assert!(!out.is_error, "{out:?}");
    out.value["job_id"].as_str().unwrap().to_owned()
}

pub fn thread(id: &str) -> ThreadId {
    id.parse().unwrap()
}

/// Polls `get_job` until the job is in `state`.
pub async fn wait_state(client: &Client, job: &str, state: &str) -> Value {
    orch_testsupport::eventually(&format!("job {job} is {state}"), || async {
        let out = call(client, "get_job", json!({ "job_id": job })).await;
        (out.value["state"] == state).then_some(out.value)
    })
    .await
}
