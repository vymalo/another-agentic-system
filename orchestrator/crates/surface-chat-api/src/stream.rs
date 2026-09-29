use std::convert::Infallible;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, Sse};
use futures::StreamExt;
use orch_api::sse::{keep_alive, stream_headers};
use orch_api::{ApiError, parse_thread_id};
use orch_core::{Event, UserId};
use orch_ports::Ports;

use crate::State as SurfaceState;

/// `Last-Event-ID` as a sequence number; missing, invalid or negative means 0 (replay everything).
fn last_event_id(headers: &HeaderMap) -> i64 {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map_or(0, |n| n.max(0))
}

fn frame(event: &Event) -> Option<SseEvent> {
    SseEvent::default()
        .id(event.seq.to_string())
        .event(event.kind().as_str())
        .json_data(event)
        .ok()
}

pub(crate) async fn stream_events<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    let id = parse_thread_id(&id)?;
    // 404 is decided here, before any stream byte is sent.
    let events = state
        .app
        .event_stream(&user, id, last_event_id(&headers))
        .await?;
    let stream = events.filter_map(|e| async move { frame(&e).map(Ok::<_, Infallible>) });
    let sse = Sse::new(stream).keep_alive(keep_alive(state.keepalive));
    Ok((stream_headers(), sse))
}
