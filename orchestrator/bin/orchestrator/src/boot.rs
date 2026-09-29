//! The composition root: builds the concrete adapters, wires them into the application and
//! runs the HTTP server and the dispatcher until told to stop (ADR 0009).
//!
//! No logic lives here. What the service does is in `orch-app` and `orch-api`, written against
//! the ports; this file only chooses the implementations (Postgres, A2A over HTTP, the system
//! clock) and sequences start and stop.
//!
//! It is built from three parts, so that the two halves of the service can be composed
//! separately:
//!
//! * **shared setup** ([`setup`]): the pool, migrations, wakeup, the A2A client, the agent
//!   directory and the [`App`], everything both halves need;
//! * **the control plane** ([`control_plane`]): the HTTP server (health, the resource API and
//!   the configured surfaces), which serves user inputs and event streams;
//! * **the worker** ([`worker`]): the dispatcher, which delivers the outbox to agents.
//!
//! Each half is a future that ends when its `CancellationToken` is cancelled. [`run`] starts
//! both and hands them to one supervisor ([`supervise`]) that knows nothing about either: it
//! treats the first half to end on its own as fatal, then drains the halves in order.

use std::future::{self, Future};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use anyhow::Context;
use orch_agent_a2a::{A2aAgentClient, A2aConfig, install_crypto_provider};
use orch_api::{ApiConfig, AuthConfig, SurfaceRoutes};
use orch_app::{AgentDirectory, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_core::BoxError;
use orch_ports::{PortSet, SystemClock, UuidV7Ids};
use orch_store_postgres::{PgStore, PgWakeup};
use tokio::net::TcpListener;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::config::{Config, ConfigError, Surface};

/// How long the wakeup listener may take to attach before the service starts anyway. It keeps
/// retrying in the background, and consumers poll in the meantime.
const LISTEN_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Failures of a running service that the exit code tells apart (see `main::exit_code`).
#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    /// The listen address could not be bound.
    #[error("cannot listen on {addr}")]
    Listen {
        /// The configured address.
        addr: SocketAddr,
        /// Why the bind failed.
        #[source]
        source: io::Error,
    },
    /// One half of the service (the HTTP server or the dispatcher) ended, failed or panicked
    /// while the other was still running.
    #[error("{half} stopped unexpectedly")]
    Stopped {
        /// Which half.
        half: &'static str,
        /// The error or panic that ended it, if it reported one.
        #[source]
        source: Option<BoxError>,
    },
}

