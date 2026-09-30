//! Local agents (ADR 0015) through the real application and dispatcher: the client is
//! `ByTransport<A2aAgentClient, LocalAgentClient>`, the one the binary builds with the feature
//! `agent-local`, over the in-memory journal or Postgres (skipped without
//! `ORCH_TEST_DATABASE_URL`). The orchestrator's own tables and the local agents' journal share
//! one schema, as they share one database in production.
//!
//! No HTTP is involved: what a surface would do (create a thread, read the log, cancel) is done
//! on `App` directly, so these tests do not depend on any interaction surface.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_agent_adam::LocalAgentClient;
use orch_agent_adam::testkit::{LocalInstance, LocalWorld, SCRIPTED, scripted_endpoint};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, NewThread};
use orch_core::{AgentId, AgentTarget, AgentTaskState, Event, ThreadId, ThreadState, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{
    AgentClient as _, AgentEndpoint, ByTransport, PortSet, Ports as _, SystemClock, TaskHandle,
    ThreadStore, UuidV7Ids, Wakeup,
};
use orch_store_postgres::{PgStore, PgWakeup};
use orch_testsupport::{eventually, fast_dispatcher};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../store-postgres/tests/support/mod.rs"]
mod pgdb;

type Stack<S, W> =
    PortSet<S, W, ByTransport<A2aAgentClient, LocalAgentClient>, SystemClock, UuidV7Ids>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Memory,
    Postgres,
}

fn postgres_available() -> bool {
    let available = pgdb::database_url().is_some();
    if !available {
        eprintln!("skipping the Postgres variant: ORCH_TEST_DATABASE_URL is not set");
    }
    available
}

