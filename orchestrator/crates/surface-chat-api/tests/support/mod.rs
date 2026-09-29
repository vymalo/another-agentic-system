//! Test harness: the real router on a real TCP port over the in-memory stack.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

pub mod contract;
pub mod sse;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use orch_api::{ApiConfig, AuthConfig};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_core::{AgentId, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds, sample_releases};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub use sse::{Item, SseClient};

pub type Ports = PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds>;
pub type TestApp = App<Ports>;

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";

pub struct Resp {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
}

pub struct Harness {
    pub base: String,
    pub client: reqwest::Client,
    pub agent: ScriptedAgent,
    pub store: MemoryStore,
    pub app: Arc<TestApp>,
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

impl Harness {
    pub async fn start() -> Self {
        Self::start_with(ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        })
        .await
    }

    pub async fn start_with(api: ApiConfig) -> Self {
        let store = MemoryStore::new();
        let agent = ScriptedAgent::new().with_releases("coder", sample_releases());
        let entry = |id: &str, name: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
        };
        let app = Arc::new(App::new(
            PortSet {
                store: store.clone(),
                wakeup: MemoryWakeup::new(),
                agents: agent.clone(),
                clock: SystemClock,
                ids: SeqIds::default(),
            },
            AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")]),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        ));
        let token = CancellationToken::new();
        let dispatcher = tokio::spawn(
            Dispatcher::new(Arc::clone(&app), fast_dispatcher(), "test-dispatcher")
                .run(token.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let keepalive = api.sse_keepalive;
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![orch_surface_chat_api::routes(Arc::clone(&app), keepalive)],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            agent,
            store,
            app,
            server,
            dispatcher,
            token,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        user: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut req = self.client.request(method, self.url(path));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        Resp {
            status,
            content_type,
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    pub async fn get(&self, path: &str, user: Option<&str>) -> Resp {
        self.send(reqwest::Method::GET, path, user, None).await
    }

    pub async fn post(&self, path: &str, user: Option<&str>, body: Value) -> Resp {
        self.send(reqwest::Method::POST, path, user, Some(body))
            .await
    }

    pub async fn post_empty(&self, path: &str, user: Option<&str>) -> Resp {
        self.send(reqwest::Method::POST, path, user, None).await
    }

    /// Creates a thread and returns its id.
    pub async fn create(&self, user: &str, agent: &str, text: &str) -> String {
        let r = self
            .post(
                "/api/threads",
                Some(user),
                json!({"target": {"agentId": agent}, "text": text}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
        r.json()["id"].as_str().unwrap().to_owned()
    }

    pub async fn state(&self, user: &str, id: &str) -> String {
        let r = self.get(&format!("/api/threads/{id}"), Some(user)).await;
        r.json()["state"].as_str().unwrap().to_owned()
    }

    pub async fn wait_state(&self, user: &str, id: &str, want: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let got = self.state(user, id).await;
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

    pub async fn events(&self, user: &str, id: &str) -> Vec<Value> {
        let r = self
            .get(&format!("/api/threads/{id}/events"), Some(user))
            .await;
        assert_eq!(r.status, 200);
        r.json().as_array().unwrap().clone()
    }

    pub async fn stream(&self, user: &str, id: &str, last_event_id: Option<&str>) -> SseClient {
        let mut req = self
            .client
            .get(self.url(&format!("/api/threads/{id}/stream")))
            .header("X-Auth-Request-Email", user);
        if let Some(l) = last_event_id {
            req = req.header("Last-Event-ID", l);
        }
        SseClient::from_response(req.send().await.unwrap())
    }
}

pub fn shape(events: &[Value]) -> Vec<String> {
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

pub fn user(s: &str) -> UserId {
    UserId::new(s)
}

pub fn dev_config(dev_user: Option<&str>) -> ApiConfig {
    ApiConfig {
        auth: AuthConfig {
            dev_user: dev_user.map(UserId::new),
        },
        sse_keepalive: Duration::from_millis(150),
        ..ApiConfig::default()
    }
}

/// Ties the contract to real responses.
pub struct Conformance {
    pub contract: contract::Contract,
    exercised: BTreeSet<String>,
    /// `(operation, status)` for responses the contract does not document.
    pub undocumented: BTreeSet<(String, u16)>,
}

impl Conformance {
    pub fn new() -> Self {
        Conformance {
            contract: contract::Contract::load(),
            exercised: BTreeSet::new(),
            undocumented: BTreeSet::new(),
        }
    }

    /// Checks a response of `operation`: the status is documented (or, if not, at least a
    /// well-formed problem), the media type matches, and the body validates against the schema.
    pub fn check(&mut self, operation: &str, resp: &Resp) {
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

    pub fn assert_all_operations_exercised(&self) {
        assert_eq!(self.exercised, self.contract.operation_ids());
    }
}
