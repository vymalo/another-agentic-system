//! The HTTP edge of the orchestrator over [`orch_app::App`]: authentication through the
//! `Authenticator` port, RFC 9457 problems, the resource API (agents, thread list and details, export, cancel, fork, branches) and health, in
//! `docs/api/chat-api.yaml`.
//!
//! Interaction surfaces (AG-UI, MCP) are separate crates. Each builds [`SurfaceRoutes`], and
//! [`router_with_surfaces`] mounts them behind the same identity layer, so a surface cannot forget
//! authentication. The one exception is a [machine route](SurfaceRoutes::machine): a route for a
//! caller that has no oauth2-proxy cookie (an MCP client with a bearer token), which the surface
//! guards itself.
//!
//! Identity is whatever the application's `Authenticator` ([ADR 0033]) makes of the request's
//! credentials: the `Authorization: Bearer` token, and the `X-Auth-Request-Email` header
//! oauth2-proxy sets. Requests it does not authenticate are refused everywhere except `/healthz`,
//! `/readyz` and `/metrics` (fail closed): 401, with `WWW-Authenticate: Bearer` when a bearer token
//! is a credential it reads, or 503 when it cannot tell (the token issuer's keys cannot be
//! fetched). `/readyz` is 503 while it cannot authenticate. **The identity header is only
//! trustworthy behind a proxy that strips client-supplied copies.**
//!
//! [ADR 0033]: ../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md

mod artifacts;
mod auth;
mod export;
mod extract;
mod host;
pub mod limiter;
mod metrics;
mod problem;
mod routes;
mod shared;
pub mod sse;
pub mod trace;

use std::sync::Arc;
use std::time::Duration;

use std::convert::Infallible;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::Route;
use axum::routing::{get, patch, post, put};
use orch_app::App;
use orch_ports::Ports;
use tower::{Layer, Service};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::limiter::PublicLimiter;

pub use artifacts::{
    CACHE_CONTROL as ARTIFACT_CACHE_CONTROL,
    CONTENT_SECURITY_POLICY as ARTIFACT_CONTENT_SECURITY_POLICY, MAX_SVG_INLINE_BYTES,
};
pub use auth::IDENTITY_HEADER;
pub use export::{FORMAT as EXPORT_FORMAT, VERSION as EXPORT_VERSION};
pub use extract::{ApiJson, ApiQuery};
pub use host::is_host_authority;
pub use limiter::{FAILURE_EXTRA, Limited, LinkKey, PublicAccess, PublicLimits, StreamPermit};
pub use problem::{ApiError, Problem};
pub use routes::parse_thread_id;
pub use shared::too_many_streams;

/// Everything configurable about the router.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Interval of the SSE `: keepalive` comment (15 s per the contract; shorter in tests).
    /// Not used by the resource API itself: surfaces read it and pass it to their streams.
    pub sse_keepalive: Duration,
    /// Request timeout for everything except the SSE stream.
    pub request_timeout: Duration,
    /// The rate limit of the public routes (ADR 0040, section 10): `Some` builds the limiter, and
    /// the default is the ADR's starting numbers. `None` builds none, and the public routes then
    /// answer the uniform 404 to everything: **public sharing fails closed without a limiter**,
    /// and [`ApiConfig::check`] refuses a `sharing.mode: public` that has none, so the order the
    /// owner asked for (the limit before `public`) is enforced by the code.
    pub public_limits: Option<PublicLimits>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            sse_keepalive: Duration::from_secs(15),
            request_timeout: Duration::from_secs(30),
            public_limits: Some(PublicLimits::default()),
        }
    }
}

/// Why an [`ApiConfig`] cannot serve the application it is given.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ApiConfigError {
    /// `sharing.mode: public` and no limiter for the public routes.
    #[error("sharing.mode is public but the public routes have no rate limiter")]
    PublicWithoutLimiter,
}

