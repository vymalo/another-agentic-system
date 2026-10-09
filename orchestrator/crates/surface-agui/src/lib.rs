//! The AG-UI interaction surface of `docs/api/agui.md`, over [`orch_app::App`]: the run route
//! `POST /agui/agents/{agentId}`, the connect stream `GET /agui/threads/{threadId}/connect` and
//! the capabilities document `GET /agui/agents/{agentId}/capabilities`.
//!
//! # The run route
//!
//! A consumer sends an AG-UI `RunAgentInput` and gets an SSE stream of AG-UI events back: the
//! [`Requester`](orch_agui_projection::Audience::Requester) projection of the thread's event
//! log, from the first event its input caused to the terminal event of that run, then EOF, as
//! the [HTTP + SSE binding](https://docs.ag-ui.com/spec/1.0/basic/transports/http-sse.md)
//! says. Everything the stream says is a function of the log: this crate feeds
//! [`orch_agui_projection`] and writes its frames, and decides nothing about them.
//!
//! One request does this, in order (each refusal is an RFC 9457 problem, before any stream byte):
//!
//! 1. the headers and the body: `Accept`, `Content-Type`, size, JSON shape (400, 406, 413, 415);
//! 2. the thread: the consumer mints its id (a UUID); someone else's id is 404, like an id that
//!    does not exist for the caller; the URL's agent must be the thread's (404, 409);
//! 3. [`orch_agui_projection::translate`]: reconcile the transcript by id, turn `resume` and the
//!    new message into one core input, or attach to a run the log already holds (400, 409, 422);
//! 4. apply it (create the thread, or [`App::submit`](orch_app::App::submit)) under the
//!    idempotency key `agui:<threadId>:msg:<messageId>` (`…:run:<runId>` for an answer), so a
//!    retried POST attaches instead of duplicating;
//! 5. stream.
//!
//! A run is not tied to its connection: dropping the response never cancels anything.
//!
//! # The connect stream
//!
//! A viewer's stream of a thread, our extension of the transport: the
//! [`Viewer`](orch_agui_projection::Audience::Viewer) projection of the log, replayed from the
//! start or from a `Last-Event-ID` cursor (with a preamble that re-opens the run that is open
//! there) and then followed across runs, with keepalive comments. `?mode=run` closes after the
//! active run. The stream is [`orch_agui_projection::Connect`] driven by
//! [`App::event_stream`](orch_app::App::event_stream), so it lives in the log and not in the
//! process: any replica serves any viewer, and a client whose replica died reconnects to another
//! with its last `id:`. A thread that is missing, malformed or someone else's is one 404, before
//! any stream byte.
//!
//! # The history read
//!
//! `GET /agui/threads/{threadId}/history` is a finite page of the connect stream's frames: a whole
//! number of settled chains of runs, from the newest back (`before`, `limit`, `since`) or after a
//! point (`after`), with the `end` a connect resumes from ([`history`](crate::history), ADR 0059,
//! `docs/api/history.md`). It folds the log with [`orch_agui_projection::History`] and is
//! mounted with the connect stream, with the same two variants for shared threads.
//!
//! # The capabilities document
//!
//! [`orch_agui_projection::agent_capabilities`] over the agent's card, read live for each request
//! ([`App::describe_agent`](orch_app::App::describe_agent)) and never cached.
//!
//! The surface is one of those the binary mounts with `ORCH_SURFACES` (name `agui`, Cargo
//! feature `surface-agui`). It depends only on `App`, `orch-core`, the pure AG-UI crates and the
//! shared HTTP pieces of `orch-api`, so it can be switched off or replaced without touching the
//! resource API. Mount it with [`orch_api::router_with_surfaces`]; the identity layer then wraps
//! every route below.

mod capabilities;
mod connect;
mod history;
mod refuse;
mod run;
mod stream;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use orch_api::SurfaceRoutes;
use orch_app::App;
use orch_ports::Ports;

/// The version of the projection this build writes (`ui.history.projection` of `GET /api/config`,
/// `projection` of a history page): the frames of an event already in a log do not change while it
/// stays the same.
pub use orch_agui_projection::PROJECTION_VERSION;

/// The largest request body: a client that re-sends a long transcript on every run must fit
/// (open question 19), and nothing more is read.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

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

/// The routes of the surface. `POST /agui/agents/{agentId}` (a run) and
/// `GET /agui/threads/{threadId}/connect` (attach, replay, follow) are streaming routes (no
/// request timeout) whose `: keepalive` comment is sent every `sse_keepalive`;
/// `GET /agui/agents/{agentId}/capabilities` and `GET /agui/threads/{threadId}/history` (and its two
/// shared variants) are ordinary requests.
pub fn routes<P: Ports>(app: Arc<App<P>>, sse_keepalive: Duration) -> SurfaceRoutes {
    let state = State {
        app,
        keepalive: sse_keepalive,
    };
    let streaming = Router::new()
        .route("/agui/agents/{agent_id}", post(run::run::<P>))
        .route(
            "/agui/threads/{thread_id}/connect",
            get(connect::connect::<P>),
        )
        // A thread somebody shared with signed-in people (ADR 0040): the same stream over the
        // reader projection, read-only.
        .route(
            "/agui/shared/{token}/connect",
            get(connect::connect_shared::<P>),
        )
        .with_state(state.clone());
    // And for anybody, outside the identity layer, behind the public routes' rate limit.
    let public = Router::new()
        .route(
            "/agui/public/shared/{token}/connect",
            get(connect::connect_public::<P>),
        )
        .route(
            "/agui/public/shared/{token}/history",
            get(history::history_public::<P>),
        )
        .with_state(state.clone());
    // The history of a thread (ADR 0059): one finite JSON answer, so it has the request timeout like
    // the capabilities document, and the owner's and the signed-in reader's sit behind the identity layer.
    let plain = Router::new()
        .route(
            "/agui/agents/{agent_id}/capabilities",
            get(capabilities::capabilities::<P>),
        )
        .route(
            "/agui/threads/{thread_id}/history",
            get(history::history::<P>),
        )
        .route(
            "/agui/shared/{token}/history",
            get(history::history_shared::<P>),
        )
        .with_state(state);
    SurfaceRoutes::new()
        .plain(plain)
        .streaming(streaming)
        .public(public)
}
