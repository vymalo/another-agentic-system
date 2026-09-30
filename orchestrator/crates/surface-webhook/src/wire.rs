//! Reading a request as a machine route does: header text, and the whole body up to a limit.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, StatusCode, header};
use futures::StreamExt;
use orch_api::Problem;

/// The value of header `name` as text, or `None` when it is absent or not visible ASCII.
pub(crate) fn text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

/// What a route's guard hands the handler after it has verified a delivery: the exact bytes that
/// were signed, and what the route needs from the headers. A handler that finds none was reached
/// without its guard and refuses, so a route cannot be mounted unguarded by accident.
#[derive(Debug, Clone)]
pub(crate) struct Verified {
    /// The raw body, as received.
    pub(crate) body: Bytes,
    /// The delivery id header, as received, for the log only: it is not signed, so nothing
    /// decides on it. Empty when absent or unusable.
    pub(crate) delivery: String,
    /// `X-GitHub-Event`, for the GitHub route; empty for the generic one. Not signed either: it
    /// only selects the parser, and the body is read as strictly as that event demands.
    pub(crate) event: String,
    /// The idempotency key the guard derived from what was signed (the generic route: a digest
    /// of the signed string); empty where the handler derives it from the body.
    pub(crate) key: String,
}

/// The delivery id header's text if it is usable in a log line: visible ASCII, at most
/// [`MAX_DELIVERY_ID_BYTES`] bytes. Anything else is dropped, never rejected.
pub(crate) fn delivery_id(headers: &HeaderMap, name: &str) -> String {
    text(headers, name)
        .map(str::trim)
        .filter(|s| s.len() <= MAX_DELIVERY_ID_BYTES && s.bytes().all(|b| b.is_ascii_graphic()))
        .unwrap_or_default()
        .to_owned()
}

/// The longest delivery id kept for the log.
const MAX_DELIVERY_ID_BYTES: usize = 64;

/// How long a delivery may take to arrive and be answered, by default (`read_timeout` of a
/// route's config). A body that trickles in is cut off with 408 instead of holding a connection
/// and a buffer.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The 413 of a body over `limit`.
pub(crate) fn too_large(limit: usize) -> Problem {
    Problem::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("the body is over the {limit}-byte limit of this route"),
    )
}

/// Reads the whole `body`, refusing (413) a declared `Content-Length` or an actual length over
/// `limit`. The declared length is checked first, so an oversized body is refused without reading
/// it; the read is bounded either way, because a length can be absent or a lie.
///
/// The buffer is **not** sized from the declared length: that number is the sender's, and a
/// thousand connections that each declare 5 MiB and send nothing would otherwise hold 5 GiB. The
/// buffer grows with what actually arrives, up to `limit`.
pub(crate) async fn read_limited(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<Bytes, Problem> {
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit as u64) {
        return Err(too_large(limit));
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Problem::bad_request("the body could not be read"))?;
        if buf.len() + chunk.len() > limit {
            return Err(too_large(limit));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buf))
}

/// Refusals, logged without letting them flood the log: a scanner or an attacker can send
/// thousands a second, every one of them a 401. The first refusal in each window is a `warn`
/// that says how many more there were; the rest are `debug`.
#[derive(Debug)]
pub(crate) struct RefusalLog {
    route: &'static str,
    window: Duration,
    state: Mutex<Window>,
}

#[derive(Debug, Default)]
struct Window {
    opened: Option<Instant>,
    suppressed: u64,
}

impl RefusalLog {
    /// A log for `route` that warns at most once per 30 seconds.
    pub(crate) fn new(route: &'static str) -> Self {
        RefusalLog {
            route,
            window: Duration::from_secs(30),
            state: Mutex::new(Window::default()),
        }
    }

    /// Whether a refusal at `now` is the one to warn about, and how many were held back since
    /// the last warning.
    fn admit(&self, now: Instant) -> Option<u64> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.opened {
            Some(opened) if now.saturating_duration_since(opened) < self.window => {
                state.suppressed += 1;
                None
            }
            _ => {
                state.opened = Some(now);
                Some(std::mem::take(&mut state.suppressed))
            }
        }
    }

    /// Records a refusal and returns the answer for it.
    pub(crate) fn refuse(&self, status: StatusCode, why: &'static str) -> Problem {
        match self.admit(Instant::now()) {
            Some(held_back) => tracing::warn!(
                route = self.route,
                %status,
                why,
                held_back,
                "webhook delivery refused (others in the last 30 s are logged at debug)"
            ),
            None => tracing::debug!(route = self.route, %status, why, "webhook delivery refused"),
        }
        Problem::new(status, why)
    }

    /// Records a refusal that already is a [`Problem`] (a body over the limit).
    pub(crate) fn refused(&self, problem: Problem) -> Problem {
        match StatusCode::from_u16(problem.status) {
            Ok(status) => {
                match self.admit(Instant::now()) {
                    Some(held_back) => tracing::warn!(
                        route = self.route,
                        %status,
                        held_back,
                        "webhook delivery refused (others in the last 30 s are logged at debug)"
                    ),
                    None => {
                        tracing::debug!(route = self.route, %status, "webhook delivery refused")
                    }
                }
                problem
            }
            Err(_) => problem,
        }
    }
}

/// `url` if it is an `http` or `https` URL, else `None`: a card links to a run, and no other
/// scheme (`javascript:`, `file:`) is ever kept. The URL kept is the parsed one (normalised), not
/// the text that was sent.
pub(crate) fn http_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.as_str().to_owned())
}

/// `text` cut to at most `max` bytes, at a character boundary.
pub(crate) fn truncate(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn refusals_warn_once_per_window_and_count_the_rest() {
        let log = RefusalLog::new("/webhooks/x");
        let t0 = Instant::now();
        assert_eq!(log.admit(t0), Some(0), "the first is a warning");
        for i in 1..=5 {
            assert_eq!(log.admit(t0 + Duration::from_secs(i)), None);
        }
        assert_eq!(
            log.admit(t0 + Duration::from_secs(31)),
            Some(5),
            "the next window's warning says how many were held back"
        );
        assert_eq!(log.admit(t0 + Duration::from_secs(32)), None);
    }

    #[test]
    fn a_url_is_kept_as_parsed_and_only_when_http() {
        assert_eq!(
            http_url(" HTTPS://Example.com/a b ").as_deref(),
            Some("https://example.com/a%20b")
        );
        assert_eq!(http_url("javascript:alert(1)"), None);
        assert_eq!(http_url("file:///etc/passwd"), None);
        assert_eq!(http_url("not a url"), None);
    }

    #[test]
    fn the_delivery_id_is_kept_for_the_log_only_when_it_is_plain() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-d",
            "72d3162e-cc78-11e3-81ab-4c9367dc0958".parse().unwrap(),
        );
        assert_eq!(
            delivery_id(&headers, "x-d"),
            "72d3162e-cc78-11e3-81ab-4c9367dc0958"
        );
        headers.insert("x-d", "a b".parse().unwrap());
        assert_eq!(delivery_id(&headers, "x-d"), "");
        headers.insert("x-d", "x".repeat(65).parse().unwrap());
        assert_eq!(delivery_id(&headers, "x-d"), "");
        assert_eq!(delivery_id(&headers, "x-missing"), "");
    }
}
