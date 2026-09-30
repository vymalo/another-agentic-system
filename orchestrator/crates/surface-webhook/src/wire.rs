//! Reading a request as a machine route does: header text, and the whole body up to a limit.

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
    /// The header text the route asked for (its delivery id), as received; empty when absent
    /// (only the GitHub route tolerates that: for an event it ignores).
    pub(crate) delivery: String,
    /// `X-GitHub-Event`, for the GitHub route; empty for the generic one.
    pub(crate) event: String,
}

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
    let mut buf: Vec<u8> = Vec::with_capacity(declared.map_or(0, |n| n as usize).min(limit));
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

/// `url` if it is an `http` or `https` URL, else `None`: a card links to a run, and no other
/// scheme (`javascript:`, `file:`) is ever kept.
pub(crate) fn http_url(url: &str) -> Option<String> {
    let url = url.trim();
    let parsed = url::Url::parse(url).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| url.to_owned())
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
