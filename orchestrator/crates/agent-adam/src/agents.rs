//! [`LocalAgents`]: the adam-rs runtime of one orchestrator process, its store and its worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use adam_a2a_runtime::RuntimeTaskBackend;
use adam_core::{ClaimScope, DynStore, MemoryStore};
use adam_notify_postgres::PgNotify;
use adam_runtime::{BroadcastSink, Runtime, RuntimeBuilder};
use adam_store_postgres::PgStore;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;

use crate::client::{Entry, LocalAgentClient};
use crate::error::LocalAgentsError;
use crate::{TABLE_PREFIX, echo};

/// A kind of agent this crate can host in the orchestrator's process.
///
/// A closed enum (ADR 0004): a new kind is a new variant, and the compiler lists every `match`
/// that must learn it. The orchestrator's configuration has its own enum of the same kinds (so
/// that it can name and refuse them without this crate), and the composition root maps one to
/// the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalKind {
    /// Repeats the user's message back. Needs no model.
    Echo,
}

impl LocalKind {
    /// Every kind this crate knows.
    pub const ALL: &'static [LocalKind] = &[LocalKind::Echo];

    /// The name of the kind: the `agent` of a run in the journal and the `name` of
    /// `AgentTransport::Local`.
    pub const fn name(self) -> &'static str {
        match self {
            LocalKind::Echo => echo::NAME,
        }
    }

    /// One line for humans, the description of the kind's card.
    pub const fn description(self) -> &'static str {
        match self {
            LocalKind::Echo => echo::DESCRIPTION,
        }
    }

    pub(crate) fn hosted(self) -> Hosted {
        match self {
            LocalKind::Echo => Hosted::new(self.name(), self.description(), |builder, steps| {
                if steps {
                    builder.agent(echo::Echo)
                } else {
                    builder.starter(echo::EchoStarter)
                }
            }),
        }
    }
}

/// How one process runs its local agents.
#[derive(Debug, Clone)]
pub struct LocalOptions {
    /// Names this process in run leases. Unique per process: two workers with one id would
    /// take each other's leases for their own.
    pub instance_id: String,
    /// Runs stepped at once, at least 1.
    pub concurrency: usize,
    /// How long a claimed run stays leased without renewal: how long after a worker dies its
    /// runs are found again by another.
    pub lease_ttl: Duration,
    /// How often an idle worker, and every open subscription, looks at the journal.
    pub poll_interval: Duration,
    /// Whether this process steps runs. `false` registers the agents' starters only: it can
    /// start, read and cancel runs but never advances one (the control plane).
    pub steps: bool,
}

impl LocalOptions {
    /// The defaults for a process named `instance_id`: 4 runs at once, a 30 s lease, a 250 ms
    /// poll, stepping.
    pub fn new(instance_id: impl Into<String>) -> Self {
        LocalOptions {
            instance_id: instance_id.into(),
            concurrency: 4,
            lease_ttl: Duration::from_secs(30),
            poll_interval: Duration::from_millis(250),
            steps: true,
        }
    }
}

/// A kind, ready to be registered on a runtime: what the composition offers to `assemble`.
pub(crate) struct Hosted {
    pub(crate) name: String,
    pub(crate) description: String,
    register: Box<dyn FnOnce(RuntimeBuilder, bool) -> RuntimeBuilder + Send>,
}

impl Hosted {
    /// `register` receives the builder and whether this process steps runs (agent) or only
    /// starts them (starter).
    pub(crate) fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        register: impl FnOnce(RuntimeBuilder, bool) -> RuntimeBuilder + Send + 'static,
    ) -> Self {
        Hosted {
            name: name.into(),
            description: description.into(),
            register: Box::new(register),
        }
    }
}

/// The local agents of one orchestrator process: one adam-rs `Runtime` with every configured
/// kind registered, its journal and its worker.
///
/// Build it with [`postgres`](Self::postgres) (the journal lives in the orchestrator's own
/// database under the prefix [`TABLE_PREFIX`], and processes find each other's work through
/// `LISTEN`/`NOTIFY`) or [`memory`](Self::memory) (tests, a single process). Then
/// [`migrate`](Self::migrate), hand [`client`](Self::client) to the dispatcher and run
/// [`run`](Self::run) beside it.
///
/// It holds no state of its own: the runs are in the store, so a process that dies loses
/// nothing, and another worker resumes a run from its last commit once the lease has expired.
pub struct LocalAgents {
    runtime: Runtime,
    store: DynStore,
    notify: Option<PgNotify>,
    steps: bool,
    entries: Arc<HashMap<String, Entry>>,
}

