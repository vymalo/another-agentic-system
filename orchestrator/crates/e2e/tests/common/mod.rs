//! Shared harness: the real resource API and AG-UI routes, dispatcher, a store (in memory, or Postgres when
//! `ORCH_TEST_DATABASE_URL` is set) and the A2A adapter, talking to in-process fake A2A agents
//! over real HTTP.
//!
//! Every scenario runs as two tests, `<name>::memory` and `<name>::postgres` (see [`backends!`]);
//! the Postgres one is a no-op unless the environment variable is set. Each Postgres test gets
//! its own schema, so they run in parallel against one database.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, ApplyOutcome, GateLayer, GateRules, InboxConfig,
    InboxWorker, NewThread, Received,
};
use orch_core::{
    AgentId, AgentTarget, CiConclusion, CiProvider, CiReport, Event, GatePolicy, Input, ThreadId,
    ThreadRecord, UserId,
};
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{
    AgentEndpoint, InboxItem, InboxLease, InboxPayload, PortSet, Ports, SystemClock, ThreadStore,
    UuidV7Ids, Wakeup,
};
use orch_store_postgres::{PgStore, PgWakeup};
use orch_testsupport::{
    Chat, FakeAgent, FakeAgentOptions, FakeReleases, Frame, TestInstance, VerifierScript,
    fast_dispatcher,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
pub use orch_testsupport::{eventually, shape};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../../store-postgres/tests/support/mod.rs"]
mod pgdb;

pub type Stack<S, W> = PortSet<S, W, A2aAgentClient, SystemClock, UuidV7Ids>;

/// Which store a scenario runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Memory,
    Postgres,
}

/// Whether the Postgres variants can run here (`ORCH_TEST_DATABASE_URL` is set).
pub fn postgres_available() -> bool {
    let available = pgdb::database_url().is_some();
    if !available {
        eprintln!("skipping the Postgres variant: ORCH_TEST_DATABASE_URL is not set");
    }
    available
}

/// Defines, for each scenario `fn name(backend: Backend)`, the tests `name::memory` and
/// `name::postgres`. A scenario nobody lists is dead code, which the lints refuse.
#[allow(unused_macros)] // a test binary may use only one of the two
macro_rules! backends {
    ($($name:ident),+ $(,)?) => {
        $(
            mod $name {
                #[tokio::test]
                async fn memory() {
                    super::$name(crate::common::Backend::Memory).await;
                }

                #[tokio::test]
                async fn postgres() {
                    if crate::common::postgres_available() {
                        super::$name(crate::common::Backend::Postgres).await;
                    }
                }
            }
        )+
    };
}

/// Defines, for each scenario `fn name(backend: Backend)` that needs a real database (separate
/// connections, `LISTEN`/`NOTIFY` across them), the single test `name`, a no-op unless
/// `ORCH_TEST_DATABASE_URL` is set.
#[allow(unused_macros)]
macro_rules! postgres_only {
    ($($name:ident),+ $(,)?) => {
        $(
            mod $name {
                #[tokio::test]
                async fn postgres() {
                    if crate::common::postgres_available() {
                        super::$name(crate::common::Backend::Postgres).await;
                    }
                }
            }
        )+
    };
}

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

/// How to set up the agents (`coder` offers release channels, `plain` does not; `reviewer`, the
/// verifier of ADR 0018, exists only when a script is given).
pub struct Setup {
    pub coder: FakeAgentOptions,
    pub plain: FakeAgentOptions,
    /// The script of the `reviewer` agent, which plays the verifier; `None`: no such agent.
    pub reviewer: Option<VerifierScript>,
    /// Bearer tokens the orchestrator is configured with.
    pub coder_token: Option<String>,
    pub plain_token: Option<String>,
    /// The gate new threads start under (the deployment's; empty: no gate).
    pub gate: GatePolicy,
    /// The `gate` key of an agent's `AGENTS_FILE` entry, by agent id.
    pub target_gates: BTreeMap<AgentId, GateLayer>,
    /// Which sources the instances honour (the build's rules by default; a test of a machinery
    /// underneath a source this build still refuses, such as the verifier, widens it).
    pub gate_rules: GateRules,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            coder: FakeAgentOptions {
                releases: Some(FakeReleases::sample()),
                ..FakeAgentOptions::default()
            },
            plain: FakeAgentOptions::default(),
            reviewer: None,
            coder_token: None,
            plain_token: None,
            gate: GatePolicy::default(),
            target_gates: BTreeMap::new(),
            gate_rules: GateRules::default(),
        }
    }
}

