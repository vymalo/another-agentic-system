//! Test support (feature `testkit`): a scripted adam-rs agent, a database shared by several
//! processes' worth of local agents, and the [`AgentFixture`] the `AgentClient` conformance
//! suite runs on. Nothing here ships in the orchestrator's binary.
//!
//! The scripted agent is named [`SCRIPTED`] and is chosen by the first word of the message,
//! like the fake A2A agent of `orch-testsupport`:
//!
//! | first word | the task |
//! |---|---|
//! | `echo` (and any other) | an artifact, then `completed` |
//! | `ask` | `input-required`; the next message completes it with an artifact |
//! | `gate` | `working` until [`LocalWorld::release_gate`], then an artifact and `completed` |
//! | `slow` | `working` until it is cancelled |
//! | `fail` | `failed` with [`FAILURE_MESSAGE`] |
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::str::FromStr as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adam_core::MemoryStore;
use adam_runtime::{
    Agent, AgentError, AgentStarter, BroadcastSink, Ctx, Inbound, RunEvent, Transition,
};
use async_trait::async_trait;
use orch_core::AgentId;
use orch_ports::AgentEndpoint;
use orch_ports::testkit::agent_client::AgentFixture;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection as _, PgConnection, PgPool};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use orch_ports::testkit::agent_client::FAILURE_MESSAGE;

use crate::agents::Hosted;
use crate::{LocalAgentClient, LocalAgents, LocalAgentsError, LocalKind, LocalOptions};

/// The name of the scripted agent (the `name` of its endpoints).
pub const SCRIPTED: &str = "scripted";

/// The environment variable that names the test database, shared with the rest of the
/// workspace. Without it the Postgres variants skip.
pub const DATABASE_URL_VAR: &str = "ORCH_TEST_DATABASE_URL";

/// Where a scripted run is: the first word of the message, the whole text, and how far it got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ScriptState {
    text: String,
    phase: u32,
}

fn initial(input: &Inbound) -> Result<ScriptState, AgentError> {
    let text = input
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| AgentError::permanent("unusable start message: it has no text"))?;
    Ok(ScriptState {
        text: text.to_owned(),
        phase: 0,
    })
}

/// What the processes of one [`LocalWorld`] share: the gate, and a record of who did what.
struct Shared {
    gate: Semaphore,
    /// Steps currently waiting at the gate.
    waiting: AtomicUsize,
    /// The instance ids that completed a `gate` task, in order.
    finished_by: Mutex<Vec<String>>,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Shared {
            gate: Semaphore::new(0),
            waiting: AtomicUsize::new(0),
            finished_by: Mutex::new(Vec::new()),
        })
    }
}

/// Counts a step at the gate for as long as its future lives, so a step that is aborted (a
/// process that dies) stops counting.
struct AtGate<'a>(&'a AtomicUsize);

impl<'a> AtGate<'a> {
    fn enter(waiting: &'a AtomicUsize) -> Self {
        waiting.fetch_add(1, Ordering::SeqCst);
        AtGate(waiting)
    }
}

impl Drop for AtGate<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The scripted agent of one process (`instance`) of a [`LocalWorld`].
#[derive(Clone)]
struct Scripted {
    shared: Arc<Shared>,
    instance: String,
}

async fn artifact(ctx: &Ctx, text: &str) {
    ctx.emit(RunEvent::Artifact {
        name: "result".to_owned(),
        mime_type: Some("text/plain".to_owned()),
        data: json!(format!("echo: {text}")),
    })
    .await;
}

#[async_trait]
impl Agent for Scripted {
    type State = ScriptState;

    fn name(&self) -> &str {
        SCRIPTED
    }

    fn init(&self, input: Inbound) -> Result<ScriptState, AgentError> {
        initial(&input)
    }