impl std::fmt::Debug for LocalAgents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalAgents")
            .field("worker", &self.runtime.worker_id())
            .field("steps", &self.steps)
            .field("kinds", &self.entries.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl LocalAgents {
    /// Connects a pool to `database_url` (the orchestrator's own database), sized for these
    /// agents (`opts.concurrency` plus 4: the notifier's listener and a little headroom), and
    /// builds [`postgres`](Self::postgres) over it. The pool is separate from the
    /// orchestrator's, so a burst of steps cannot starve its store. A database that cannot be
    /// reached is a transient [`LocalAgentsError::Store`].
    pub async fn connect(
        database_url: &str,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        let size = u32::try_from(opts.concurrency.saturating_add(4)).unwrap_or(u32::MAX);
        let pool = PgPoolOptions::new()
            .max_connections(size)
            .connect(database_url)
            .await
            .map_err(LocalAgentsError::unavailable)?;
        Self::postgres(pool, opts, kinds)
    }

    /// Local agents whose journal is the Postgres behind `pool`, in tables prefixed
    /// [`TABLE_PREFIX`] and notified through the channels of the same prefix.
    ///
    /// The pool is dedicated: the notifier's listener holds one of its connections for as long
    /// as [`run`](Self::run) runs, and each stepping run holds one briefly, so give it
    /// `concurrency + 4`. Call [`migrate`](Self::migrate) before first use.
    pub fn postgres(
        pool: PgPool,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        Self::postgres_hosting(pool, opts, kinds.iter().map(|k| k.hosted()).collect())
    }

    /// [`postgres`](Self::postgres) over any hosted set: the testkit adds its scripted agent.
    pub(crate) fn postgres_hosting(
        pool: PgPool,
        opts: LocalOptions,
        hosted: Vec<Hosted>,
    ) -> Result<Self, LocalAgentsError> {
        let store = PgStore::from_pool(pool.clone())
            .with_table_prefix(TABLE_PREFIX)
            .map_err(LocalAgentsError::Store)?;
        let events = BroadcastSink::default();
        // A process that does not step (the control plane) runs no notifier: it would only
        // queue signals nobody sends.
        let notify = if opts.steps {
            Some(
                PgNotify::new(pool, events.clone())
                    .with_channel_prefix(TABLE_PREFIX)
                    .map_err(LocalAgentsError::Notify)?,
            )
        } else {
            None
        };
        Ok(Self::assemble(
            Arc::new(store),
            notify,
            events,
            opts,
            hosted,
        ))
    }

    /// Local agents whose journal is in this process's memory: nothing survives the process,
    /// and no other process sees the runs. For tests and a single-process trial.
    pub fn memory(opts: LocalOptions, kinds: &[LocalKind]) -> Self {
        Self::assemble(
            Arc::new(MemoryStore::new()),
            None,
            BroadcastSink::default(),
            opts,
            kinds.iter().map(|k| k.hosted()).collect(),
        )
    }

    /// The composition: a runtime over `store`, one task backend per hosted kind.
    pub(crate) fn assemble(
        store: DynStore,
        notify: Option<PgNotify>,
        events: BroadcastSink,
        opts: LocalOptions,
        hosted: Vec<Hosted>,
    ) -> Self {
        let mut builder = Runtime::builder(Arc::clone(&store))
            .worker_id(opts.instance_id.clone())
            .claim_scope(ClaimScope::Any)
            .lease_ttl(opts.lease_ttl)
            .poll_interval(opts.poll_interval)
            .concurrency(opts.concurrency);
        builder = match &notify {
            // What the steps emit reaches this process's subscribers at once and the other
            // processes' through NOTIFY; new work and cancels wake their workers likewise.
            Some(notify) => builder
                .event_sink(notify.event_sink())
                .notifier(notify.notifier()),
            None => builder.event_sink(events.clone()),
        };
        let mut kinds = Vec::with_capacity(hosted.len());
        for kind in hosted {
            kinds.push((kind.name.clone(), kind.description.clone()));
            builder = (kind.register)(builder, opts.steps);
        }
        let runtime = builder.build();
        let entries = kinds
            .into_iter()
            .map(|(name, description)| {
                let backend = RuntimeTaskBackend::new(runtime.clone(), events.clone(), &name)
                    .with_poll_interval(opts.poll_interval);
                (name, Entry::new(description, backend))
            })
            .collect();
        LocalAgents {
            runtime,
            store,
            notify,
            steps: opts.steps,
            entries: Arc::new(entries),
        }
    }

    /// Creates the journal's tables if they are missing. Idempotent, and safe to run from
    /// every replica at once: the store serialises it with an advisory lock of its own prefix.
    pub async fn migrate(&self) -> Result<(), LocalAgentsError> {
        self.store.migrate().await.map_err(LocalAgentsError::Store)
    }

    /// The [`AgentClient`](orch_ports::AgentClient) for the endpoints of these agents.
    pub fn client(&self) -> LocalAgentClient {
        LocalAgentClient::new(Arc::clone(&self.entries))
    }

    /// Steps runs and carries notifications until `stop` is cancelled: the worker and the
    /// notifier together. On stop no new run is claimed, the steps in flight finish and commit,
    /// and their leases are released. If either half ends by itself, the other is stopped and
    /// the first error is returned.
    ///
    /// A process that does not step ([`LocalOptions::steps`] is `false`) has nothing to run:
    /// this waits for `stop`.
    pub async fn run(&self, stop: CancellationToken) -> Result<(), LocalAgentsError> {
        if !self.steps {
            stop.cancelled().await;
            return Ok(());
        }
        let both = stop.child_token();
        let worker = async {
            let outcome = self
                .runtime
                .run_worker(both.clone().cancelled_owned())
                .await
                .map_err(LocalAgentsError::Worker);
            both.cancel();
            outcome
        };
        let notifier = async {
            let Some(notify) = &self.notify else {
                return Ok(());
            };
            let outcome = notify
                .run(both.clone().cancelled_owned())
                .await
                .map_err(LocalAgentsError::Notify);
            both.cancel();
            outcome
        };
        let (worker, notifier) = tokio::join!(worker, notifier);
        worker.and(notifier)
    }
}