/// `plain` requires the verifier `reviewer` (its `AGENTS_FILE` entry says so), which answers as
/// `script` says: the world of the verifier goldens.
pub fn verified_by_reviewer(script: VerifierScript) -> Setup {
    let gate = GateLayer::from_json(&json!({"require": ["verifier"], "verifier": "reviewer"}))
        .unwrap()
        .unwrap();
    Setup {
        reviewer: Some(script),
        target_gates: BTreeMap::from([(AgentId::new("plain"), gate)]),
        ..Setup::default()
    }
}

/// The "database" every orchestrator instance of a [`World`] shares.
enum Db {
    Memory {
        store: MemoryStore,
        wakeup: MemoryWakeup,
    },
    /// One schema of the test database; every instance opens its own pool and listener on it,
    /// like separate processes would.
    Postgres(pgdb::TestDb),
}

/// One database and the agents, shared by every orchestrator instance.
pub struct World {
    db: Db,
    pub agents: A2aAgentClient,
    pub coder: FakeAgent,
    pub plain: FakeAgent,
    /// The verifier, when the setup has one (agent id `reviewer`).
    pub reviewer: Option<FakeAgent>,
    coder_token: Option<String>,
    plain_token: Option<String>,
    gate: GatePolicy,
    target_gates: BTreeMap<AgentId, GateLayer>,
    gate_rules: GateRules,
}

impl World {
    pub async fn start(backend: Backend) -> Self {
        Self::with(backend, Setup::default()).await
    }

    pub async fn with(backend: Backend, setup: Setup) -> Self {
        let db = match backend {
            Backend::Memory => Db::Memory {
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
            },
            Backend::Postgres => {
                let db = pgdb::TestDb::new()
                    .await
                    .expect("the Postgres variant needs ORCH_TEST_DATABASE_URL");
                db.store().await; // migrate once; instances only connect
                Db::Postgres(db)
            }
        };
        World {
            db,
            agents: A2aAgentClient::new(A2aConfig {
                use_system_proxy: false,
                ..A2aConfig::default()
            })
            .unwrap(),
            coder: FakeAgent::spawn(setup.coder).await,
            plain: FakeAgent::spawn(setup.plain).await,
            reviewer: match setup.reviewer {
                Some(script) => Some(
                    FakeAgent::spawn(FakeAgentOptions {
                        verifier: Some(script),
                        ..FakeAgentOptions::default()
                    })
                    .await,
                ),
                None => None,
            },
            coder_token: setup.coder_token,
            plain_token: setup.plain_token,
            gate: setup.gate,
            target_gates: setup.target_gates,
            gate_rules: setup.gate_rules,
        }
    }

    fn directory(&self) -> AgentDirectory {
        let entry = |name: &str, endpoint: AgentEndpoint| AgentEntry {
            endpoint,
            name: name.to_owned(),
        };
        let mut entries = vec![
            entry(
                "Coder",
                self.coder.endpoint("coder", self.coder_token.as_deref()),
            ),
            entry(
                "Plain",
                self.plain.endpoint("plain", self.plain_token.as_deref()),
            ),
        ];
        if let Some(reviewer) = &self.reviewer {
            entries.push(entry("Reviewer", reviewer.endpoint("reviewer", None)));
        }
        AgentDirectory::new(entries)
    }

