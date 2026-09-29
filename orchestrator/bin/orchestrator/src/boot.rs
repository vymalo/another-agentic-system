//! The composition root: builds the concrete adapters, wires them into the application and
//! runs the components the role asks for until told to stop (ADR 0009, ADR 0015).
//!
//! No logic lives here. What the service does is in `orch-app` and `orch-api`, written against
//! the ports; this file only chooses the implementations (Postgres, A2A over HTTP, the system
//! clock) and sequences start and stop.
//!
//! It is built from shared setup plus the components that the role (`ORCH_ROLE`) selects:
//!
//! * **shared setup** ([`setup`]): the pool, migrations, wakeup, the A2A client, the agent
//!   directory and the [`App`], everything every role needs;
//! * **the control plane** ([`control_plane_router`], [`serve`]): the HTTP server (health, the
//!   resource API and the configured surfaces), which serves user inputs and event streams;
//! * **the worker** ([`dispatcher`], and for a worker-only process [`serve`] over the
//!   health-only router): the dispatcher, which delivers the outbox to agents.
//!
//! Every component is a future that ends when its `CancellationToken` is cancelled. [`run`]
//! registers them with [`adam_host::Host`], the supervisor every adam-rs host shares: it starts
//! only what the role asks for, treats the first component to end on its own as fatal, and stops
//! the control plane before the workers, each bounded by `SHUTDOWN_GRACE_SECS`.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use adam_host::Host;
use anyhow::Context;
use axum::Router;
use orch_agent_a2a::{A2aAgentClient, A2aConfig, install_crypto_provider};
use orch_api::{ApiConfig, AuthConfig, SurfaceRoutes};
use orch_app::{AgentDirectory, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_core::BoxError;
use orch_ports::{PortSet, SystemClock, UuidV7Ids};
use orch_store_postgres::{PgStore, PgWakeup};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, ConfigError, Surface};

/// How long the wakeup listener may take to attach before the service starts anyway. It keeps
/// retrying in the background, and consumers poll in the meantime.
const LISTEN_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Failures of a running service that the exit code tells apart (see `main::exit_code`). A
/// component that stops, ends early or panics is an [`adam_host::HostError`] instead, which the
/// exit code maps too.
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

/// What every role needs, built once by [`setup`].
struct Shared {
    /// Kept to close the pool at the very end.
    store: PgStore,
    app: Arc<App<Stack>>,
}

/// Connects, migrates and builds the [`App`] every role runs on.
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
    if cfg.role.runs_control_plane()
        && let Some(user) = &cfg.auth_dev_user
    {
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

/// Binds the listen address. Every role needs it: a control plane serves its API there, a worker
/// serves its probes.
async fn listen(cfg: &Config) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind(cfg.listen_addr)
        .await
        .map_err(|source| Fatal::Listen {
            addr: cfg.listen_addr,
            source,
        })?;
    let surfaces = if cfg.role.runs_control_plane() {
        cfg.surfaces
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join(",")
    } else {
        String::new()
    };
    tracing::info!(
        addr = %listener.local_addr().context("listener address")?,
        agents = cfg.agents.len(),
        surfaces = %surfaces,
        "orchestrator listening"
    );
    Ok(listener)
}

/// The control plane's router over `app`: health, the resource API and the configured surfaces.
fn control_plane_router(cfg: &Config, app: &Arc<App<Stack>>) -> Result<Router, ConfigError> {
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
    Ok(orch_api::router_with_surfaces(
        Arc::clone(app),
        api,
        surfaces,
    ))
}

/// Serves `router` on `listener` until `stop` is cancelled, then drains its connections.
async fn serve(
    listener: TcpListener,
    router: Router,
    stop: CancellationToken,
) -> Result<(), BoxError> {
    axum::serve(listener, router)
        .with_graceful_shutdown(stop.cancelled_owned())
        .await
        .map_err(|e| Box::new(e) as BoxError)
}

/// The worker's dispatcher over `app`. It runs until its token is cancelled, then stops its
/// workers and releases their leases.
fn dispatcher(cfg: &Config, app: &Arc<App<Stack>>) -> Arc<Dispatcher<Stack>> {
    let dispatcher_cfg = DispatcherConfig {
        concurrency: cfg.dispatcher_concurrency,
        lease: cfg.outbox_lease,
        heartbeat: cfg.outbox_lease / 3,
        ..DispatcherConfig::default()
    };
    Dispatcher::new(Arc::clone(app), dispatcher_cfg, cfg.instance_id.clone())
}

