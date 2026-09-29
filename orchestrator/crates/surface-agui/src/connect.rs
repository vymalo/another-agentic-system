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
//! that does not exist for the caller (missing, malformed id, or someone else's, all the same
//! answer), 400 for a cursor or `mode` that is not understood, 406 for an `Accept` that excludes
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
use orch_agui_projection::{Connect, Follow, Frame};
use orch_api::sse::{keep_alive, stream_headers};
use orch_api::{ApiError, ApiQuery, Problem, parse_thread_id};
use orch_core::{Event, UserId};
use orch_ports::Ports;
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
    Extension(user): Extension<UserId>,
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
    let record = state.app.get_thread(&user, thread).await?;
    let connect = Connect::new(meta_of(&record), cursor, record.last_seq, follow);
    // From the first event, always: the fold up to the cursor is silent, and is what makes the
    // preamble the same on every replica. `head` is `record.last_seq`; what arrives after it is
    // live.
    let live = state.app.event_stream(&user, thread, 0).await?;
    tracing::debug!(
        %thread,
        cursor = connect.cursor(),
        head = record.last_seq,
        ?follow,
        "a viewer connected"
    );
    let sse = Sse::new(frames(connect, live)).keep_alive(keep_alive(state.keepalive));
    Ok((stream_headers(), sse).into_response())
}

/// The SSE messages of a connect stream. It ends when [`Connect::finished`] says so, or when
/// `live` does (the process is shutting down): the client then holds a truncated stream and
/// reconnects with the last `id:` it saw.
fn frames(
    connect: Connect,
    live: BoxStream<'static, Event>,
) -> impl Stream<Item = Result<SseEvent, Infallible>> + Send {
    struct St {
        connect: Connect,
        live: BoxStream<'static, Event>,
        pending: VecDeque<Frame>,
    }
    let st = St {
        connect,
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
            let Some(event) = st.live.next().await else {
                tracing::debug!(
                    "the event stream ended (shutdown): the viewer reconnects with its cursor"
                );
                return None;
            };
            st.pending.extend(st.connect.feed(&event));
        }
    })
}