/// The routes of one surface, built from its adapter crate.
///
/// The configuration refuses a surface that is not compiled in, so the error arm below is a
/// second line of defence, not a path a running service takes.
fn surface_routes<P: orch_ports::Ports>(
    surface: Surface,
    app: &Arc<App<P>>,
    sse_keepalive: Duration,
) -> Result<SurfaceRoutes, ConfigError> {
    match surface {
        #[cfg(feature = "surface-chat-api")]
        Surface::ChatApi => Ok(orch_surface_chat_api::routes(
            Arc::clone(app),
            sse_keepalive,
        )),
        #[cfg(not(feature = "surface-chat-api"))]
        Surface::ChatApi => {
            let _ = (app, sse_keepalive);
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
    }
}

type Stack = PortSet<PgStore, PgWakeup, A2aAgentClient, SystemClock, UuidV7Ids>;

/// What every half needs, built once by [`setup`].
struct Shared {
    /// Kept to close the pool at the very end.
    store: PgStore,
    app: Arc<App<Stack>>,
}

/// The future of one half: it ends when its token is cancelled, or on its own if it fails.
type HalfFuture = Pin<Box<dyn Future<Output = Result<(), BoxError>> + Send + 'static>>;

/// One half of the service, running as a task the supervisor can stop and wait for.
struct Half {
    /// Names the half in [`Fatal::Stopped`].
    name: &'static str,
    /// Logged when the half does not stop within the grace period and is aborted.
    on_timeout: &'static str,
    stop: CancellationToken,
    task: JoinHandle<Result<(), BoxError>>,
}

impl Half {
    fn spawn(
        name: &'static str,
        on_timeout: &'static str,
        stop: CancellationToken,
        future: HalfFuture,
    ) -> Self {
        Half {
            name,
            on_timeout,
            stop,
            task: tokio::spawn(future),
        }
    }
}

/// Connects, migrates and builds the [`App`] both halves run on.
async fn setup(cfg: &Config) -> anyhow::Result<Shared> {
    let store = PgStore::connect_with(&cfg.database_url, cfg.database_max_connections)
        .await
        .context("cannot connect to Postgres (DATABASE_URL)")?;
    // Idempotent and serialised by sqlx's advisory lock: every replica may run it at boot.
    store
        .migrate()
        .await
        .context("cannot apply the database migrations")?;
    tracing::info!("database migrations applied");

    let pool = store.pool().clone();
    let wakeup = PgWakeup::start(pool);
    if !wakeup.wait_listening(LISTEN_ATTACH_TIMEOUT).await {
        tracing::warn!("the wakeup listener is not attached yet; falling back to polling");
    }

    let agents =
        A2aAgentClient::new(A2aConfig::default()).context("cannot build the A2A client")?;
    for agent in &cfg.agents {
        let e = &agent.endpoint;
        if e.bearer.is_some() && e.card_url.starts_with("http://") {
            tracing::warn!(agent = %e.id, "a bearer token is sent to this agent over plain http");
        }
    }
    if let Some(user) = &cfg.auth_dev_user {
        tracing::warn!(
            %user,
            "AUTH_DEV_USER is set: requests without X-Auth-Request-Email are served as this user. \
             Never use this in production."
        );
    }

    // The database is migrated and reachable by now, so the app starts ready.
    let app: Arc<App<Stack>> = Arc::new(App::new(
        PortSet {
            store: store.clone(),
            wakeup,
            agents,
            clock: SystemClock,
            ids: UuidV7Ids,
        },
        AgentDirectory::new(cfg.agents.clone()),
        AppConfig::default(),
    ));
    Ok(Shared { store, app })
}

/// Binds the listen address; only a process that runs the control plane needs it.
async fn listen(cfg: &Config) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind(cfg.listen_addr)
        .await
        .map_err(|source| Fatal::Listen {
            addr: cfg.listen_addr,
            source,
        })?;
    tracing::info!(
        addr = %listener.local_addr().context("listener address")?,
        instance = %cfg.instance_id,
        agents = cfg.agents.len(),
        surfaces = %cfg.surfaces.iter().map(|s| s.name()).collect::<Vec<_>>().join(","),
        "orchestrator listening"
    );
    Ok(listener)
}

/// The control plane: the HTTP server over `app` (health, the resource API and the configured
/// surfaces), serving until `stop` is cancelled and then draining its connections.
fn control_plane(
    cfg: &Config,
    app: &Arc<App<Stack>>,
    listener: TcpListener,
    stop: CancellationToken,
) -> Result<HalfFuture, ConfigError> {
    let api = ApiConfig {
        auth: AuthConfig {
            dev_user: cfg.auth_dev_user.clone(),
        },
        ..ApiConfig::default()
    };
    if cfg.surfaces.is_empty() {
        tracing::warn!(
            "no interaction surface is mounted: only the resource API and health are served"
        );
    }
    let surfaces = cfg
        .surfaces
        .iter()
        .map(|&surface| surface_routes(surface, app, api.sse_keepalive))
        .collect::<Result<Vec<_>, _>>()?;
    let router = orch_api::router_with_surfaces(Arc::clone(app), api, surfaces);
    Ok(Box::pin(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
            .map_err(|e| Box::new(e) as BoxError)
    }))
}