    async fn step(
        &self,
        ctx: &mut Ctx,
        mut state: ScriptState,
    ) -> Result<Transition<ScriptState>, AgentError> {
        let word = state.text.split_whitespace().next().unwrap_or_default();
        match word {
            "fail" => Ok(Transition::Fail {
                state,
                error: FAILURE_MESSAGE.to_owned(),
            }),
            "slow" => {
                let wake_at = ctx.now() + chrono::Duration::milliseconds(50);
                state.phase += 1;
                Ok(Transition::Park {
                    state,
                    wake_at: Some(wake_at),
                })
            }
            "gate" if state.phase == 0 => {
                // One commit first, so the task is `working` (not `submitted`) while it waits.
                state.phase = 1;
                Ok(Transition::Continue(state))
            }
            "gate" => {
                {
                    let _waiting = AtGate::enter(&self.shared.waiting);
                    self.shared
                        .gate
                        .acquire()
                        .await
                        .expect("the gate is never closed")
                        .forget();
                }
                self.shared
                    .finished_by
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(self.instance.clone());
                artifact(ctx, &state.text).await;
                let output = json!({ "text": format!("echo: {}", state.text) });
                Ok(Transition::Done { state, output })
            }
            "ask" if state.phase == 0 => {
                state.phase = 1;
                Ok(Transition::Park {
                    state,
                    wake_at: None,
                })
            }
            "ask" => {
                let answers = ctx.take_inbox();
                let Some(answer) = answers.first() else {
                    return Ok(Transition::Park {
                        state,
                        wake_at: None,
                    });
                };
                let answer = answer.payload["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                artifact(ctx, &answer).await;
                let output = json!({ "text": format!("answered: {answer}") });
                Ok(Transition::Done { state, output })
            }
            _ => {
                artifact(ctx, &state.text).await;
                let output = json!({ "text": format!("echo: {}", state.text) });
                Ok(Transition::Done { state, output })
            }
        }
    }
}

impl AgentStarter for Scripted {
    type State = ScriptState;

    fn name(&self) -> &str {
        SCRIPTED
    }

    fn init(&self, input: Inbound) -> Result<ScriptState, AgentError> {
        initial(&input)
    }
}

fn hosted_scripted(shared: Arc<Shared>, instance: String) -> Hosted {
    Hosted::new(
        SCRIPTED,
        "a scripted agent for tests",
        move |builder, steps| {
            let scripted = Scripted { shared, instance };
            if steps {
                builder.agent(scripted)
            } else {
                builder.starter(scripted)
            }
        },
    )
}

/// An endpoint of the scripted agent, as the configuration key `id` names it.
pub fn scripted_endpoint(id: &str) -> AgentEndpoint {
    AgentEndpoint::local(AgentId::new(id), SCRIPTED)
}

/// A creation of a fresh, private schema in the test database, so tests run in parallel against
/// one database. `None` when [`DATABASE_URL_VAR`] is unset.
async fn private_schema() -> Option<PgConnectOptions> {
    let url = std::env::var(DATABASE_URL_VAR)
        .ok()
        .filter(|v| !v.is_empty())?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // The `t_` prefix and the timestamp are the convention of the workspace's other test
    // helpers, which drop the schemas an hour after they were made.
    let schema = format!("t_{secs}_{}", uuid::Uuid::now_v7().simple());
    let mut conn = PgConnection::connect(&url)
        .await
        .expect("connect to the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA \"{schema}\"")))
        .execute(&mut conn)
        .await
        .expect("create the test schema");
    Some(
        PgConnectOptions::from_str(&url)
            .expect("a valid ORCH_TEST_DATABASE_URL")
            .options([("search_path", schema.as_str())]),
    )
}

async fn connect(options: &PgConnectOptions, name: &str, max_connections: u32) -> PgPool {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options.clone().application_name(name))
        .await
        .expect("connect the pool")
}

enum Journal {
    Memory(Arc<MemoryStore>),
    /// One schema of the test database; every process opens its own pool on it.
    Postgres(Box<PgConnectOptions>),
}

/// The journal, the gate and the schema that several [`LocalInstance`]s share, the way several
/// orchestrator processes share one database.
pub struct LocalWorld {
    journal: Journal,
    shared: Arc<Shared>,
}

impl LocalWorld {
    /// Processes that share an in-memory journal.
    pub fn memory() -> Self {
        LocalWorld {
            journal: Journal::Memory(Arc::new(MemoryStore::new())),
            shared: Shared::new(),
        }
    }

    /// Processes that share a private schema of the test database; `None` (skip) when
    /// [`DATABASE_URL_VAR`] is unset.
    pub async fn postgres() -> Option<Self> {
        Some(Self::postgres_over(private_schema().await?))
    }

    /// Processes that share the schema `options` connect to (an end-to-end test that keeps the
    /// orchestrator's own tables in the same schema passes its options).
    pub fn postgres_over(options: PgConnectOptions) -> Self {
        LocalWorld {
            journal: Journal::Postgres(Box::new(options)),
            shared: Shared::new(),
        }
    }

    /// The options a test starts from: fast polling, a 1 s lease, stepping.
    pub fn options(instance_id: &str) -> LocalOptions {
        LocalOptions {
            poll_interval: Duration::from_millis(10),
            lease_ttl: Duration::from_secs(1),
            ..LocalOptions::new(instance_id)
        }
    }

