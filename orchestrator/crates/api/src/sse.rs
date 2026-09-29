use std::convert::Infallible;
use std::time::Duration;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, header};
use axum::response::IntoResponse;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::StreamExt;
use orch_core::{Event, UserId};
use orch_ports::Ports;

use crate::ApiState;
use crate::problem::ApiError;
use crate::routes::parse_thread_id;

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
    State(state): State<ApiState<P>>,
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
    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::max(state.keepalive, Duration::from_millis(10)))
            .text("keepalive"),
    );
    Ok((
        [
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-transform"),
            ),
            (
                HeaderName::from_static("x-accel-buffering"),
                HeaderValue::from_static("no"),
            ),
        ],
        sse,
    ))
}
