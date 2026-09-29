//! The legacy interaction surface of the chat API: `createThread`, `postMessage`,
//! `listEvents` and `streamEvents` of `docs/api/chat-api.yaml`, over [`orch_app::App`].
//!
//! It is one of the surfaces the binary mounts with `ORCH_SURFACES` (name `chat-api`), and
//! deprecated in favour of AG-UI (ADR 0012). It depends only on `App` and on the shared HTTP
//! pieces of `orch-api` (problems, extractors, identity), so it can be switched off, or
//! replaced, without touching the resource API. Mount it with
//! [`orch_api::router_with_surfaces`]; the identity layer then wraps every route below.

mod routes;
mod stream;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use orch_api::SurfaceRoutes;
use orch_app::App;
use orch_ports::Ports;

pub(crate) struct State<P: Ports> {
    pub(crate) app: Arc<App<P>>,
    pub(crate) keepalive: Duration,
}

impl<P: Ports> Clone for State<P> {
    fn clone(&self) -> Self {
        State {
            app: Arc::clone(&self.app),
            keepalive: self.keepalive,
        }
    }
}

/// The routes of the surface: `POST /api/threads`, `POST /api/threads/{id}/messages`,
/// `GET /api/threads/{id}/events` and the SSE `GET /api/threads/{id}/stream`, whose
/// `: keepalive` comment is sent every `sse_keepalive`.
pub fn routes<P: Ports>(app: Arc<App<P>>, sse_keepalive: Duration) -> SurfaceRoutes {
    let state = State {
        app,
        keepalive: sse_keepalive,
    };
    let plain = Router::new()
        .route("/api/threads", post(routes::create_thread::<P>))
        .route(
            "/api/threads/{thread_id}/events",
            get(routes::list_events::<P>),
        )
        .route(
            "/api/threads/{thread_id}/messages",
            post(routes::post_message::<P>),
        )
        .with_state(state.clone());
    let streaming = Router::new()
        .route(
            "/api/threads/{thread_id}/stream",
            get(stream::stream_events::<P>),
        )
        .with_state(state);
    SurfaceRoutes::new().plain(plain).streaming(streaming)
}
