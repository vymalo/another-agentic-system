//! Shared harness: in-memory stack with a scripted agent and fast dispatcher timings.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig, NewThread,
};
use orch_core::{
    AgentId, AgentTarget, Event, EventKind, ThreadId, ThreadRecord, ThreadState, UserId,
};
use orch_ports::memory::{
    MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds, sample_releases,
};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, ThreadStore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub type Ports =
    PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds, ScriptedModel>;
pub type TestApp = App<Ports>;

pub fn alice() -> UserId {
    UserId::new("alice@example.com")
}

pub fn bob() -> UserId {
    UserId::new("bob@example.com")
}

pub fn directory() -> AgentDirectory {
    let entry = |id: &str, name: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
    };
    AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")])
}

/// One "database": the store, wakeup, ids and scripted agent shared by every app instance.
#[derive(Clone)]
pub struct World {
    pub store: MemoryStore,
    pub wakeup: MemoryWakeup,
    pub agent: ScriptedAgent,
    pub ids: SeqIds,
    /// The model of the utility tasks: scripted by a test, asked only for a task that
    /// `AppConfig::tasks` has, at the endpoint `default` (the only one it holds).
    pub model: ScriptedModel,
}

impl World {
    pub fn new() -> Self {
        World {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agent: ScriptedAgent::new().with_releases("coder", sample_releases()),
            ids: SeqIds::default(),
            model: ScriptedModel::default().with_endpoints(["default"]),
        }
    }

    pub fn app(&self) -> Arc<TestApp> {
        self.app_with(AppConfig {
            stream_poll: Duration::from_millis(100),
            ..AppConfig::default()
        })
    }

    pub fn app_with(&self, cfg: AppConfig) -> Arc<TestApp> {
        Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: self.store.clone(),
                    wakeup: self.wakeup.clone(),
                    agents: self.agent.clone(),
                    clock: SystemClock,
                    ids: self.ids.clone(),
                    model: self.model.clone(),
                    auth: orch_ports::RefuseAll,
                    registry: directory().fixed_registry(),
                },
                directory(),
                cfg,
            )
            .expect("a valid gate"),
        )
    }
}

pub fn fast() -> DispatcherConfig {
    DispatcherConfig {
        concurrency: 8,
        lease: Duration::from_millis(400),
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

pub struct Running {
    pub handle: JoinHandle<()>,
    pub token: CancellationToken,
}

impl Running {
    /// Simulates a crash: drops the dispatcher future, and with it every worker.
    pub fn kill(&self) {
        self.handle.abort();
    }

    pub async fn shutdown(self) {
        self.token.cancel();
        self.handle.await.unwrap();
    }
}

pub fn spawn_dispatcher(app: &Arc<TestApp>, cfg: DispatcherConfig, owner: &str) -> Running {
    let token = CancellationToken::new();
    let d = Dispatcher::new(Arc::clone(app), cfg, owner);
    let handle = tokio::spawn(d.run(token.clone()));
    Running { handle, token }
}

pub fn target(agent: &str) -> AgentTarget {
    AgentTarget {
        agent_id: AgentId::new(agent),
        release: None,
    }
}

pub async fn create(app: &TestApp, user: &UserId, agent: &str, text: &str) -> ThreadRecord {
    app.create_thread(
        user,
        NewThread {
            title: None,
            target: target(agent),
            text: text.to_owned(),
        },
    )
    .await
    .unwrap()
}

/// Polls `f` until it returns `Some`, or panics after 10 s.
pub async fn eventually<T, Fut: Future<Output = Option<T>>>(
    what: &str,
    mut f: impl FnMut() -> Fut,
) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub async fn wait_state(
    app: &TestApp,
    user: &UserId,
    id: ThreadId,
    want: ThreadState,
) -> ThreadRecord {
    eventually(&format!("state {want:?}"), || async {
        let t = app.get_thread(user, id).await.unwrap();
        (t.state == want).then_some(t)
    })
    .await
}

pub async fn events(app: &TestApp, user: &UserId, id: ThreadId) -> Vec<Event> {
    app.list_events(user, id, 0, 500).await.unwrap()
}

pub fn kinds(events: &[Event]) -> Vec<EventKind> {
    events.iter().map(Event::kind).collect()
}

/// A compact description of the events, e.g. `user_message`, `agent_status:working`.
pub fn shape(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            let v = serde_json::to_value(e).unwrap();
            let kind = v["kind"].as_str().unwrap().to_owned();
            let data = &v["data"];
            match kind.as_str() {
                "agent_status" => format!("{kind}:{}", data["status"].as_str().unwrap()),
                "thread_state" => format!("{kind}:{}", data["state"].as_str().unwrap()),
                _ => kind,
            }
        })
        .collect()
}

pub fn assert_contiguous(events: &[Event]) {
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    let want: Vec<i64> = (1..=i64::try_from(events.len()).unwrap()).collect();
    assert_eq!(seqs, want, "seq must be contiguous from 1");
}

pub async fn state_of(world: &World, id: ThreadId) -> ThreadState {
    world
        .store
        .get_thread(None, id)
        .await
        .unwrap()
        .unwrap()
        .state
}
