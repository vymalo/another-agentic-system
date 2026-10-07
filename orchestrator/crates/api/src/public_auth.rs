//! `GET /api/public/auth`: where a browser signs in (ADR 0054, decision 7).
//!
//! The web is a public OAuth client of the token issuer and signs in itself. What it needs to
//! start, the issuer, its client id and the scope, is the orchestrator's to say: a static export of
//! the web has no server to read an environment variable from. The route is outside the identity
//! layer (the person is not signed in yet), behind the rate limit and the headers of the public
//! routes, and answers a plain 404 when `auth.browser` is not configured: the web then keeps the
//! edge's cookie, as before. It holds nothing secret: a public client has no secret.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::problem::Problem;

/// The path of the route.
pub(crate) const PATH: &str = "/api/public/auth";

/// What the browser signs in with (`auth.browser`, and `auth.jwt.issuer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserAuth {
    /// The token issuer, as its tokens say it in `iss`.
    pub issuer: String,
    /// The web's OAuth client id, a public client.
    pub client_id: String,
    /// The scope the web asks for (`openid email profile offline_access`).
    pub scope: String,
}

/// `GET /api/public/auth`: [`BrowserAuth`], or 404 when this deployment has none.
pub(crate) async fn get(State(browser): State<Option<Arc<BrowserAuth>>>) -> Response {
    let Some(browser) = browser else {
        // The one 404 of every route that does not exist.
        return Problem::not_found("no such route").into_response();
    };
    let mut response = Json(&*browser).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