/// Marks the process as stopping: `/healthz` and `/readyz` turn 503 and open event streams end
/// once caught up, so clients reconnect elsewhere. Idempotent.
fn mark_stopping(app: &App<Stack>) {
    app.set_shutting_down();
    app.set_ready(false);
}

/// Calls [`mark_stopping`] when dropped. A component holds one, so a component that ends or
/// fails on its own (and is dropped, aborted or unwound) marks the process as stopping before
/// the supervisor begins to stop the others.
struct MarkStopping(Arc<App<Stack>>);

impl Drop for MarkStopping {
    fn drop(&mut self) {
        mark_stopping(&self.0);
    }
}

/// Connects, migrates and runs the components of `cfg.role` until `shutdown` resolves or one of
/// them ends on its own (then the result is an error, so the process exits non-zero and is
/// restarted), then stops gracefully:
///
/// 1. `/healthz` and `/readyz` turn 503 (see [`mark_stopping`]);
/// 2. the control plane is drained, then the workers are stopped, each for at most
///    `cfg.shutdown_grace` and aborted after that (the dispatcher releases its leases as it
///    stops); this is the `Host` stop order, and the server always goes first;
/// 3. the pool closes last.
///
/// | role | components |
/// |---|---|
/// | `all` | the HTTP server (control plane), the dispatcher (worker) |
/// | `control-plane` | the HTTP server; no dispatcher |
/// | `worker` | the dispatcher, and the health-only router on `LISTEN_ADDR` |
pub async fn run(cfg: Config, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    install_crypto_provider();

    let Shared { store, app } = setup(&cfg).await?;

    // Bind and build everything that can fail before any component starts.
    let listener = listen(&cfg).await?;
    let grace = Some(cfg.shutdown_grace);
    let host = Host::new(cfg.role)
        .control_plane_drain(grace)
        .worker_grace(grace);

    // Only a worker-only process runs the probes-only server; it is ended by the dispatcher
    // (see below), through this token.
    let probes_stop = CancellationToken::new();
    let host = if cfg.role.runs_control_plane() {
        let router = control_plane_router(&cfg, &app)?;
        let mark = MarkStopping(Arc::clone(&app));
        host.control_plane("the HTTP server", move |stop| async move {
            let _mark = mark;
            serve(listener, router, stop).await
        })
    } else {
        // The health router is a worker-tier component, so the host would cancel it together
        // with the dispatcher. It ignores that token and outlives the dispatcher instead: the
        // dispatcher cancels `probes_stop` as it ends, so probes answer (503) for the whole
        // drain and a liveness probe cannot kill a worker that is draining.
        let router = orch_api::health_router(Arc::clone(&app));
        let mark = MarkStopping(Arc::clone(&app));
        let stop = probes_stop.clone();
        host.worker("the health router", move |_host_stop| async move {
            let _mark = mark;
            serve(listener, router, stop).await
        })
    };

    let dispatcher = dispatcher(&cfg, &app);
    let mark = MarkStopping(Arc::clone(&app));
    let ready_when_started = (!cfg.role.runs_control_plane()).then(|| Arc::clone(&app));
    if ready_when_started.is_some() {
        // A worker is ready once the store is reachable (`/readyz` pings it) and its dispatcher
        // has started. The other roles start ready: the database is migrated and reachable.
        app.set_ready(false);
    }
    let probes_open = probes_stop.drop_guard();
    let host = host.worker("the dispatcher", move |stop| async move {
        let _mark = mark;
        // Dropped when the dispatcher ends, fails or is aborted: that ends the probes server.
        let _probes_open = probes_open;
        if let Some(app) = ready_when_started {
            app.set_ready(true);
        }
        dispatcher.run(stop).await;
        Ok(())
    });

    let stopping = Arc::clone(&app);
    let outcome = host
        .run(async move {
            shutdown.await;
            tracing::info!("shutting down");
            mark_stopping(&stopping);
        })
        .await;
    store.pool().close().await;
    tracing::info!("stopped");
    outcome.map_err(Into::into)
}
