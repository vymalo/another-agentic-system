//! [`LocalAgents`]: the adam-rs runtime of one orchestrator process, its store and its worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use adam_a2a_runtime::RuntimeTaskBackend;
use adam_core::{ClaimScope, DynStore, MemoryStore, StoreError};
use adam_notify_postgres::PgNotify;
use adam_postgres::PgConfig;
use adam_runtime::{BroadcastSink, Runtime, RuntimeBuilder};
use adam_store_postgres::PgStore;
use deadpool_postgres::Pool;
use orch_pg_connect::PgOptions;
use tokio_util::sync::CancellationToken;

use crate::client::{Entry, LocalAgentClient};
use crate::error::LocalAgentsError;
use crate::{TABLE_PREFIX, echo};

/// The connections of the direct pool of [`LocalAgents::postgres_direct`]: the notifier's `NOTIFY`s
/// (sent one statement at a time) and the creation of the tables. The notifier's listener is a
/// connection of its own beside them.
pub const DIRECT_CONNECTIONS: u32 = 2;

/// How long a call waits for a connection of a pool [`LocalAgents::connect`] made: sqlx's default
/// `acquire_timeout`, the wait those pools had on sqlx.
pub(crate) const POOL_WAIT: Duration = Duration::from_secs(30);

/// `url` as the orchestrator's other crates read a connection string (`orch-pg-connect`: the
/// `PG*` variables, the URL, the password file; sqlx's TLS). The error never holds the text: it
/// may carry a password. No `extra_float_digits` is sent (nor any startup parameter but the URL's
/// own): PgBouncer refuses one it does not track, and sqlx sent it unless told not to.
pub(crate) fn options(url: &str) -> Result<PgOptions, LocalAgentsError> {
    url.parse::<PgOptions>().map_err(|_| unparsable())
}

fn unparsable() -> LocalAgentsError {
    LocalAgentsError::unavailable(std::io::Error::other(
        "the connection string cannot be parsed",
    ))
}

/// A pool of at most `size` connections to `url`, one of them made now: a database that cannot be
/// reached fails here, as sqlx's `connect_with` failed, not at the first statement.
async fn connect_pool(url: &str, size: u32) -> Result<Pool, LocalAgentsError> {
    let size = usize::try_from(size.max(1)).unwrap_or(usize::MAX);
    let pool = options(url)?
        .pool(size, POOL_WAIT)
        .map_err(LocalAgentsError::unavailable)?;
    drop(pool.get().await.map_err(LocalAgentsError::unavailable)?);
    Ok(pool)
}