/// The worker: the dispatcher over `app`, running until `stop` is cancelled. It then stops its
/// workers and releases their leases.
fn worker(cfg: &Config, app: &Arc<App<Stack>>, stop: CancellationToken) -> HalfFuture {
    let dispatcher_cfg = DispatcherConfig {
        concurrency: cfg.dispatcher_concurrency,
        lease: cfg.outbox_lease,
        heartbeat: cfg.outbox_lease / 3,
        ..DispatcherConfig::default()
    };
    let dispatcher = Dispatcher::new(Arc::clone(app), dispatcher_cfg, cfg.instance_id.clone());
    Box::pin(async move {
        dispatcher.run(stop).await;
        Ok(())
    })
}

/// Resolves with the name and outcome of the first half whose task ends. Pending forever when
/// there are no halves, so that only the shutdown signal can end a supervisor with none.
async fn first_to_end(
    halves: &mut [Half],
) -> (&'static str, Result<Result<(), BoxError>, JoinError>) {
    future::poll_fn(|cx| {
        for half in halves.iter_mut() {
            if let Poll::Ready(outcome) = Pin::new(&mut half.task).poll(cx) {
                return Poll::Ready((half.name, outcome));
            }
        }
        Poll::Pending
    })
    .await
}

/// Runs the halves until `shutdown` resolves or one of them ends on its own (then the result
/// is an error, so the process exits non-zero and is restarted), then stops gracefully:
///
/// 1. `/healthz` and `/readyz` turn 503 and open event streams end once caught up, so clients
///    reconnect elsewhere;
/// 2. every half is told to stop;
/// 3. the halves are awaited in the order given, each for at most `grace` and aborted after
///    that. Order them so that the one that must drain first comes first (the server before the
///    dispatcher, which releases its leases as it stops).
async fn supervise(
    app: &App<Stack>,
    mut halves: Vec<Half>,
    shutdown: impl Future<Output = ()>,
    grace: Duration,
) -> Result<(), Fatal> {
    let failure = tokio::select! {
        () = shutdown => None,
        (name, outcome) = first_to_end(&mut halves) => Some(Fatal::Stopped {
            half: name,
            source: match outcome {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(join) => Some(Box::new(join) as BoxError),
            },
        }),
    };
    tracing::info!("shutting down");

    app.set_shutting_down();
    app.set_ready(false);
    for half in &halves {
        half.stop.cancel();
    }
    for half in &mut halves {
        if !half.task.is_finished() && tokio::time::timeout(grace, &mut half.task).await.is_err() {
            tracing::warn!("{}", half.on_timeout);
            half.task.abort();
        }
    }
    match failure {
        Some(fatal) => Err(fatal),
        None => Ok(()),
    }
}

/// Connects, migrates and serves until `shutdown` resolves, then stops gracefully (see
/// [`supervise`]); the pool closes last. Each step is bounded by `cfg.shutdown_grace`.
///
/// Runs both halves. A later change can run only one of them by building only its future.
pub async fn run(cfg: Config, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    install_crypto_provider();

    let Shared { store, app } = setup(&cfg).await?;

    // Bind and build everything that can fail before any half starts.
    let listener = listen(&cfg).await?;
    let stop_server = CancellationToken::new();
    let stop_dispatcher = CancellationToken::new();
    let server = control_plane(&cfg, &app, listener, stop_server.clone())?;
    let dispatcher = worker(&cfg, &app, stop_dispatcher.clone());

    let halves = vec![
        Half::spawn(
            "the HTTP server",
            "connections did not drain in time; closing them",
            stop_server,
            server,
        ),
        Half::spawn(
            "the dispatcher",
            "the dispatcher did not stop in time; its leases will lapse on their own",
            stop_dispatcher,
            dispatcher,
        ),
    ];
    let outcome = supervise(&app, halves, shutdown, cfg.shutdown_grace).await;
    store.pool().close().await;
    tracing::info!("stopped");
    outcome.map_err(Into::into)
}
