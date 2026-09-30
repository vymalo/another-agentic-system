//! Shared harness: the real resource API and AG-UI routes, dispatcher, a store (in memory, or Postgres when
//! `ORCH_TEST_DATABASE_URL` is set) and the A2A adapter, talking to in-process fake A2A agents
//! over real HTTP.
//!
//! Every scenario runs as two tests, `<name>::memory` and `<name>::postgres` (see [`backends!`]);
//! the Postgres one is a no-op unless the environment variable is set. Each Postgres test gets
//! its own schema, so they run in parallel against one database.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig};
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, ThreadStore, UuidV7Ids, Wakeup};
use orch_store_postgres::{PgStore, PgWakeup};
use orch_testsupport::{
    Chat, FakeAgent, FakeAgentOptions, FakeReleases, Frame, TestInstance, fast_dispatcher,
};
use serde_json::{Value, json};

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
    coder_token: Option<String>,
    plain_token: Option<String>,
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

    /// A new orchestrator process (app state is per process; the database is shared).
    fn app<S: ThreadStore, W: Wakeup>(&self, store: S, wakeup: W) -> Arc<App<Stack<S, W>>> {
        Arc::new(App::new(
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
                ..AppConfig::default()
            },
        ))
    }

    /// Starts an orchestrator instance (API + dispatcher) named `owner`.
    pub async fn instance(&self, owner: &str) -> TestInstance {
        self.instance_with(owner, true).await
    }

    /// Starts an instance named `owner`; `dispatch: false` serves the API only.
    pub async fn instance_with(&self, owner: &str, dispatch: bool) -> TestInstance {
        let dispatcher = dispatch.then(fast_dispatcher);
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        match &self.db {
            Db::Memory { store, wakeup } => {
                let app = self.app(store.clone(), wakeup.clone());
                TestInstance::spawn_with(app, api, dispatcher, owner).await
            }
            Db::Postgres(db) => {
                let pool = db.pool(owner, 8).await;
                let wakeup = PgWakeup::start(pool.clone());
                assert!(
                    wakeup.wait_listening(Duration::from_secs(10)).await,
                    "the wakeup listener did not attach"
                );
                let app = self.app(PgStore::from_pool(pool), wakeup);
                TestInstance::spawn_with(app, api, dispatcher, owner).await
            }
        }
    }

    /// A client for `instance` acting as [`ALICE`], which can also read the event log.
    pub fn chat(&self, instance: &TestInstance) -> Chat {
        instance.chat(ALICE)
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
