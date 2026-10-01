//! A running router with one webhook surface, on the in-memory store and a clock the test holds.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_api::{ApiConfig, SurfaceRoutes, router_with_surfaces};
use orch_app::{AgentDirectory, App, AppConfig};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{InboxItem, PortSet, ThreadStore};

/// 2026-09-30T12:00:00Z, the time the clock starts at.
pub const NOW: i64 = 1_790_769_600;

pub type Ports =
    PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds, orch_ports::NoModel>;

pub struct Rig {
    pub base: String,
    pub store: MemoryStore,
    pub clock: FixedClock,
    pub client: reqwest::Client,
}

impl Rig {
    /// Serves the router of `routes(app)` (plus the resource API) on a free port.
    pub async fn start(routes: impl FnOnce(Arc<App<Ports>>) -> SurfaceRoutes) -> Rig {
        let store = MemoryStore::new();
        let clock = FixedClock::new(Timestamp::from_second(NOW).unwrap());
        let app: Arc<App<Ports>> = Arc::new(
            App::new(
                PortSet {
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: clock.clone(),
                    ids: SeqIds::default(),
                    model: orch_ports::NoModel,
                },
                AgentDirectory::new(Vec::new()),
                AppConfig::default(),
            )
            .unwrap(),
        );
        let router =
            router_with_surfaces(Arc::clone(&app), ApiConfig::default(), vec![routes(app)]);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await });
        Rig {
            base,
            store,
            clock,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    pub fn now(&self) -> Timestamp {
        orch_ports::Clock::now(&self.clock)
    }

    /// Whether the store holds any inbox row at all (a probe claim finds every due row).
    pub async fn holds_a_row(&self) -> bool {
        !self
            .store
            .claim_inbox("probe", self.now(), Duration::from_secs(1), 10)
            .await
            .unwrap()
            .is_empty()
    }

    /// How many rows the store holds (claims them all).
    pub async fn rows(&self) -> usize {
        self.store
            .claim_inbox("probe", self.now(), Duration::from_secs(1), 100)
            .await
            .unwrap()
            .len()
    }

    pub async fn find(&self, source: &str, key: &str) -> Option<InboxItem> {
        self.store.find_inbox(source, key).await.unwrap()
    }
}
