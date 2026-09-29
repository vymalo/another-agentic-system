//! Shared harness: the real chat API, dispatcher, in-memory store and A2A adapter, talking to
//! in-process fake A2A agents over real HTTP.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig};
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, UuidV7Ids};
use orch_testsupport::{
    Chat, FakeAgent, FakeAgentOptions, FakeReleases, TestInstance, fast_dispatcher,
};
use serde_json::Value;

#[allow(unused_imports)]
pub use orch_testsupport::{eventually, shape};

pub type Stack = PortSet<MemoryStore, MemoryWakeup, A2aAgentClient, SystemClock, UuidV7Ids>;

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";

/// The five events of a plain successful run.
pub const FIVE: [&str; 5] = [
    "user_message",
    "agent_status:working",
    "artifact",
    "agent_status:completed",
    "thread_state:done",
];

/// How to set up the two agents (`coder` offers release channels, `plain` does not).
pub struct Setup {
    pub coder: FakeAgentOptions,
    pub plain: FakeAgentOptions,
    /// Bearer tokens the orchestrator is configured with.
    pub coder_token: Option<String>,
    pub plain_token: Option<String>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            coder: FakeAgentOptions {
                releases: Some(FakeReleases::sample()),
                ..FakeAgentOptions::default()
            },
            plain: FakeAgentOptions::default(),
            coder_token: None,
            plain_token: None,
        }
    }
}

/// One "database" (store + wakeup) and the agents, shared by every orchestrator instance.
pub struct World {
    pub store: MemoryStore,
    pub wakeup: MemoryWakeup,
    pub agents: A2aAgentClient,
    pub coder: FakeAgent,
    pub plain: FakeAgent,
    coder_token: Option<String>,
    plain_token: Option<String>,
}

impl World {
    pub async fn start() -> Self {
        Self::with(Setup::default()).await
    }

    pub async fn with(setup: Setup) -> Self {
        World {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agents: A2aAgentClient::new(A2aConfig {
                use_system_proxy: false,
                ..A2aConfig::default()
            })
            .unwrap(),
            coder: FakeAgent::spawn(setup.coder).await,
            plain: FakeAgent::spawn(setup.plain).await,
            coder_token: setup.coder_token,
            plain_token: setup.plain_token,
        }
    }

    fn directory(&self) -> AgentDirectory {
        let entry = |name: &str, endpoint: AgentEndpoint| AgentEntry {
            endpoint,
            name: name.to_owned(),
        };
        AgentDirectory::new(vec![
            entry(
                "Coder",
                self.coder.endpoint("coder", self.coder_token.as_deref()),
            ),
            entry(
                "Plain",
                self.plain.endpoint("plain", self.plain_token.as_deref()),
            ),
        ])
    }

    /// A new orchestrator process (app state is per process; the store is shared).
    pub fn app(&self) -> Arc<App<Stack>> {
        Arc::new(App::new(
            PortSet {
                store: self.store.clone(),
                wakeup: self.wakeup.clone(),
                agents: self.agents.clone(),
                clock: SystemClock,
                ids: UuidV7Ids,
            },
            self.directory(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        ))
    }

    /// Starts an orchestrator instance (API + dispatcher) named `owner`.
    pub async fn instance(&self, owner: &str) -> TestInstance {
        TestInstance::spawn(
            self.app(),
            ApiConfig {
                sse_keepalive: Duration::from_millis(150),
                ..ApiConfig::default()
            },
            fast_dispatcher(),
            owner,
        )
        .await
    }

    pub fn chat(&self, instance: &TestInstance) -> Chat {
        Chat::new(&instance.base_url, ALICE)
    }
}

/// Asserts `seq` is exactly 1..=n.
pub fn assert_contiguous(events: &[Value]) {
    let seqs: Vec<i64> = events.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    let want: Vec<i64> = (1..=i64::try_from(events.len()).unwrap()).collect();
    assert_eq!(seqs, want, "seq must be contiguous from 1");
}

/// Every agent-produced event of `kind`, its `data` field.
pub fn data_of<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|e| e["kind"] == kind)
        .map(|e| &e["data"])
        .collect()
}