    /// A new orchestrator process (app state is per process; the database is shared).
    fn app<S: ThreadStore, W: Wakeup>(&self, store: S, wakeup: W) -> Arc<App<Stack<S, W>>> {
        Arc::new(
            App::new(
                PortSet {
                    store,
                    wakeup,
                    agents: self.agents.clone(),
                    clock: SystemClock,
                    ids: UuidV7Ids,
                },
                self.directory(),
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    gate: self.gate.clone(),
                    target_gates: self.target_gates.clone(),
                    gate_rules: self.gate_rules.clone(),
                    ..AppConfig::default()
                },
            )
            .expect("a valid gate"),
        )
    }

    /// An application instance with no HTTP server and no dispatcher, on the shared database:
    /// what a worker process that only runs the inbox worker looks like, and the handle a test
    /// uses to play the surfaces that do not exist yet (a webhook) and the agent.
    pub async fn node(&self, owner: &str) -> Node {
        match &self.db {
            Db::Memory { store, wakeup } => Node::Memory(self.app(store.clone(), wakeup.clone())),
            Db::Postgres(db) => {
                let pool = db.pool(owner, 8).await;
                let wakeup = PgWakeup::start(pool.clone());
                assert!(
                    wakeup.wait_listening(Duration::from_secs(10)).await,
                    "the wakeup listener did not attach"
                );
                Node::Postgres(self.app(PgStore::from_pool(pool), wakeup))
            }
        }
    }

    /// Starts an orchestrator instance (API + dispatcher) named `owner`.
    pub async fn instance(&self, owner: &str) -> TestInstance {
        self.instance_with(owner, true).await
    }

    /// Starts an instance named `owner`; `dispatch: false` serves the API only.
    pub async fn instance_with(&self, owner: &str, dispatch: bool) -> TestInstance {
        self.instance_full(owner, dispatch, false).await
    }

    /// Like [`World::instance_with`], and the MCP server is mounted at `/mcp` with the tokens
    /// [`ALICE_TOKEN`] and [`BOB_TOKEN`].
    pub async fn instance_with_mcp(&self, owner: &str, dispatch: bool) -> TestInstance {
        self.instance_full(owner, dispatch, true).await
    }

    async fn instance_full(&self, owner: &str, dispatch: bool, mcp: bool) -> TestInstance {
        let dispatcher = dispatch.then(fast_dispatcher);
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        match &self.db {
            Db::Memory { store, wakeup } => {
                let app = self.app(store.clone(), wakeup.clone());
                let extra = mcp_routes(&app, mcp);
                TestInstance::spawn_with_surfaces(app, api, dispatcher, owner, extra).await
            }
            Db::Postgres(db) => {
                let pool = db.pool(owner, 8).await;
                let wakeup = PgWakeup::start(pool.clone());
                assert!(
                    wakeup.wait_listening(Duration::from_secs(10)).await,
                    "the wakeup listener did not attach"
                );
                let app = self.app(PgStore::from_pool(pool), wakeup);
                let extra = mcp_routes(&app, mcp);
                TestInstance::spawn_with_surfaces(app, api, dispatcher, owner, extra).await
            }
        }
    }

    /// A client for `instance` acting as [`ALICE`], which can also read the event log.
    pub fn chat(&self, instance: &TestInstance) -> Chat {
        instance.chat(ALICE)
    }
}

pub const ALICE_TOKEN: &str = "alice-token-0123456789abcdef0123456789";
pub const BOB_TOKEN: &str = "bob-token-0123456789abcdef012345678901";

/// The MCP surface over `app`, when asked for.
fn mcp_routes<P: orch_ports::Ports>(app: &Arc<App<P>>, mcp: bool) -> Vec<orch_api::SurfaceRoutes> {
    if !mcp {
        return Vec::new();
    }
    let secret = |s: &str| secrecy::SecretString::from(s.to_owned());
    let tokens = orch_surface_mcp::TokenTable::new([
        (orch_core::UserId::new(ALICE), secret(ALICE_TOKEN)),
        (orch_core::UserId::new(BOB), secret(BOB_TOKEN)),
    ])
    .unwrap();
    let config = orch_surface_mcp::McpConfig::new(tokens, ["127.0.0.1"]).unwrap();
    vec![orch_surface_mcp::routes(Arc::clone(app), config)]
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

// ---- AG-UI goldens ----------------------------------------------------------------------

/// `docs/api/examples/agui`.
pub fn examples_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples/agui")
}

