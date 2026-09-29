//! What every server-sent-events response of the orchestrator shares: the keepalive comment
//! and the headers that keep a proxy from buffering or transforming the stream.

use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, header};
use axum::response::sse::KeepAlive;

/// The `: keepalive` comment at `interval`. The interval is clamped to at least 10 ms so a
/// misconfiguration cannot turn the keepalive into a busy loop.
pub fn keep_alive(interval: Duration) -> KeepAlive {
    KeepAlive::new()
        .interval(Duration::max(interval, Duration::from_millis(10)))
        .text("keepalive")
}

/// `Cache-Control: no-cache, no-transform` and `X-Accel-Buffering: no`, for the response head
/// of a stream.
pub fn stream_headers() -> [(HeaderName, HeaderValue); 2] {
    [
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache, no-transform"),
        ),
        (
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        ),
    ]
}
