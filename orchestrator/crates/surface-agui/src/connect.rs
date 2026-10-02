//! `GET /agui/threads/{threadId}/connect`: the thread's AG-UI events for a viewer, replayed from
//! the start or from a cursor, then followed live across runs.
//!
//! The stream is [`orch_agui_projection::Connect`] driven by the log: the handler reads the
//! thread's events from the first through [`App::event_stream`] (a log read that then follows
//! new events by wakeup, or by polling where there is none), and [`Connect`] folds them, drops
//! what the client holds already, and says when a `?mode=run` stream is over. Nothing about the
//! stream lives in the process: a client whose replica dies reconnects to any other with
//! `Last-Event-ID` and gets the rest of what the log says. Closing the connection never cancels a
//! run.
//!
//! Every refusal is an RFC 9457 problem and comes before the first stream byte: 404 for a thread
//! that does not exist for the caller (missing, malformed id, or one they may not read, all the same
//! answer; an administrator may read everyone's, ADR 0033), 403 for roles that hold no `thread.read`, 400 for a cursor or `mode` that is not understood, 406 for an `Accept` that excludes
//! `text/event-stream`.

use std::collections::VecDeque;
use std::convert::Infallible;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use orch_agui_projection::{Connect, Follow, Frame, LiveOverlay};
use orch_api::sse::{bounded, keep_alive, stream_budget, stream_headers};
use orch_api::{ApiError, ApiQuery, Problem, parse_thread_id};
use orch_app::FeedItem;
use orch_ports::{Clock, Ports, Principal};
use serde::Deserialize;

use crate::State as SurfaceState;
use crate::refuse::check_accept;
use crate::run::meta_of;
use crate::stream::sse;

/// The query of the connect route.
#[derive(Debug, Deserialize)]
pub(crate) struct ConnectQuery {
    mode: Option<Mode>,
}

/// `?mode=`: the only value besides the default (stay open) is `run`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Run,
}

/// `Last-Event-ID` as the cursor: a non-negative integer (the `seq` of a resume point), absent or
/// empty for "from the start".
fn cursor_of(headers: &HeaderMap) -> Result<i64, Problem> {
    let Some(raw) = headers.get("last-event-id") else {
        return Ok(0);
    };
    let text = raw
        .to_str()
        .map_err(|_| Problem::bad_request("Last-Event-ID is not text"))?
        .trim();
    if text.is_empty() {
        return Ok(0);
    }
    text.parse::<i64>().ok().filter(|n| *n >= 0).ok_or_else(|| {
        Problem::bad_request("Last-Event-ID must be a non-negative integer (the id: of a frame)")
    })
}

pub(crate) async fn connect<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<ConnectQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_accept(&headers)?;
    let cursor = cursor_of(&headers)?;
    let follow = match query.mode {
        Some(Mode::Run) => Follow::ThroughRun,
        None => Follow::Forever,
    };
    // 404 is decided here, before any stream byte is sent.
    let thread = parse_thread_id(&id)?;
    let record = state.app.get_thread(&principal, thread).await?;
    let connect = Connect::new(meta_of(&record), cursor, record.last_seq, follow);
    // From the first event, always: the fold up to the cursor is silent, and is what makes the
    // preamble the same on every replica. `head` is `record.last_seq`; what arrives after it is
    // live.
    // The log, and the live text of the thread's replies mixed in (ADR 0027).
    let live = state.app.thread_feed(&principal, thread, 0).await?;
    tracing::debug!(
        %thread,
        cursor = connect.cursor(),
        head = record.last_seq,
        ?follow,
        "a viewer connected"
    );
    // The stream lasts as long as the token it was opened with (ADR 0033): the client reconnects
    // with `Last-Event-ID` and a fresh one.
    let budget = stream_budget(&principal, state.app.ports().clock().now());
    let sse =
        Sse::new(bounded(frames(connect, live), budget)).keep_alive(keep_alive(state.keepalive));
    Ok((stream_headers(), sse).into_response())
}

/// The SSE messages of a connect stream. It ends when [`Connect::finished`] says so, or when
/// `live` does (the process is shutting down): the client then holds a truncated stream and
/// reconnects with the last `id:` it saw.
///
/// The words of a reply that is still being written (ADR 0027) are frames of the connection's own
/// [`LiveOverlay`], beside the fold of the log: never a resume point, merged by message id with the
/// log's final message. A new connection has an empty overlay, which is why it is told the text so
/// far by the sender's refresh.
fn frames(
    connect: Connect,
    live: BoxStream<'static, FeedItem>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> + Send {
    struct St {
        connect: Connect,
        overlay: LiveOverlay,
        live: BoxStream<'static, FeedItem>,
        pending: VecDeque<Frame>,
    }
    let st = St {
        connect,
        overlay: LiveOverlay::new(),
        live,
        pending: VecDeque::new(),
    };
    futures::stream::unfold(st, |mut st| async move {
        loop {
            if let Some(frame) = st.pending.pop_front() {
                return Some((Ok(sse(&frame)), st));
            }
            if st.connect.finished() {
                tracing::debug!("a ?mode=run connect stream is over");
                return None;
            }
            let Some(item) = st.live.next().await else {
                tracing::debug!(
                    "the event stream ended (shutdown): the viewer reconnects with its cursor"
                );
                return None;
            };
            match item {
                FeedItem::Event(event) => {
                    let said = st.connect.feed(&event);
                    let said = st.overlay.logged(st.connect.projector(), said);
                    st.pending.extend(said);
                }
                // Before the replay is over the pieces would be attributed to an old invocation.
                FeedItem::Live(piece) if st.connect.caught_up() => {
                    let said = st.overlay.live(st.connect.projector(), &piece);
                    st.pending.extend(said);
                }
                FeedItem::Live(_) => {}
            }
        }
    })
}