/// `{"id"?, "event"}` per frame, the thread id as a placeholder, like the projection's goldens.
pub fn render(responses: &[Vec<Frame>], thread: &str) -> String {
    fn placeholder(v: &mut Value, thread: &str) {
        match v {
            Value::String(s) if s == thread => *s = "<thread-id>".to_owned(),
            Value::Array(items) => items.iter_mut().for_each(|i| placeholder(i, thread)),
            Value::Object(map) => map.values_mut().for_each(|i| placeholder(i, thread)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    let mut value = Value::Array(
        responses
            .iter()
            .flatten()
            .map(|f| {
                let mut frame = serde_json::Map::new();
                if let Some(id) = f.id {
                    frame.insert("id".to_owned(), json!(id));
                }
                frame.insert("event".to_owned(), f.event.clone());
                Value::Object(frame)
            })
            .collect(),
    );
    placeholder(&mut value, thread);
    pretty(&value)
}

/// Pretty JSON with a trailing newline, the way the goldens are stored.
pub fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

/// Compares `text` with the golden `path`, or rewrites it when `UPDATE_GOLDEN=1`. Returns what
/// is stale, if anything.
pub fn check_golden(path: &std::path::Path, text: &str) -> Option<String> {
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(want) if want == text => None,
        Ok(want) => Some(format!(
            "{}: differs\n--- want\n{want}--- got\n{text}",
            path.display()
        )),
        Err(e) => Some(format!("{}: {e}", path.display())),
    }
}

// ---- an application instance without HTTP --------------------------------------------------

type MemoryApp = App<Stack<MemoryStore, MemoryWakeup>>;
type PgApp = App<Stack<PgStore, PgWakeup>>;

/// The application of one orchestrator process, whichever store it runs on.
pub enum Node {
    Memory(Arc<MemoryApp>),
    Postgres(Arc<PgApp>),
}

macro_rules! on_node {
    ($node:expr, $app:ident => $body:expr) => {
        match $node {
            Node::Memory($app) => $body,
            Node::Postgres($app) => $body,
        }
    };
}

/// A running inbox worker.
pub struct InboxRun {
    handle: Option<JoinHandle<()>>,
    token: CancellationToken,
}

impl InboxRun {
    /// The process dies: the future is dropped, nothing is released.
    pub fn kill(&self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }

    /// The process is told to stop and does, releasing its claims.
    pub async fn shutdown(mut self) {
        self.token.cancel();
        if let Some(handle) = self.handle.take() {
            handle.await.unwrap();
        }
    }
}

impl Drop for InboxRun {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Inbox timings that make leases, polls and retries happen in milliseconds.
pub fn fast_inbox() -> InboxConfig {
    InboxConfig {
        lease: Duration::from_millis(800),
        poll_interval: Duration::from_millis(50),
        backoff_base: Duration::from_millis(30),
        backoff_max: Duration::from_millis(200),
        ..InboxConfig::default()
    }
}

impl Node {
    pub fn alice() -> UserId {
        UserId::new(ALICE)
    }

    /// Starts an inbox worker named `owner` on this instance's application.
    pub fn spawn_inbox(&self, cfg: InboxConfig, owner: &str) -> InboxRun {
        on_node!(self, app => {
            let token = CancellationToken::new();
            let worker = InboxWorker::new(Arc::clone(app), cfg, owner);
            InboxRun { handle: Some(tokio::spawn(worker.run(token.clone()))), token }
        })
    }

    /// Creates a thread for Alice on `agent`; no dispatcher of this node delegates it.
    pub async fn create_thread(&self, agent: &str, text: &str) -> ThreadRecord {
        let req = NewThread {
            title: None,
            target: AgentTarget {
                agent_id: AgentId::new(agent),
                release: None,
            },
            text: text.to_owned(),
        };
        on_node!(self, app => app.create_thread(&Self::alice(), req).await.unwrap())
    }

    /// Plays the agent: applies what it would report.
    pub async fn apply(&self, thread: ThreadId, input: Input) {
        let outcome =
            on_node!(self, app => app.apply(thread, input, None, None, None).await.unwrap());
        assert!(
            matches!(outcome, ApplyOutcome::Applied { .. }),
            "{outcome:?}"
        );
    }

    /// Plays a webhook surface: stores a CI report as `source` under the sender's delivery id.
    pub async fn receive(&self, source: &str, delivery: &str, report: CiReport) -> Received {
        on_node!(self, app => app
            .receive(source, delivery, InboxPayload::CiReport(report))
            .await
            .unwrap())
    }

    pub async fn thread(&self, id: ThreadId) -> ThreadRecord {
        on_node!(self, app => app.get_thread(&Self::alice(), id).await.unwrap())
    }

    pub async fn events(&self, id: ThreadId) -> Vec<Event> {
        on_node!(self, app => app.list_events(&Self::alice(), id, 0, 500).await.unwrap())
    }

    /// A worker that claims one due row and then dies: the claim, for a late write to be tried
    /// against.
    pub async fn claim_and_die(&self, owner: &str, lease: Duration) -> InboxLease {
        let claimed = on_node!(self, app => {
            let now = orch_ports::Clock::now(app.ports().clock());
            app.ports().store().claim_inbox(owner, now, lease, 1).await.unwrap()
        });
        assert_eq!(claimed.len(), 1, "a row was due");
        claimed[0].lease().unwrap()
    }

    /// A worker that woke up after its claim was taken over and applies what it read.
    pub async fn apply_from_inbox_late(
        &self,
        thread: ThreadId,
        input: Input,
        lease: &InboxLease,
    ) -> ApplyOutcome {
        on_node!(self, app => app.apply_from_inbox(thread, input, lease).await.unwrap())
    }

    pub async fn inbox_row(&self, source: &str, key: &str) -> Option<InboxItem> {
        on_node!(self, app => app.ports().store().find_inbox(source, key).await.unwrap())
    }

    /// The thread's `pending` and `inflight` outbox rows, oldest first.
    pub async fn open_outbox(&self, thread: ThreadId) -> Vec<orch_ports::OutboxItem> {
        on_node!(self, app => app.ports().store().list_open_outbox(thread).await.unwrap())
    }

    pub async fn watch(&self, key: &str) -> Option<ThreadId> {
        on_node!(self, app => app.ports().store().get_watch(key).await.unwrap())
    }
}

// ---- the `ci` golden ---------------------------------------------------------------------

/// The report of the `ci` golden: check `build` of the commit the fake agent's `verify-ci`
/// script pushes in `attempt`, in the repository it reports.
pub fn golden_ci_report(attempt: u32, conclusion: CiConclusion, summary: &str) -> CiReport {
    CiReport {
        provider: CiProvider::Generic,
        repository: "github.com/acme/demo".to_owned(),
        sha: orch_testsupport::fake::verify_commit(attempt),
        branch: Some("agent/fix".to_owned()),
        name: "ci/build".to_owned(),
        conclusion,
        url: Some(format!("https://ci.example.com/runs/{attempt}")),
        summary: Some(summary.to_owned()),
    }
}

/// Plays a CI system for a thread of the `ci` golden, which runs `verify-ci` under a gate that
/// requires CI: waits until the agent's push is being verified, reports a red `ci/build` for it
/// (the agent is sent back and pushes the next commit), waits for that, and reports a green one.
/// The inbox worker of `node` applies the reports.
pub async fn drive_ci(chat: &Chat, node: &Node, thread: &str) {
    use orch_core::CiConclusion::{Failure, Success};
    chat.wait_state(thread, "verifying").await;
    node.receive(
        "generic",
        "golden-delivery-1",
        golden_ci_report(1, Failure, "1 test failed: tests::login"),
    )
    .await;
    eventually(
        "the agent was sent back and its second push is being verified",
        || async {
            let t = chat.thread(thread).await;
            (t["job"]["attempt"] == 2 && t["state"] == "verifying").then_some(())
        },
    )
    .await;
    node.receive(
        "generic",
        "golden-delivery-2",
        golden_ci_report(2, Success, "3 tests passed"),
    )
    .await;
}