/// Where the notifier's listener connects: `url` as adam-rs's `adam-postgres` reads it, the only
/// configuration `PgNotify` takes for the connection it opens itself (`LISTEN` needs a session of
/// its own, outside any pool). `adam-postgres` reads the URL alone, no `PG*` variable and no
/// password file, so a `url` that leaves part of the server to them (a password in `PGPASSWORD`,
/// say) is refused here: the pools would connect and the listener would not, and the agents would
/// fall back to polling with nothing said. The error names the part, never a value.
fn listener(url: &str) -> Result<PgConfig, LocalAgentsError> {
    if let Some(part) = PgOptions::left_to_the_environment(url).map_err(|_| unparsable())? {
        return Err(LocalAgentsError::store(StoreError::InvalidInput(format!(
            "the in-process agents' notifier reads only the connection string, and its {part} \
             comes from the environment (a PG* variable, the password file or the operating \
             system's user): name it in the URL"
        ))));
    }
    PgConfig::parse(url).map_err(|_| unparsable())
}

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
    /// The store [`migrate`](Self::migrate) runs on: the journal's own, unless a direct connection
    /// was given for it.
    migrations: DynStore,
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
    /// agents (`opts.concurrency` plus 3: a little headroom; the notifier's listener is a
    /// connection of its own beside it), and builds [`postgres`](Self::postgres) over it. The
    /// pool is separate from the orchestrator's, so a burst of steps cannot starve its store. A
    /// database that cannot be reached, or a connection string that does not parse, is a transient
    /// [`LocalAgentsError::Store`]; neither error holds the text.
    ///
    /// The connection strings are read as the rest of the orchestrator reads them
    /// (`orch-pg-connect`), except the notifier's listener's: `PgNotify` connects it itself, from
    /// adam-rs's own reading of the same string (`adam-postgres`).
    ///
    /// With `direct_url`, the database itself behind a transaction-mode pooler that
    /// `database_url` points at: the journal's queries use the pool (`concurrency + 2`
    /// connections), and what needs a session takes a pool of [`DIRECT_CONNECTIONS`] connections
    /// to `direct_url` and the listener's own connection to it, as
    /// [`postgres_direct`](Self::postgres_direct) does.
    pub async fn connect(
        database_url: &str,
        direct_url: Option<&str>,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        let headroom = if direct_url.is_some() { 2 } else { 3 };
        let size = u32::try_from(opts.concurrency.saturating_add(headroom)).unwrap_or(u32::MAX);
        let pool = connect_pool(database_url, size).await?;
        match direct_url {
            None => Self::postgres(pool, database_url, opts, kinds),
            Some(direct_url) => {
                let direct = connect_pool(direct_url, DIRECT_CONNECTIONS).await?;
                Self::postgres_direct(pool, direct, direct_url, opts, kinds)
            }
        }
    }

    /// Local agents whose journal is the Postgres behind `pool`, in tables prefixed
    /// [`TABLE_PREFIX`] and notified through the channels of the same prefix.
    ///
    /// `listen_url` is the same database, for the notifier's listener: it opens a connection of
    /// its own for as long as [`run`](Self::run) runs (a pooled connection never receives a
    /// notification), read by adam-rs's `adam-postgres`. A process that does not step opens none.
    ///
    /// The pool is dedicated: the notifier sends its `NOTIFY`s on it and each stepping run holds
    /// one of its connections briefly, so give it `concurrency + 3`. Call
    /// [`migrate`](Self::migrate) before first use. Fails on an invalid prefix, or a `listen_url`
    /// that does not parse.
    pub fn postgres(
        pool: Pool,
        listen_url: &str,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        Self::postgres_hosting(
            pool,
            listen_url,
            opts,
            kinds.iter().map(|k| k.hosted()).collect(),
        )
    }

    /// [`postgres`](Self::postgres) for a `pool` that goes through a transaction-mode pooler
    /// (PgBouncer): the journal's queries use `pool`, and the notifier and the creation of the
    /// tables take `direct`, a pool of connections to the database itself
    /// ([`DIRECT_CONNECTIONS`] are enough: a `NOTIFY` and the migration), and `direct_url`, the
    /// same database, which the notifier's listener connects to on its own. A `LISTEN` cannot be
    /// kept by a pooler; the `NOTIFY`s stay beside it on the direct pool.
    pub fn postgres_direct(
        pool: Pool,
        direct: Pool,
        direct_url: &str,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        Self::postgres_direct_hosting(
            pool,
            direct,
            direct_url,
            opts,
            kinds.iter().map(|k| k.hosted()).collect(),
        )
    }

    /// [`postgres_direct`](Self::postgres_direct) over any hosted set.
    pub(crate) fn postgres_direct_hosting(
        pool: Pool,
        direct: Pool,
        direct_url: &str,
        opts: LocalOptions,
        hosted: Vec<Hosted>,
    ) -> Result<Self, LocalAgentsError> {
        let schema = PgStore::from_pool(direct.clone())
            .with_table_prefix(TABLE_PREFIX)
            .map_err(LocalAgentsError::store)?;
        let mut agents = Self::postgres_hosting_with(pool, direct, direct_url, opts, hosted)?;
        agents.migrations = Arc::new(schema);
        Ok(agents)
    }

    /// [`postgres`](Self::postgres) over any hosted set: the testkit adds its scripted agent.
    pub(crate) fn postgres_hosting(
        pool: Pool,
        listen_url: &str,
        opts: LocalOptions,
        hosted: Vec<Hosted>,
    ) -> Result<Self, LocalAgentsError> {
        Self::postgres_hosting_with(pool.clone(), pool, listen_url, opts, hosted)
    }

    /// The journal over `pool`, the notifier over `notify_pool` and `listen_url` (the same pool
    /// and database unless a pooler is in front of the journal's).
    fn postgres_hosting_with(
        pool: Pool,
        notify_pool: Pool,
        listen_url: &str,
        opts: LocalOptions,
        hosted: Vec<Hosted>,
    ) -> Result<Self, LocalAgentsError> {
        let store = PgStore::from_pool(pool)
            .with_table_prefix(TABLE_PREFIX)
            .map_err(LocalAgentsError::store)?;
        let events = BroadcastSink::default();
        // A process that does not step (the control plane) runs no notifier: it would only
        // queue signals nobody sends.
        let notify = if opts.steps {
            // Events cross to the other processes whatever adam-rs's `NOTIFY_LOCAL_ONLY` says: the
            // orchestrator's roles share one database, and this crate reads no environment.
            Some(
                PgNotify::new(notify_pool, listener(listen_url)?, events.clone())
                    .local_only(false)
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

    /// Local agents whose journal is the SQLite file at `path` (the embedded build, ADR 0076):
    /// one process writes it, no server and no Postgres driver, in tables prefixed
    /// [`TABLE_PREFIX`]. Work is found by polling the journal (there is no `NOTIFY`), and a run
    /// survives the process: a new one resumes it from its last commit once the lease expires.
    /// Call [`migrate`](Self::migrate) before first use.
    #[cfg(feature = "embedded")]
    pub fn sqlite(
        path: impl AsRef<std::path::Path>,
        opts: LocalOptions,
        kinds: &[LocalKind],
    ) -> Result<Self, LocalAgentsError> {
        let store = adam_store_sqlite::SqliteStore::open(path)
            .and_then(|store| store.with_table_prefix(TABLE_PREFIX))
            .map_err(LocalAgentsError::store)?;
        Ok(Self::assemble(
            Arc::new(store),
            None,
            BroadcastSink::default(),
            opts,
            kinds.iter().map(|k| k.hosted()).collect(),
        ))
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
            migrations: store,
            notify,
            steps: opts.steps,
            entries: Arc::new(entries),
        }
    }

    /// Creates the journal's tables if they are missing. Idempotent, and safe to run from
    /// every replica at once: the store serialises it with an advisory lock of its own prefix.
    pub async fn migrate(&self) -> Result<(), LocalAgentsError> {
        self.migrations
            .migrate()
            .await
            .map_err(LocalAgentsError::store)
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
                .map_err(LocalAgentsError::worker);
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
