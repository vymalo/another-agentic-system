//! Calls from a page of another origin (CORS, ADR 0047): the desktop and mobile apps, whose pages
//! are `tauri://localhost` or `http://tauri.localhost`, call this API at its own origin.
//!
//! An allow-list, compared exactly with the request's `Origin`: never `*`, never credentials (a
//! cross-origin caller sends a DPoP-bound token in `Authorization`, never a cookie). The allowed
//! request headers are the ones the web sends; the exposed response headers are the ones it reads
//! (`WWW-Authenticate` for a DPoP refusal, `Date` for its clock, `Content-Disposition` for a file's
//! name). A preflight is answered before identity, which it never carries. The configuration checks
//! the origins (`server.cors`); one that is not a valid header value is skipped here.

use std::time::Duration;

use axum::http::header::{
    ACCEPT, AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_TYPE, DATE, WWW_AUTHENTICATE,
};
use axum::http::{HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// How long a browser may keep a preflight's answer (10 minutes).
pub const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(600);

/// The layer for `origins`, or `None` when there is none (no CORS header at all).
pub fn layer(origins: &[String]) -> Option<CorsLayer> {
    let origins: Vec<HeaderValue> = origins
        .iter()
        .filter(|origin| origin.as_str() != "*" && origin.as_str() != "null")
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    if origins.is_empty() {
        return None;
    }
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([
                Method::GET,
                Method::HEAD,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([
                AUTHORIZATION,
                CONTENT_TYPE,
                ACCEPT,
                HeaderName::from_static("dpop"),
                HeaderName::from_static("last-event-id"),
                HeaderName::from_static("x-web-revision"),
            ])
            .expose_headers([WWW_AUTHENTICATE, DATE, CONTENT_DISPOSITION])
            .max_age(PREFLIGHT_MAX_AGE),
    )
}
