//! The chat HTTP API, implementing `docs/api/chat-api.yaml` over [`orch_app::App`].
//!
//! Identity comes from `X-Auth-Request-Email` (set by oauth2-proxy). Requests without it are
//! refused with 401 everywhere except `/healthz` and `/readyz` (fail closed); the optional
//! `AUTH_DEV_USER` identity applies only when configured. **The identity header is only
//! trustworthy behind a proxy that strips client-supplied copies.**

mod auth;
mod extract;
mod problem;
mod routes;
mod sse;

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

pub use auth::AuthConfig;
pub use problem::Problem;

/// Everything configurable about the router.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Identity handling.
    pub auth: AuthConfig,
    /// Interval of the SSE `: keepalive` comment (15 s per the contract; shorter in tests).
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
    pub(crate) keepalive: Duration,
}

impl<P: Ports> Clone for ApiState<P> {
    fn clone(&self) -> Self {
        ApiState {
            app: Arc::clone(&self.app),
            keepalive: self.keepalive,
        }
    }
}

/// Builds the router for every operation of the contract.
pub fn router<P: Ports>(app: Arc<App<P>>, cfg: ApiConfig) -> Router {
    let state = ApiState {
        app,
        keepalive: cfg.sse_keepalive,
    };
    let health = Router::new()
        .route("/healthz", get(routes::healthz::<P>))
        .route("/readyz", get(routes::readyz::<P>));
    let plain = Router::new()
        .route("/api/agents", get(routes::list_agents::<P>))
        .route(
            "/api/threads",
            get(routes::list_threads::<P>).post(routes::create_thread::<P>),
        )
        .route("/api/threads/{thread_id}", get(routes::get_thread::<P>))
        .route(
            "/api/threads/{thread_id}/events",
            get(routes::list_events::<P>),
        )
        .route(
            "/api/threads/{thread_id}/messages",
            post(routes::post_message::<P>),
        )
        .route(
            "/api/threads/{thread_id}/cancel",
            post(routes::cancel_thread::<P>),
        )
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            cfg.request_timeout,
        ));
    let streaming = Router::new().route(
        "/api/threads/{thread_id}/stream",
        get(sse::stream_events::<P>),
    );
    // The identity layer wraps every non-health path, unknown ones included.
    let api = plain
        .merge(streaming)
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .layer(from_fn_with_state(
            Arc::new(cfg.auth),
            auth::require_identity,
        ));
    Router::new()
        .merge(health)
        .merge(api)
        .with_state(state)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}
