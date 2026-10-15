//! A rig for the tests that put the real pieces together: the real credential broker over the
//! in-memory stores, the real MCP client (guarded) against real servers on the loopback, the real
//! router with the resource API, the AG-UI routes and the thread-tools endpoint, a dispatcher and a
//! scripted agent that works until it is cancelled. The test plays the agent with an MCP client.
//!
//! A person's servers are added through the API, with the secret in the request body, as the web
//! does; the broker keeps the credential sealed in its store; the relay leases it for each request.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig, Inbound, NewThread,
    Permission, Policy, RoleGrant,
};
use orch_core::{AgentId, AgentTarget, ThreadId, ThreadState, UserId};
use orch_credential_broker::Broker;
use orch_ports::memory::{
    MemoryPendingAuthorizations, MemorySecretStore, MemoryStore, MemoryUserToolServers,
    MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, EgressPolicy, FixedRegistry, IdGen, NoArtifacts, NoModel, PortSet, Role,
    SystemClock, UserTools,
};
use orch_surface_thread_tools::{RelayTools, ThreadToolsConfig};
use orch_tools_mcp::McpToolClient;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{ALICE, Client, connect, grant_token, keys};

pub type WireBundle = UserTools<
    MemoryUserToolServers,
    Broker<MemorySecretStore, MemoryUserToolServers, SystemClock>,
    MemoryPendingAuthorizations,
>;

pub type WireStack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    orch_auth_header::HeaderAuth,
    NoArtifacts,
    WireBundle,
>;

pub struct WireRig {
    pub base: String,
    pub http: reqwest::Client,
    pub app: Arc<App<WireStack>>,
    pub ids: SeqIds,
    pub servers: MemoryUserToolServers,
    pub secrets: MemorySecretStore,
    server: JoinHandle<()>,
    stop: CancellationToken,
}

impl Drop for WireRig {
    fn drop(&mut self) {
        self.stop.cancel();
        self.server.abort();
    }
}

/// Everybody who signs in may use servers of their own here.
fn policy() -> Policy {
    let mut grant = RoleGrant::user();
    grant.permissions.insert(Permission::ToolsUserAdd);
    Policy::new(
        BTreeMap::from([(Role::new("user"), grant)]),
        Some(Role::new("user")),
    )
    .unwrap()
}

fn directory() -> AgentDirectory {
    let entry = |id: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    };
    AgentDirectory::new(vec![entry("plain")])
}

fn dispatcher_config() -> DispatcherConfig {
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

impl WireRig {
    /// The rig: `egress` is the policy of the deployment (the broker's, the bundle's, the picker's),
    /// and `client` is what the relay calls people's servers with.
    pub async fn start(egress: EgressPolicy, client: McpToolClient) -> WireRig {
        let servers = MemoryUserToolServers::new();
        let secrets = MemorySecretStore::new();
        let broker = Broker::builder(secrets.clone(), servers.clone(), egress.clone())
            .build()
            .unwrap();
        let bundle = WireBundle::full(
            egress,
            servers.clone(),
            broker,
            MemoryPendingAuthorizations::new(),
        );
        let ids = SeqIds::default();
        let app = Arc::new(
            App::new(
                PortSet {
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: ids.clone(),
                    model: NoModel,
                    auth: orch_auth_header::HeaderAuth::new(),
                    registry: directory().fixed_registry(),
                    artifacts: NoArtifacts,
                    user_tools: bundle,
                },
                directory(),
                AppConfig {
                    policy: policy(),
                    stream_poll: Duration::from_millis(100),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        // the deployment lists no server: the relay serves people's, through the guarded client
        let relay = RelayTools::new(Arc::clone(&app), McpToolClient::new().unwrap(), Vec::new())
            .unwrap()
            .with_user_servers(client);
        let config = ThreadToolsConfig::new(keys(), ["127.0.0.1", "localhost"])
            .unwrap()
            .with_provider(relay);
        let stop = CancellationToken::new();
        tokio::spawn(
            Dispatcher::new(Arc::clone(&app), dispatcher_config(), "wire").run(stop.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let sse_keepalive = ApiConfig::default().sse_keepalive;
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            ApiConfig::default(),
            vec![
                orch_surface_agui::routes(Arc::clone(&app), sse_keepalive),
                orch_surface_thread_tools::routes(Arc::clone(&app), config),
            ],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        WireRig {
            base: format!("http://{addr}"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            ids,
            servers,
            secrets,
            server,
            stop,
        }
    }

    pub fn alice() -> UserId {
        UserId::new(ALICE)
    }

    /// `method path` as Alice, with an optional JSON body: `(status, the body as text)`.
    pub async fn api(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String) {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("X-Auth-Request-Email", ALICE);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.text().await.unwrap())
    }

    /// Adds a server of Alice's through the API and returns its id.
    pub async fn add(&self, body: Value) -> String {
        let (status, text) = self
            .api(reqwest::Method::POST, "/api/user-tool-servers", Some(body))
            .await;
        assert_eq!(status, 201, "{text}");
        serde_json::from_str::<Value>(&text).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// A thread of Alice's with `tools` attached, whose agent is at work.
    pub async fn working_thread(&self, tools: &[&str]) -> ThreadId {
        let id = ThreadId(self.ids.new_id());
        self.app
            .create_thread_as(
                &Self::alice(),
                id,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "slow work".to_owned(),
                },
                Inbound {
                    tools: tools.iter().map(|t| (*t).to_owned()).collect(),
                    ..Inbound::default()
                },
            )
            .await
            .unwrap();
        orch_testsupport::eventually("the agent is working", || async {
            let thread = self.app.get_thread(&Self::alice(), id).await.unwrap();
            (thread.state == ThreadState::Working).then_some(())
        })
        .await;
        id
    }

    /// The agent's MCP client for the thread's endpoint.
    pub async fn client(&self, thread: ThreadId) -> Client {
        connect(
            &format!("{}/thread-tools/{thread}/mcp", self.base),
            &grant_token(thread, "plain"),
        )
        .await
    }
}