    /// Lets one waiting `gate` task continue (a permit is kept if none waits yet).
    pub fn release_gate(&self) {
        self.shared.gate.add_permits(1);
    }

    /// How many `gate` steps wait at the gate right now (a step of a process that was killed
    /// no longer counts).
    pub fn gate_waiters(&self) -> usize {
        self.shared.waiting.load(Ordering::SeqCst)
    }

    /// The ids of the instances that completed a `gate` task, in order.
    pub fn finished_by(&self) -> Vec<String> {
        self.shared
            .finished_by
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A pool on the shared schema (`None` for the in-memory journal), for a test that looks at
    /// the tables and channels themselves.
    pub async fn pool(&self, application_name: &str, max_connections: u32) -> Option<PgPool> {
        match &self.journal {
            Journal::Memory(_) => None,
            Journal::Postgres(options) => {
                Some(connect(options, application_name, max_connections).await)
            }
        }
    }

    /// A process hosting the production `echo` kind and the scripted agent, migrated and
    /// running (unless `opts.steps` is `false`, which is a control plane: it runs nothing).
    pub async fn instance(&self, opts: LocalOptions) -> LocalInstance {
        let hosted = vec![
            LocalKind::Echo.hosted(),
            hosted_scripted(self.shared.clone(), opts.instance_id.clone()),
        ];
        let id = opts.instance_id.clone();
        let agents = match &self.journal {
            Journal::Memory(store) => {
                let events = BroadcastSink::default();
                LocalAgents::assemble(store.clone(), None, events, opts, hosted)
            }
            Journal::Postgres(options) => {
                let pool = connect(options, &id, 8).await;
                LocalAgents::postgres_hosting(pool, opts, hosted).expect("build the local agents")
            }
        };
        agents.migrate().await.expect("migrate the journal");
        let agents = Arc::new(agents);
        let client = agents.client();
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let (agents, stop) = (agents.clone(), stop.clone());
            async move { agents.run(stop).await }
        });
        LocalInstance {
            agents,
            client,
            stop,
            task: Some(task),
        }
    }
}

/// One process's local agents, running.
pub struct LocalInstance {
    /// The agents, for `migrate`, `client` and the like.
    pub agents: Arc<LocalAgents>,
    /// The client a dispatcher would call.
    pub client: LocalAgentClient,
    stop: CancellationToken,
    task: Option<JoinHandle<Result<(), LocalAgentsError>>>,
}

impl LocalInstance {
    /// A crash: the worker's future is dropped where it stands, no lease is released, the steps
    /// in flight are aborted.
    pub fn kill(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    /// A graceful stop: in-flight steps finish, leases are released. Returns what `run` returned.
    pub async fn shutdown(mut self) -> Result<(), LocalAgentsError> {
        self.stop.cancel();
        match self.task.take() {
            Some(task) => task.await.expect("the worker task does not panic"),
            None => Ok(()),
        }
    }
}

impl Drop for LocalInstance {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The [`AgentFixture`] of the `AgentClient` conformance suite: one process with the scripted
/// agent, on the in-memory journal or on Postgres.
pub struct LocalFixture {
    world: LocalWorld,
    instance: LocalInstance,
}

impl LocalFixture {
    /// On the in-memory journal.
    pub async fn memory() -> Self {
        Self::over(LocalWorld::memory()).await
    }

    /// On Postgres; `None` (skip) when [`DATABASE_URL_VAR`] is unset.
    pub async fn postgres() -> Option<Self> {
        Some(Self::over(LocalWorld::postgres().await?).await)
    }

    async fn over(world: LocalWorld) -> Self {
        let instance = world.instance(LocalWorld::options("fixture")).await;
        LocalFixture { world, instance }
    }

    /// Unused by the suite; for tests that reach past the client.
    pub fn instance(&self) -> &LocalInstance {
        &self.instance
    }
}

impl AgentFixture for LocalFixture {
    type Client = LocalAgentClient;

    fn client(&self) -> &LocalAgentClient {
        &self.instance.client
    }

    fn endpoint(&self) -> AgentEndpoint {
        scripted_endpoint("scripted")
    }

    /// A local agent of a kind this process does not host.
    fn unreachable(&self) -> AgentEndpoint {
        AgentEndpoint::local(AgentId::new("elsewhere"), "not-hosted-here")
    }

    fn release_gate(&self) {
        self.world.release_gate();
    }
}
