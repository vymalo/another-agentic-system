//! The request span of the API, with the token of a share link taken out of the path (ADR 0040,
//! section 11).
//!
//! `TraceLayer::new_for_http()` records the request's URI, and a share link carries a capability
//! in its path: a log of the path would be a list of working links. [`RedactedSpan`] records the
//! same fields as the default span (`method`, `uri`, `version`) and the request id, with the token
//! segment of `/s/`, `/api/shared/`, `/api/public/shared/`, `/agui/shared/` and
//! `/agui/public/shared/` replaced by `…`. The query is kept: no token travels in it.

use std::borrow::Cow;

use axum::http::{Request, Uri};
use tower_http::trace::MakeSpan;
use tracing::Span;

/// What a token segment is written as in a log.
pub const REDACTED: &str = "…";

/// The prefixes after which the next path segment is a share token.
const TOKEN_PREFIXES: [&str; 5] = [
    "/s/",
    "/api/shared/",
    "/api/public/shared/",
    "/agui/shared/",
    "/agui/public/shared/",
];

/// `path` with the share token it carries, if any, replaced by `…`.
pub fn redact_path(path: &str) -> Cow<'_, str> {
    for prefix in TOKEN_PREFIXES {
        if let Some(rest) = path.strip_prefix(prefix) {
            let (token, after) = match rest.find('/') {
                Some(end) => rest.split_at(end),
                None => (rest, ""),
            };
            if token.is_empty() {
                return Cow::Borrowed(path);
            }
            return Cow::Owned(format!("{prefix}{REDACTED}{after}"));
        }
    }
    Cow::Borrowed(path)
}

/// The URI as a log may show it: the path redacted, the query as it is.
pub fn redact_uri(uri: &Uri) -> String {
    let path = redact_path(uri.path());
    match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.into_owned(),
    }
}

/// The span of one request, at the level of the default one, over a redacted URI.
#[derive(Debug, Clone, Copy, Default)]
pub struct RedactedSpan;

impl<B> MakeSpan<B> for RedactedSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let request_id = request
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        tracing::debug_span!(
            "request",
            method = %request.method(),
            uri = %redact_uri(request.uri()),
            version = ?request.version(),
            request_id = %request_id,
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_token_segment_of_every_link_path_is_taken_out() {
        for (path, want) in [
            ("/s/TOKEN", "/s/…"),
            ("/s/TOKEN/", "/s/…/"),
            ("/api/shared/TOKEN", "/api/shared/…"),
            (
                "/api/shared/TOKEN/artifacts/ab12",
                "/api/shared/…/artifacts/ab12",
            ),
            ("/api/public/shared/TOKEN", "/api/public/shared/…"),
            (
                "/api/public/shared/TOKEN/artifacts/ab12",
                "/api/public/shared/…/artifacts/ab12",
            ),
            ("/agui/shared/TOKEN/connect", "/agui/shared/…/connect"),
            (
                "/agui/public/shared/TOKEN/connect",
                "/agui/public/shared/…/connect",
            ),
        ] {
            assert_eq!(redact_path(path), want, "{path}");
        }
    }

    #[test]
    fn nothing_else_is_touched() {
        for path in [
            "/",
            "/s",
            "/s/",
            "/api/threads",
            "/api/threads/0190aaaa-0000-7000-8000-000000000123",
            "/api/threads/0190aaaa-0000-7000-8000-000000000123/share",
            "/api/shared/",
            "/api/shared",
            "/agui/threads/abc/connect",
            "/healthz",
            "/sx/TOKEN",
            "/x/s/TOKEN",
        ] {
            assert_eq!(redact_path(path), path, "{path}");
        }
    }

    #[test]
    fn the_uri_keeps_its_query_and_loses_the_token() {
        let uri: Uri = "/agui/shared/SECRET/connect?mode=run".parse().unwrap();
        assert_eq!(redact_uri(&uri), "/agui/shared/…/connect?mode=run");
        let uri: Uri = "http://host/api/public/shared/SECRET".parse().unwrap();
        assert_eq!(redact_uri(&uri), "/api/public/shared/…");
        assert!(!redact_uri(&uri).contains("SECRET"));
    }
}
