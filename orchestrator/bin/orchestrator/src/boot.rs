//! The composition root: builds the concrete adapters, wires them into the application and
//! runs the HTTP server and the dispatcher until told to stop (ADR 0009).
//!
//! No logic lives here. What the service does is in `orch-app` and `orch-api`, written against
//! the ports; this file only chooses the implementations (Postgres, A2A over HTTP, the system
//! clock) and sequences start and stop.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
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

/// Connects, migrates and serves until `shutdown` resolves, then stops gracefully:
///
/// 1. `/healthz` and `/readyz` turn 503 and open event streams end once caught up, so clients
///    reconnect elsewhere;
/// 2. the server stops accepting connections and drains the in-flight ones;
/// 3. the dispatcher stops its workers and releases their leases, so another replica takes
///    the threads over immediately instead of after the lease expires;
/// 4. the pool closes.
///
/// Each step is bounded by `cfg.shutdown_grace`.
pub async fn run(cfg: Config, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    install_crypto_provider();

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
    let wakeup = PgWakeup::start(pool.clone());
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
            store,
            wakeup,
            agents,
            clock: SystemClock,
            ids: UuidV7Ids,
        },
        AgentDirectory::new(cfg.agents.clone()),
        AppConfig::default(),
    ));

    let dispatcher_cfg = DispatcherConfig {
        concurrency: cfg.dispatcher_concurrency,
        lease: cfg.outbox_lease,
        heartbeat: cfg.outbox_lease / 3,
        ..DispatcherConfig::default()
    };
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

    let stop_dispatcher = CancellationToken::new();
    let stop_server = CancellationToken::new();
    let mut dispatcher = tokio::spawn(
        Dispatcher::new(Arc::clone(&app), dispatcher_cfg, cfg.instance_id.clone())
            .run(stop_dispatcher.clone()),
    );
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
        .map(|&surface| surface_routes(surface, &app, api.sse_keepalive))
        .collect::<Result<Vec<_>, _>>()?;
    let router = orch_api::router_with_surfaces(Arc::clone(&app), api, surfaces);
    let mut server = tokio::spawn({
        let stop = stop_server.clone();
        async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
        }
    });

    // Run until the signal, or until one half dies (then exit non-zero so it is restarted).
    let failure = tokio::select! {
        () = shutdown => None,
        r = &mut server => Some(Fatal::Stopped {
            half: "the HTTP server",
            source: match r {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(Box::new(e) as BoxError),
                Err(join) => Some(Box::new(join) as BoxError),
            },
        }),
        r = &mut dispatcher => Some(Fatal::Stopped {
            half: "the dispatcher",
            source: r.err().map(|join| Box::new(join) as BoxError),
        }),
    };
    tracing::info!("shutting down");

    app.set_shutting_down();
    app.set_ready(false);
    stop_server.cancel();
    stop_dispatcher.cancel();

    let grace = cfg.shutdown_grace;
    if !server.is_finished() && tokio::time::timeout(grace, &mut server).await.is_err() {
        tracing::warn!("connections did not drain in time; closing them");
        server.abort();
    }
    if !dispatcher.is_finished() && tokio::time::timeout(grace, &mut dispatcher).await.is_err() {
        tracing::warn!("the dispatcher did not stop in time; its leases will lapse on their own");
        dispatcher.abort();
    }
    pool.close().await;
    tracing::info!("stopped");

    match failure {
        Some(fatal) => Err(fatal.into()),
        None => Ok(()),
    }
}
