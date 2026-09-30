//! The HTTP edge of the orchestrator over [`orch_app::App`]: proxy-identity auth, RFC 9457
//! problems, the resource API (agents, thread list and details, cancel) and health, in
//! `docs/api/chat-api.yaml`.
//!
//! Interaction surfaces (AG-UI today) are separate crates. Each builds
//! [`SurfaceRoutes`], and [`router_with_surfaces`] mounts them behind the same identity layer,
//! so a surface cannot forget authentication.
//!
//! Identity comes from `X-Auth-Request-Email` (set by oauth2-proxy). Requests without it are
//! refused with 401 everywhere except `/healthz`, `/readyz` and `/metrics` (fail closed); the optional
//! `AUTH_DEV_USER` identity applies only when configured. **The identity header is only
//! trustworthy behind a proxy that strips client-supplied copies.**

mod auth;
mod extract;
mod metrics;
mod problem;
mod routes;
pub mod sse;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use orch_app::App;
use orch_ports::Ports;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub use auth::{AuthConfig, IDENTITY_HEADER};
pub use extract::{ApiJson, ApiQuery};
pub use problem::{ApiError, Problem};
pub use routes::parse_thread_id;

/// Everything configurable about the router.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Identity handling.
    pub auth: AuthConfig,
    /// Interval of the SSE `: keepalive` comment (15 s per the contract; shorter in tests).
    /// Not used by the resource API itself: surfaces read it and pass it to their streams.
    pub sse_keepalive: Duration,
    /// Request timeout for everything except the SSE stream.
    pub request_timeout: Duration,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            auth: AuthConfig::default(),
            sse_keepalive: Duration::from_secs(15),
            request_timeout: Duration::from_secs(30),
        }
    }
}

pub(crate) struct ApiState<P: Ports> {
    pub(crate) app: Arc<App<P>>,
}

impl<P: Ports> Clone for ApiState<P> {
    fn clone(&self) -> Self {
        ApiState {
            app: Arc::clone(&self.app),
        }
    }
}

/// The routes an interaction surface contributes, already bound to their own state.
///
/// `plain` routes get the request timeout; `streaming` routes (SSE) do not. Both sit behind
/// the identity layer once mounted by [`router_with_surfaces`], and a handler can take
/// `Extension<orch_core::UserId>`.
#[derive(Debug, Default)]
pub struct SurfaceRoutes {
    plain: Router,
    streaming: Router,
}

impl SurfaceRoutes {
    /// No routes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds request/response routes (subject to the request timeout).
    #[must_use]
    pub fn plain(mut self, routes: Router) -> Self {
        self.plain = self.plain.merge(routes);
        self
    }

    /// Adds streaming routes (no timeout).
    #[must_use]
    pub fn streaming(mut self, routes: Router) -> Self {
        self.streaming = self.streaming.merge(routes);
        self
    }
}

/// `GET /healthz`, `GET /readyz` and `GET /metrics`, bound to `state` and without any layer (no
/// identity, no timeout): the one definition shared by [`router_with_surfaces`] and
/// [`health_router`].
fn health_routes<P: Ports>(state: ApiState<P>) -> Router {
    Router::new()
        .route("/healthz", get(routes::healthz::<P>))
        .route("/readyz", get(routes::readyz::<P>))
        .route("/metrics", get(metrics::serve::<P>))
        .with_state(state)
}

/// The layers every served router carries: body limit, request tracing and request ids.
fn edge_layers(router: Router) -> Router {
    router
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// Builds a router that serves `/healthz`, `/readyz` and `/metrics` and nothing else, for a
/// process that runs no HTTP interface (a worker-only orchestrator) but must answer probes and
/// be scraped.
///
/// It is the same handlers, and the same tracing and request-id layers, as the health routes of
/// [`router_with_surfaces`]; like there, health and metrics need no identity. Every other path
/// is 404.
pub fn health_router<P: Ports>(app: Arc<App<P>>) -> Router {
    edge_layers(health_routes(ApiState { app }))
}

/// Builds the router for the resource API and health, with no interaction surface mounted.
pub fn router<P: Ports>(app: Arc<App<P>>, cfg: ApiConfig) -> Router {
    router_with_surfaces(app, cfg, Vec::new())
}

/// Builds the router for the resource API and health, plus the routes of `surfaces`.
///
/// # Panics
///
/// Like axum's `Router::merge`, when two surfaces (or a surface and the resource API) route
/// the same method and path.
pub fn router_with_surfaces<P: Ports>(
    app: Arc<App<P>>,
    cfg: ApiConfig,
    surfaces: Vec<SurfaceRoutes>,
) -> Router {
    let state = ApiState { app };
    let health = health_routes(state.clone());
    let resource = Router::new()
        .route("/api/agents", get(routes::list_agents::<P>))
        .route("/api/threads", get(routes::list_threads::<P>))
        .route("/api/threads/{thread_id}", get(routes::get_thread::<P>))
        .route(
            "/api/threads/{thread_id}/cancel",
            post(routes::cancel_thread::<P>),
        )
        .with_state(state);
    let mut plain = resource;
    let mut streaming = Router::new();
    for surface in surfaces {
        plain = plain.merge(surface.plain);
        streaming = streaming.merge(surface.streaming);
    }
    let plain = plain.layer(TimeoutLayer::with_status_code(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        cfg.request_timeout,
    ));
    // The identity layer wraps every non-health path, unknown ones included.
    let api = plain
        .merge(streaming)
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .layer(from_fn_with_state(
            Arc::new(cfg.auth),
            auth::require_identity,
        ));
    edge_layers(Router::new().merge(health).merge(api))
}