impl ApiConfig {
    /// Checks that this configuration can serve `app`: a deployment that allows public sharing has
    /// the limiter of the public routes (ADR 0040, section 10). A binary calls it at startup and
    /// refuses to start (exit 78) when it fails.
    ///
    /// # Errors
    /// [`ApiConfigError::PublicWithoutLimiter`].
    pub fn check<P: Ports>(&self, app: &App<P>) -> Result<(), ApiConfigError> {
        if app.sharing().mode() == orch_app::SharingMode::Public && self.public_limits.is_none() {
            return Err(ApiConfigError::PublicWithoutLimiter);
        }
        Ok(())
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
/// `Extension<orch_ports::Principal>` (who is calling and their roles; pass it to the application,
/// which enforces ADR 0033). `machine` routes sit behind neither. `public` routes (ADR 0040) sit
/// outside the identity layer too, but behind the rate limit and the headers of the public routes.
#[derive(Debug, Default)]
pub struct SurfaceRoutes {
    plain: Router,
    streaming: Router,
    machine: Router,
    public: Router,
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

    /// Adds machine routes (ADR 0016, ADR 0019): routes for a caller that is not a person behind
    /// oauth2-proxy, so they sit **outside** the identity layer, and outside the request timeout
    /// (a machine route may be a long call).
    ///
    /// `guard` is the surface's own authentication, a tower layer (an HMAC check, a bearer check)
    /// that wraps `routes` here, so a machine route cannot be added without one. It must fail
    /// closed and never read `X-Auth-Request-Email` (the edge does not set it on these paths, and
    /// a client can send anything): the caller's identity is whatever the surface's own credential
    /// says it is.
    #[must_use]
    pub fn machine<L>(mut self, routes: Router, guard: L) -> Self
    where
        L: Layer<Route> + Clone + Send + Sync + 'static,
        L::Service: Service<Request> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Response: IntoResponse + 'static,
        <L::Service as Service<Request>>::Error: Into<Infallible> + 'static,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        self.machine = self.machine.merge(routes.layer(guard));
        self
    }

    /// Adds public routes (ADR 0040): routes anybody may call, with no identity, for a thread its
    /// owner shared with the world. They sit **outside** the identity layer, like a machine route,
    /// and have no request timeout (they may stream), but every one passes the **rate limit** and
    /// the headers of the public routes (`Cache-Control: no-store`, `X-Robots-Tag: noindex,
    /// nofollow`); the limiter is in the request's extensions as a [`PublicAccess`], from which a
    /// stream takes its [`StreamPermit`]. A handler must **never** read a credential: the routes
    /// are unauthenticated by construction, and an `Authorization` header is ignored.
    #[must_use]
    pub fn public(mut self, routes: Router) -> Self {
        self.public = self.public.merge(routes);
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
        .layer(TraceLayer::new_for_http().make_span_with(trace::RedactedSpan))
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
    let state_for_me = state.clone();
    let state_for_public = state.clone();
    let identity_app = Arc::clone(&state.app);
    let health = health_routes(state.clone());
    let resource = Router::new()
        .route("/api/agents", get(routes::list_agents::<P>))
        .route("/api/registry", get(routes::registry_status::<P>))
        .route("/api/config", get(routes::public_config::<P>))
        .route("/api/tool-servers", get(routes::list_tool_servers::<P>))
        .route("/api/threads", get(routes::list_threads::<P>))
        .route(
            "/api/threads/{thread_id}",
            get(routes::get_thread::<P>)
                .patch(routes::patch_thread::<P>)
                .delete(routes::delete_thread::<P>),
        )
        .route(
            "/api/threads/{thread_id}/rail",
            patch(routes::arrange_thread::<P>),
        )
        .route(
            "/api/threads/{thread_id}/tools",
            put(routes::put_thread_tools::<P>),
        )
        .route(
            "/api/threads/{thread_id}/export",
            get(routes::export_thread::<P>),
        )
        .route(
            "/api/threads/{thread_id}/cancel",
            post(routes::cancel_thread::<P>),
        )
        .route(
            "/api/threads/{thread_id}/fork",
            post(routes::fork_thread::<P>),
        )
        .route(
            "/api/threads/{thread_id}/branches",
            get(routes::list_branches::<P>),
        )
        .route(
            "/api/threads/{thread_id}/artifacts/{sha256}",
            get(artifacts::get_artifact::<P>),
        )
        // Sharing by a revocable link (ADR 0040): the owner's routes, then what a signed-in reader
        // of a shared thread asks.
        .route(
            "/api/threads/{thread_id}/share",
            put(routes::put_share::<P>).delete(routes::delete_share::<P>),
        )
        .route(
            "/api/threads/{thread_id}/share/rotate",
            post(routes::rotate_share::<P>),
        )
        .with_state(state.clone())
        .merge(
            // What a signed-in reader asks of a shared thread: never cached, never indexed, the
            // refusals included.
            Router::new()
                .route("/api/shared/{token}", get(shared::get_shared::<P>))
                .route(
                    "/api/shared/{token}/artifacts/{sha256}",
                    get(shared::get_shared_artifact::<P>),
                )
                .with_state(state.clone())
                .layer(axum::middleware::map_response(shared::harden_response)),
        );
    let mut plain = resource;
    let mut streaming = Router::new();
    let mut machine = Router::new();
    let mut surface_public = Router::new();
    for surface in surfaces {
        plain = plain.merge(surface.plain);
        streaming = streaming.merge(surface.streaming);
        machine = machine.merge(surface.machine);
        surface_public = surface_public.merge(surface.public);
    }
    let timeout = TimeoutLayer::with_status_code(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        cfg.request_timeout,
    );
    let plain = plain.layer(timeout);
    // A person whose roles grant nothing is refused by every route but `/api/me` (ADR 0033), which
    // tells them so. The guard wraps what the resource API and the surfaces route, not `/api/me`.
    let guarded = plain.merge(streaming).layer(from_fn_with_state(
        Arc::clone(&identity_app),
        auth::require_access::<P>,
    ));
    let me = Router::new()
        .route("/api/me", get(routes::me::<P>))
        .with_state(state_for_me)
        .layer(timeout);
    // The public routes of a shared thread (ADR 0040): outside the identity layer, so no identity can
    // ride along, behind the rate limit and the headers. Always mounted, so that a link that does not
    // work (a deployment whose cap is `disabled` or `internal` included) is the one 404 and not a 401.
    let limiter = cfg.public_limits.map(PublicLimiter::new);
    let public = Router::new()
        .route(
            "/api/public/shared/{token}",
            get(shared::get_public_shared::<P>),
        )
        .route(
            "/api/public/shared/{token}/artifacts/{sha256}",
            get(shared::get_public_shared_artifact::<P>),
        )
        .with_state(state_for_public)
        .layer(timeout)
        .merge(surface_public)
        .layer(from_fn_with_state(limiter, shared::guard));
    // The identity layer wraps every non-health path, unknown ones included.
    let api = guarded
        .merge(me)
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .layer(from_fn_with_state(
            identity_app,
            auth::require_identity::<P>,
        ));
    // Machine routes are merged beside the identity-guarded routes, not under them: their surface
    // guards them. A path they do not own falls through to the guarded router's 401/404.
    edge_layers(
        Router::new()
            .merge(health)
            .merge(machine)
            .merge(public)
            .merge(api),
    )
}
