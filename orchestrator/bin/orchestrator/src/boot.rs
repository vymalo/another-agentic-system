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
//! * **the worker** ([`dispatcher`], [`inbox_worker`], and for a worker-only process [`serve`]
//!   over the health-only router): the dispatcher, which delivers the outbox to agents, and the
//!   inbox worker, which applies timers and reports to their threads.
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
use orch_agent_a2a::{A2aAgentClient, A2aConfig, FileFetch, install_crypto_provider};
use orch_api::{ApiConfig, SurfaceRoutes};
use orch_app::{AgentDirectory, App, Dispatcher, DispatcherConfig, InboxWorker};
use orch_core::BoxError;
use orch_ports::{
    AgentTransport, CompositeRegistry, FixedRegistry, PortSet, SystemClock, UuidV7Ids,
};
use orch_store_postgres::{PgStore, PgWakeup};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::artifacts::ConfiguredArtifacts;
use crate::auth::ConfiguredAuth;
use crate::config::{Config, ConfigError, Surface};
use crate::local::{self, Agents, Local};
use crate::model::ConfiguredModel;

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
/// The configuration refuses a surface that is not compiled in, and a webhook surface without
/// its secrets, so the error arms below are a second line of defence, not paths a running
/// service takes.
fn surface_routes<P: orch_ports::Ports>(
    surface: Surface,
    app: &Arc<App<P>>,
    sse_keepalive: Duration,
    cfg: &Config,
) -> Result<SurfaceRoutes, ConfigError> {
    match surface {
        #[cfg(feature = "surface-agui")]
        Surface::Agui => {
            let _ = cfg;
            Ok(orch_surface_agui::routes(Arc::clone(app), sse_keepalive))
        }
        #[cfg(not(feature = "surface-agui"))]
        Surface::Agui => {
            let _ = (cfg, app, sse_keepalive);
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
        #[cfg(feature = "surface-webhook")]
        Surface::WebhookGeneric => {
            let _ = sse_keepalive;
            let generic = cfg
                .webhook_generic
                .clone()
                .ok_or(ConfigError::MissingForSurface {
                    var: "WEBHOOK_GENERIC_SECRETS",
                    surface: surface.name(),
                })?;
            Ok(orch_surface_webhook::generic::routes(
                Arc::clone(app),
                generic,
            ))
        }
        #[cfg(feature = "surface-webhook")]
        Surface::WebhookGithub => {
            let _ = sse_keepalive;
            let github = cfg
                .webhook_github
                .clone()
                .ok_or(ConfigError::MissingForSurface {
                    var: "WEBHOOK_GITHUB_SECRETS",
                    surface: surface.name(),
                })?;
            Ok(orch_surface_webhook::github::routes(
                Arc::clone(app),
                github,
            ))
        }
        #[cfg(not(feature = "surface-webhook"))]
        Surface::WebhookGeneric | Surface::WebhookGithub => {
            let _ = (cfg, app, sse_keepalive);
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
        #[cfg(feature = "surface-mcp")]
        Surface::Mcp => mcp_routes(app, cfg),
        #[cfg(not(feature = "surface-mcp"))]
        Surface::Mcp => {
            let _ = (app, cfg);
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
        #[cfg(feature = "surface-thread-tools")]
        Surface::ThreadTools => thread_tools_routes(app, cfg),
        #[cfg(not(feature = "surface-thread-tools"))]
        Surface::ThreadTools => {
            let _ = (app, cfg);
            Err(ConfigError::SurfaceNotCompiled {
                surface: surface.name(),
                feature: surface.feature(),
            })
        }
    }
}

/// The MCP server's routes. The tokens, hosts and public URL were validated when the
/// configuration was read; the crate checks them again, and a failure is the same kind of error
/// (exit 78), not a panic.
#[cfg(feature = "surface-mcp")]
fn mcp_routes<P: orch_ports::Ports>(
    app: &Arc<App<P>>,
    cfg: &Config,
) -> Result<SurfaceRoutes, ConfigError> {
    let invalid = |var: &'static str| {
        move |e: &dyn std::fmt::Display| ConfigError::Invalid {
            var,
            reason: e.to_string(),
        }
    };
    // Only reachable with the surface mounted, which the configuration reads the settings for.
    let settings = cfg
        .mcp
        .as_ref()
        .ok_or(ConfigError::Missing("MCP_TOKENS_FILE"))?;
    let tokens = orch_surface_mcp::TokenTable::with_roles(settings.tokens.iter().cloned())
        .map_err(|e| invalid("MCP_TOKENS_FILE")(&e))?;
    let mut config =
        orch_surface_mcp::McpConfig::new(tokens, settings.allowed_hosts.iter().cloned())
            .map_err(|e| invalid("MCP_ALLOWED_HOSTS")(&e))?
            .with_wait_max(settings.wait_max)
            .map_err(|e| invalid("MCP_WAIT_MAX_SECS")(&e))?;
    config = config
        .with_wait_limits(settings.wait_max_concurrent, settings.wait_max_per_user)
        .map_err(|e| invalid("MCP_WAIT_MAX_CONCURRENT")(&e))?
        .with_allowed_origins(settings.allowed_origins.iter().cloned())
        .map_err(|e| invalid("MCP_ALLOWED_ORIGINS")(&e))?;
    if let Some(url) = &settings.public_url {
        config = config
            .with_public_url(url)
            .map_err(|e| invalid("ORCH_PUBLIC_URL")(&e))?;
    }
    tracing::info!(
        tokens = settings.tokens.len(),
        allowed_hosts = %settings.allowed_hosts.join(","),
        web_url = settings.public_url.is_some(),
        wait_max_secs = settings.wait_max.as_secs(),
        wait_max_concurrent = settings.wait_max_concurrent,
        wait_max_per_user = settings.wait_max_per_user,
        allowed_origins = settings.allowed_origins.len(),
        "the MCP server is mounted at /mcp"
    );
    Ok(orch_surface_mcp::routes(Arc::clone(app), config))
}

/// The thread-tools endpoint's routes. The keys and hosts were validated when the configuration
/// was read; the crate checks the hosts again, and a failure is the same kind of error (exit 78),
/// not a panic.
#[cfg(feature = "surface-thread-tools")]
fn thread_tools_routes<P: orch_ports::Ports>(
    app: &Arc<App<P>>,
    cfg: &Config,
) -> Result<SurfaceRoutes, ConfigError> {
    // Only reachable with the surface mounted, which the configuration reads the settings for.
    let settings = cfg
        .thread_tools
        .as_ref()
        .ok_or(ConfigError::Missing("THREAD_TOOLS_SECRET"))?;
    let config = orch_surface_thread_tools::ThreadToolsConfig::new(
        settings.issuer.keys().clone(),
        settings.allowed_hosts.iter().cloned(),
    )
    .map_err(|e| ConfigError::Invalid {
        var: "THREAD_TOOLS_ALLOWED_HOSTS",
        reason: e.to_string(),
    })?;
    let config = with_relay(app, cfg, config)?;
    // `ask_agent` (ADR 0026): the addressed agent asks an agent the person mentioned. It is part of
    // the surface, not a feature of its own: it is offered only for a job in which the person
    // mentioned someone, and the limits are `asks` of the configuration.
    let config = config.with_provider(orch_surface_thread_tools::AskTools::new(Arc::clone(app)));
    tracing::info!(
        ask_max_depth = cfg.asks.depth,
        ask_max_per_job = cfg.asks.per_job,
        ask_max_running = cfg.asks.running,
        ask_timeout_secs = cfg.asks.timeout.as_secs(),
        "ask_agent is offered on the thread-tools endpoint"
    );
    tracing::info!(
        allowed_hosts = %settings.allowed_hosts.join(","),
        "the thread-tools endpoint is mounted at {}",
        orch_surface_thread_tools::ROUTE
    );
    Ok(orch_surface_thread_tools::routes(Arc::clone(app), config))
}

/// The relay of the attached MCP servers (ADR 0024), a provider of the endpoint: the deployment's
/// `toolServers`, each with its endpoint (the URL and the credentials the configuration resolved),
/// called through the MCP client. Nothing is added when no server is listed.
#[cfg(feature = "tool-relay")]
fn with_relay<P: orch_ports::Ports>(
    app: &Arc<App<P>>,
    cfg: &Config,
    config: orch_surface_thread_tools::ThreadToolsConfig,
) -> Result<orch_surface_thread_tools::ThreadToolsConfig, ConfigError> {
    if cfg.tool_servers.is_empty() {
        return Ok(config);
    }
    let invalid = |reason: String| ConfigError::Invalid {
        var: "toolServers",
        reason,
    };
    let servers = cfg
        .tool_servers
        .iter()
        .map(|info| {
            let endpoint = cfg
                .tool_endpoints
                .iter()
                .find(|e| e.id == info.id)
                .cloned()
                .ok_or_else(|| invalid(format!("the server {:?} has no endpoint", info.id)))?;
            Ok((info.clone(), endpoint))
        })
        .collect::<Result<Vec<_>, ConfigError>>()?;
    let client = orch_tools_mcp::McpToolClient::new().map_err(|e| invalid(e.to_string()))?;
    let relay = orch_surface_thread_tools::RelayTools::new(Arc::clone(app), client, servers)
        .map_err(|e| invalid(e.to_string()))?;
    tracing::info!(
        servers = %cfg.tool_servers.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(","),
        "the attached MCP servers are relayed on the thread-tools endpoint"
    );
    Ok(config.with_provider(relay))
}

/// Without the feature the servers a deployment lists can be attached and no agent can use them:
/// the operator is told once, at startup.
#[cfg(all(feature = "surface-thread-tools", not(feature = "tool-relay")))]
fn with_relay<P: orch_ports::Ports>(
    _app: &Arc<App<P>>,
    cfg: &Config,
    config: orch_surface_thread_tools::ThreadToolsConfig,
) -> Result<orch_surface_thread_tools::ThreadToolsConfig, ConfigError> {
    if !cfg.tool_servers.is_empty() {
        tracing::warn!(
            "toolServers are listed, but this build has no relay (Cargo feature `tool-relay`): attached servers give an agent no tools"
        );
    }
    Ok(config)
}

/// The platform's registry type: the real one with the feature `registry-platform`, else a type
/// that exists and is never built (the registry is then always `None`).
#[cfg(feature = "registry-platform")]
type Platform = orch_registry_platform::PlatformRegistry;
#[cfg(not(feature = "registry-platform"))]
type Platform = FixedRegistry;

/// The agents a person can pick (ADR 0022): the deployment's own list, then the platform's
/// registry when `AGENT_REGISTRY_URL` is set. Static dispatch over both, one build either way.
type Registry = CompositeRegistry<FixedRegistry, Option<Platform>>;

type Stack = PortSet<
    PgStore,
    PgWakeup,
    Agents,
    SystemClock,
    UuidV7Ids,
    ConfiguredModel,
    Registry,
    ConfiguredAuth,
    ConfiguredArtifacts,
>;

/// The platform's registry client, when `AGENT_REGISTRY_URL` is set.
#[cfg(feature = "registry-platform")]
fn platform_registry(cfg: &Config) -> Result<Option<Platform>, ConfigError> {
    use orch_registry_platform::{BuildError, PlatformConfig, PlatformRegistry};

    let Some(settings) = &cfg.registry else {
        return Ok(None);
    };
    let mut client = PlatformConfig::new(settings.url.clone())
        .with_timeout(settings.timeout)
        .with_max_age(settings.max_age);
    if let Some(token) = &settings.token {
        client = client.with_token(token.clone());
    }
    if let Some(token) = &settings.agent_token {
        client = client.with_agent_token(token.clone());
    }
    if settings.url.starts_with("http://")
        && (settings.token.is_some() || settings.agent_token.is_some())
    {
        tracing::warn!("a bearer token is sent to the agent registry over plain http");
    }
    let registry = PlatformRegistry::new(client).map_err(|e| ConfigError::Invalid {
        var: match e {
            BuildError::BadToken("agent") => "AGENT_REGISTRY_AGENT_TOKEN",
            BuildError::BadToken(_) => "AGENT_REGISTRY_TOKEN",
            BuildError::BadUrl | BuildError::Http(_) => "AGENT_REGISTRY_URL",
        },
        reason: e.to_string(),
    })?;
    tracing::info!(
        agents_file = cfg.agents.len(),
        timeout_secs = settings.timeout.as_secs(),
        max_age_secs = settings.max_age.as_secs(),
        "the agent registry of the platform is read live beside AGENTS_FILE"
    );
    Ok(Some(registry))
}

/// Without the feature there is no registry (a URL for one was refused when the configuration
/// was read).
#[cfg(not(feature = "registry-platform"))]
fn platform_registry(_cfg: &Config) -> Result<Option<Platform>, ConfigError> {
    Ok(None)
}

/// The A2A client's configuration: the defaults, and the issuer of the thread-tools grants when the
/// keys and the URL are set (every role has them: the worker sends, the control plane serves).
fn a2a_config(cfg: &Config) -> A2aConfig {
    #[allow(unused_mut)]
    let mut a2a = A2aConfig::default();
    #[cfg(feature = "surface-thread-tools")]
    if let Some(settings) = &cfg.thread_tools {
        tracing::info!(
            url = settings.issuer.base_url(),
            ttl_secs = settings.issuer.ttl().as_secs(),
            keys = ?settings.issuer.keys(),
            "agents that list thread-tools/v1 are given a grant"
        );
        a2a.thread_tools = Some(Arc::clone(&settings.issuer));
        // The endpoint the grants open offers `ask_agent` (see `thread_tools_routes`), so an agent
        // that lists `mentions/v1` and `thread-tools/v1` is told it may coordinate the agents the
        // person mentioned with it.
        a2a.asks = true;
    }
    // A `url` part of an agent's artifact is read only from the hosts the operator listed, and only
    // where there is a store to keep it in (ADR 0032).
    if let Some(artifacts) = cfg.artifacts.as_ref().filter(|a| !a.fetch_hosts.is_empty()) {
        tracing::info!(
            hosts = ?artifacts.fetch_hosts,
            "an agent's url parts are fetched from these hosts"
        );
        a2a.fetch_files = Some(FileFetch::new(
            artifacts.fetch_hosts.clone(),
            artifacts.max_file_bytes,
        ));
    }
    let _ = cfg;
    a2a
}

/// What every role needs, built once by [`setup`].
struct Shared {
    /// Kept to close the pool at the very end.
    store: PgStore,
    app: Arc<App<Stack>>,
    /// The local agents (`transport: local`), when `AGENTS_FILE` lists any.
    local: Option<Local>,
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

    let a2a = A2aAgentClient::new(a2a_config(cfg)).context("cannot build the A2A client")?;
    // After the orchestrator's own migrations, before the app: every role creates the local
    // agents' journal too (idempotent, and serialised by its own advisory lock).
    let local = Local::start(cfg).await?;
    let agents = local::compose(a2a, local.as_ref());
    for agent in &cfg.agents {
        let e = &agent.endpoint;
        match &e.transport {
            AgentTransport::A2a { card_url, bearer }
                if bearer.is_some() && card_url.starts_with("http://") =>
            {
                tracing::warn!(agent = %e.id, "a bearer token is sent to this agent over plain http");
            }
            AgentTransport::A2a { .. } | AgentTransport::Local { .. } => {}
        }
    }
    let auth = ConfiguredAuth::build(cfg)?;
    if cfg.role.runs_control_plane() {
        tracing::info!(mode = auth.describe(), "requests are authenticated");
        if let Some(user) = &cfg.auth_dev_user {
            tracing::warn!(
                %user,
                "AUTH_DEV_USER is set: requests without X-Auth-Request-Email are served as this user. \
                 Never use this in production."
            );
        }
        if cfg.auth.mode.reads_header() {
            tracing::warn!(
                mode = cfg.auth.mode.as_str(),
                "X-Auth-Request-Email is trusted: this is safe only behind a proxy that strips the \
                 copies a client sends; auth.mode jwt validates tokens instead (ADR 0033)"
            );
        }
    }

    // The database is migrated and reachable by now, so the app starts ready.
    let directory = AgentDirectory::new(cfg.agents.clone());
    let platform = platform_registry(cfg)?;
    // Every role: a worker keeps the files an agent hands over, the control plane serves them.
    let artifacts = ConfiguredArtifacts::build(cfg.artifacts.as_ref())
        .await
        .map_err(ConfigError::Artifacts)?;
    let app: Arc<App<Stack>> = Arc::new(
        App::new(
            PortSet {
                artifacts,
                store: store.clone(),
                wakeup,
                agents,
                clock: SystemClock,
                ids: UuidV7Ids,
                model: ConfiguredModel::build(&cfg.models)
                    .context("cannot build the model endpoints (models.endpoints)")?,
                auth,
                registry: CompositeRegistry::new(directory.fixed_registry(), platform),
            },
            directory,
            cfg.app_config(),
        )
        .map_err(|e| ConfigError::Gate {
            context: "the verification gate",
            reason: e.to_string(),
        })?,
    );
    Ok(Shared { store, app, local })
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
    let api = ApiConfig::default();
    if cfg.surfaces.is_empty() {
        tracing::warn!(
            "no interaction surface is mounted: only the resource API and health are served"
        );
    }
    let surfaces = cfg
        .surfaces
        .iter()
        .map(|&surface| surface_routes(surface, app, api.sse_keepalive, cfg))
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
        verify_watch: cfg.verifier_watch,
        ..DispatcherConfig::default()
    };
    Dispatcher::new(Arc::clone(app), dispatcher_cfg, cfg.instance_id.clone())
}

/// The worker's inbox worker over `app`: it applies the timers that came due and the reports
/// that webhooks stored. It runs until its token is cancelled, then releases its leases.
fn inbox_worker(cfg: &Config, app: &Arc<App<Stack>>) -> Arc<InboxWorker<Stack>> {
    InboxWorker::new(Arc::clone(app), cfg.inbox.clone(), cfg.instance_id.clone())
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
///    `cfg.shutdown_grace` and aborted after that (the dispatcher and the inbox worker
///    release their leases as they stop); this is the `Host` stop order, and the server always goes first;
/// 3. the pool closes last.
///
/// | role | components |
/// |---|---|
/// | `all` | the HTTP server (control plane), the dispatcher, the inbox worker and the local agents' worker (workers) |
/// | `control-plane` | the HTTP server; no dispatcher, no inbox worker, no local agents' worker |
/// | `worker` | the dispatcher, the inbox worker, the local agents' worker, and the health-only router on `LISTEN_ADDR` |
///
/// The local agents' worker exists only in a build with the feature `agent-local`, and only
/// when `AGENTS_FILE` lists a `transport: local` agent (see [`crate::local`]).
pub async fn run(cfg: Config, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    install_crypto_provider();

    let Shared { store, app, local } = setup(&cfg).await?;

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

    let inbox = inbox_worker(&cfg, &app);
    let mark = MarkStopping(Arc::clone(&app));
    let host = host.worker("the inbox worker", move |stop| async move {
        let _mark = mark;
        inbox.run(stop).await;
        Ok(())
    });

    // The local agents' worker (roles that run workers) steps runs beside the dispatcher.
    let host = match &local {
        Some(local) => local.register(host),
        None => host,
    };

    let stopping = Arc::clone(&app);
    let outcome = host
        .run(async move {
            shutdown.await;
            tracing::info!("shutting down");
            mark_stopping(&stopping);
        })
        .await;
    if let Some(local) = local {
        local.close().await;
    }
    store.pool().close().await;
    tracing::info!("stopped");
    outcome.map_err(Into::into)
}
