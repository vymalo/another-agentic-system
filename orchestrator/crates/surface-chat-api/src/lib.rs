//! The legacy interaction surface of the chat API: `createThread`, `postMessage`,
//! `listEvents` and `streamEvents` of `docs/api/chat-api.yaml`, over [`orch_app::App`].
//!
//! It is one of the surfaces the binary mounts with `ORCH_SURFACES` (name `chat-api`), and
//! deprecated in favour of AG-UI (ADR 0012). It depends only on `App` and on the shared HTTP
//! pieces of `orch-api` (problems, extractors, identity), so it can be switched off, or
//! replaced, without touching the resource API. Mount it with
//! [`orch_api::router_with_surfaces`]; the identity layer then wraps every route below.
//!
//! # Deprecation
//!
//! Every response one of the four operations produces (an error included) carries
//! `Deprecation: @1790640000`, the header of [RFC 9745](https://www.rfc-editor.org/rfc/rfc9745):
//! an sf-date, the unix time in seconds of the day the operation was deprecated
//! ([`DEPRECATED_AT`], 2026-09-29T00:00:00Z). The header is added by a layer on this surface's own
//! routes, so the resource API, the health routes, the AG-UI surface and a 401 from the shared
//! identity layer never carry it. There is no `Sunset` and no `Link`: the RFC makes both optional,
//! the removal is tied to the web's migration and not to a date, and a successor is a URI
//! template (`/agui/agents/{agentId}`), which `Link` cannot carry. The replacements are named in
//! the contract (`docs/api/chat-api.yaml`, `deprecated: true`).

mod routes;
mod stream;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::HeaderValue;
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::{get, post};
use orch_api::SurfaceRoutes;
use orch_app::App;
use orch_ports::Ports;

/// Unix time, in seconds, of the day the four operations were deprecated: 2026-09-29T00:00:00Z.
pub const DEPRECATED_AT: i64 = 1_790_640_000;

/// The value of the `Deprecation` header: `@` and [`DEPRECATED_AT`], an RFC 9651 sf-date.
pub const DEPRECATION: &str = "@1790640000";

/// Adds `Deprecation` (RFC 9745) to a response.
async fn deprecation(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("deprecation"),
        HeaderValue::from_static(DEPRECATION),
    );
    response
}

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

/// The routes of the surface, each answering with `Deprecation`: `POST /api/threads`, `POST /api/threads/{id}/messages`,
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
        .with_state(state.clone())
        .layer(map_response(deprecation));
    let streaming = Router::new()
        .route(
            "/api/threads/{thread_id}/stream",
            get(stream::stream_events::<P>),
        )
        .with_state(state)
        .layer(map_response(deprecation));
    SurfaceRoutes::new().plain(plain).streaming(streaming)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_value_is_the_constant() {
        assert_eq!(format!("@{DEPRECATED_AT}"), DEPRECATION);
    }
}