/// Defines, for each scenario `fn name(backend: Backend)`, the tests `name::memory` and
/// `name::postgres`.
macro_rules! backends {
    ($($name:ident),+ $(,)?) => {
        $(
            mod $name {
                #[tokio::test]
                async fn memory() {
                    super::$name(super::Backend::Memory).await;
                }

                #[tokio::test]
                async fn postgres() {
                    if super::postgres_available() {
                        super::$name(super::Backend::Postgres).await;
                    }
                }
            }
        )+
    };
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

/// The orchestrator's database and the local agents' journal, shared by every instance.
struct World {
    db: Db,
    local: LocalWorld,
    a2a: A2aAgentClient,
}

enum Db {
    Memory(MemoryStore, MemoryWakeup),
    Postgres(pgdb::TestDb),
}

impl World {
    async fn start(backend: Backend) -> Self {
        let (db, local) = match backend {
            Backend::Memory => (
                Db::Memory(MemoryStore::new(), MemoryWakeup::new()),
                LocalWorld::memory(),
            ),
            Backend::Postgres => {
                let db = pgdb::TestDb::new()
                    .await
                    .expect("the Postgres variant needs ORCH_TEST_DATABASE_URL");
                db.store().await; // the orchestrator's tables, once; instances only connect
                let local = LocalWorld::postgres_over(db.options());
                (Db::Postgres(db), local)
            }
        };
        World {
            db,
            local,
            a2a: A2aAgentClient::new(A2aConfig {
                use_system_proxy: false,
                ..A2aConfig::default()
            })
            .unwrap(),
        }
    }

    fn directory() -> AgentDirectory {
        let entry = |name: &str, endpoint: AgentEndpoint| AgentEntry {
            endpoint,
            name: name.to_owned(),
        };
        AgentDirectory::new(vec![
            entry("Echo", AgentEndpoint::local(AgentId::new("echo"), "echo")),
            entry("Scripted", scripted_endpoint(SCRIPTED)),
        ])
    }

    /// A new orchestrator process: its own pools, its local agents, its dispatcher.
    async fn instance(&self, owner: &str) -> Instance {
        let local = self.local.instance(LocalWorld::options(owner)).await;
        let agents = ByTransport {
            a2a: self.a2a.clone(),
            local: local.client.clone(),
        };
        let app = match &self.db {
            Db::Memory(store, wakeup) => AnyApp::Memory(app(store.clone(), wakeup.clone(), agents)),
            Db::Postgres(db) => {
                let pool = db.pool(owner, 8).await;
                let wakeup = PgWakeup::start(pool.clone());
                assert!(
                    wakeup.wait_listening(Duration::from_secs(10)).await,
                    "the wakeup listener did not attach"
                );
                AnyApp::Postgres(app(PgStore::from_pool(pool), wakeup, agents))
            }
        };
        let stop = CancellationToken::new();
        let dispatcher = match &app {
            AnyApp::Memory(app) => spawn_dispatcher(app, owner, &stop),
            AnyApp::Postgres(app) => spawn_dispatcher(app, owner, &stop),
        };
        Instance {
            app,
            local,
            dispatcher,
            stop,
        }
    }
}

fn app<S: ThreadStore, W: Wakeup>(
    store: S,
    wakeup: W,
    agents: ByTransport<A2aAgentClient, LocalAgentClient>,
) -> Arc<App<Stack<S, W>>> {
    Arc::new(
        App::new(
            PortSet {
                store,
                wakeup,
                agents,
                clock: SystemClock,
                ids: UuidV7Ids,
            },
            World::directory(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn spawn_dispatcher<S: ThreadStore, W: Wakeup>(
    app: &Arc<App<Stack<S, W>>>,
    owner: &str,
    stop: &CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(Dispatcher::new(Arc::clone(app), fast_dispatcher(), owner).run(stop.clone()))
}

enum AnyApp {
    Memory(Arc<App<Stack<MemoryStore, MemoryWakeup>>>),
    Postgres(Arc<App<Stack<PgStore, PgWakeup>>>),
}

/// Runs `$body` with `$app` bound to the instance's `App`, whichever store it sits on.
macro_rules! on {
    ($any:expr, $app:ident => $body:expr) => {
        match &$any {
            AnyApp::Memory($app) => $body,
            AnyApp::Postgres($app) => $body,
        }
    };
}

/// One orchestrator process: an app, its local agents and its dispatcher.
struct Instance {
    app: AnyApp,
    local: LocalInstance,
    dispatcher: JoinHandle<()>,
    stop: CancellationToken,
}

impl Instance {
    async fn create(&self, agent: &str, text: &str) -> ThreadId {
        on!(self.app, app => app
            .create_thread(
                &alice(),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: None,
                    },
                    text: text.to_owned(),
                },
            )
            .await
            .unwrap()
            .id)
    }

    async fn state(&self, id: ThreadId) -> ThreadState {
        on!(self.app, app => app.get_thread(&alice(), id).await.unwrap().state)
    }

    async fn wait_state(&self, id: ThreadId, want: ThreadState) {
        eventually(&format!("thread state {want:?}"), || async {
            (self.state(id).await == want).then_some(())
        })
        .await;
    }

    async fn events(&self, id: ThreadId) -> Vec<Event> {
        on!(self.app, app => app.list_events(&alice(), id, 0, 500).await.unwrap())
    }

    async fn cancel(&self, id: ThreadId) {
        on!(self.app, app => app.cancel(&alice(), id).await.unwrap());
    }

    /// The task the thread is bound to on its agent.
    async fn task_of(&self, id: ThreadId) -> String {
        eventually("the thread is bound to a task", || async {
            let binding = on!(self.app, app => app.ports().store().get_binding(id).await.unwrap());
            binding.and_then(|b| b.task_id)
        })
        .await
    }

    /// A crash: the dispatcher and the local worker are dropped where they stand.
    fn kill(&mut self) {
        self.dispatcher.abort();
        self.local.kill();
    }

    async fn shutdown(self) {
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.dispatcher).await;
        self.local.shutdown().await.unwrap();
    }
}

/// `kind` (with the status or state for the kinds that have one) of each event.
fn shape(events: &[Event]) -> Vec<String> {
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

fn assert_contiguous(events: &[Event]) {
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    let want: Vec<i64> = (1..=i64::try_from(events.len()).unwrap()).collect();
    assert_eq!(seqs, want, "seq must be contiguous from 1");
}

async fn echo_through_the_dispatcher(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let id = orch.create("echo", "hello local").await;
    orch.wait_state(id, ThreadState::Done).await;

    let events = orch.events(id).await;
    assert_contiguous(&events);
    let shape = shape(&events);
    assert_eq!(
        shape.first().map(String::as_str),
        Some("user_message"),
        "{shape:?}"
    );
    assert_eq!(
        &shape[shape.len() - 2..],
        ["agent_status:completed", "thread_state:done"],
        "{shape:?}"
    );
    // The agent's answer is in the log, once: the detail of the completed status.
    let completed: Vec<_> = events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .filter(|v| v["kind"] == "agent_status" && v["data"]["status"] == "completed")
        .collect();
    assert_eq!(completed.len(), 1, "{shape:?}");
    assert_eq!(
        completed[0]["data"]["detail"], "hello local",
        "{completed:?}"
    );
    orch.shutdown().await;
}

async fn restart_mid_task_finishes_with_no_gap_and_no_duplicate(backend: Backend) {
    let world = World::start(backend).await;
    let mut first = world.instance("orch-1").await;
    let id = first.create(SCRIPTED, "gate crash").await;
    first.wait_state(id, ThreadState::Working).await;
    let task = first.task_of(id).await;
    // The local step is inside its gate: the first process holds the run.
    eventually("the local step at the gate", || async {
        (world.local.gate_waiters() == 1).then_some(())
    })
    .await;
    let before = first.events(id).await;

    // The process dies mid-task: nothing is released, both leases have to expire.
    first.kill();
    let second = world.instance("orch-2").await;
    // The new process steps the run again (it waits at the gate) and its dispatcher re-attaches.
    eventually("the second process at the gate", || async {
        (world.local.gate_waiters() == 1).then_some(())
    })
    .await;
    world.local.release_gate();
    second.wait_state(id, ThreadState::Done).await;

    let events = second.events(id).await;
    assert_contiguous(&events);
    assert_eq!(
        &events[..before.len()],
        &before[..],
        "what was said before the crash is untouched"
    );
    let shape = shape(&events);
    assert_eq!(
        shape
            .iter()
            .filter(|s| *s == "agent_status:working")
            .count(),
        1,
        "{shape:?}"
    );
    assert_eq!(
        shape.iter().filter(|s| *s == "artifact").count(),
        1,
        "{shape:?}"
    );
    assert_eq!(
        &shape[shape.len() - 2..],
        ["agent_status:completed", "thread_state:done"],
        "{shape:?}"
    );
    // One task, stepped to its end by the second process only, and never started twice.
    assert_eq!(second.task_of(id).await, task);
    assert_eq!(world.local.finished_by(), ["orch-2"]);
    second.shutdown().await;
}

async fn cancel_reaches_the_local_task(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let id = orch.create(SCRIPTED, "slow work").await;
    orch.wait_state(id, ThreadState::Working).await;
    let task = orch.task_of(id).await;

    orch.cancel(id).await;
    orch.wait_state(id, ThreadState::Cancelled).await;

    // The task itself is cancelled in the local journal, not merely forgotten by the thread.
    let handle = TaskHandle {
        endpoint: scripted_endpoint(SCRIPTED),
        task_id: task,
    };
    eventually("the local task is cancelled", || async {
        let snapshot = orch.local.client.get_task(&handle).await.unwrap();
        (snapshot.state == AgentTaskState::Canceled).then_some(())
    })
    .await;
    let events = orch.events(id).await;
    assert_contiguous(&events);
    let shape = shape(&events);
    assert_eq!(
        &shape[shape.len() - 2..],
        ["agent_status:canceled", "thread_state:cancelled"],
        "{shape:?}"
    );
    orch.shutdown().await;
}

backends!(
    echo_through_the_dispatcher,
    restart_mid_task_finishes_with_no_gap_and_no_duplicate,
    cancel_reaches_the_local_task,
);
