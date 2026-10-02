//! The identity layer: reads the credentials of a request, asks the [`Authenticator`] of the
//! application's ports who is calling (ADR 0033), and either lets the request through with the
//! [`Principal`] in its extensions or answers it. The only place that reads `Authorization` and
//! `X-Auth-Request-Email`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_app::App;
use orch_ports::{AuthError, Authenticator, CredentialKind, Credentials, Ports};

use crate::problem::Problem;

/// Header set by oauth2-proxy after login.
pub const IDENTITY_HEADER: &str = "x-auth-request-email";

/// The `realm` of a `WWW-Authenticate: Bearer` challenge.
const REALM: &str = "orchestrator";

/// Seconds to wait before asking again when authentication is unavailable.
const RETRY_AFTER_SECS: &str = "5";

/// The token of an `Authorization: Bearer <token>` header. `None` when there is no such header
/// or its scheme is not `Bearer` (`Basic` is somebody else's); `Some("")` when it is a bearer
/// header that cannot be a token (empty, not text, or sent twice), so that it is refused and
/// never mistaken for an absent one.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let first = all.next()?;
    if all.next().is_some() {
        return Some("");
    }
    let Ok(text) = first.to_str() else {
        return Some("");
    };
    let (scheme, rest) = text.split_once(' ').unwrap_or((text, ""));
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim_matches(' '))
}

/// The identity header, with one that is not text read as empty (so it is refused).
fn identity_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(IDENTITY_HEADER)
        .map(|v| v.to_str().ok().unwrap_or(""))
}

/// The response to a request that was not authenticated.
fn refusal(error: &AuthError, accepts_bearer: bool) -> Response {
    tracing::debug!(%error, "request not authenticated");
    match error {
        AuthError::Missing | AuthError::Invalid { .. } => {
            let detail = match error {
                AuthError::Invalid { detail, .. } => detail.clone(),
                _ if accepts_bearer => {
                    "authentication required: send Authorization: Bearer <token>".to_owned()
                }
                _ => "authentication required".to_owned(),
            };
            let mut response = Problem::unauthorized(detail).into_response();
            if accepts_bearer {
                let challenge = match error {
                    AuthError::Invalid {
                        credential: CredentialKind::Bearer,
                        ..
                    } => format!("Bearer realm=\"{REALM}\", error=\"invalid_token\""),
                    _ => format!("Bearer realm=\"{REALM}\""),
                };
                if let Ok(value) = HeaderValue::from_str(&challenge) {
                    response
                        .headers_mut()
                        .insert(header::WWW_AUTHENTICATE, value);
                }
            }
            response
        }
        // `Unavailable`, `NotConfigured`: nobody is let in, and it is not a refusal of anybody.
        _ => {
            let detail = if matches!(error, AuthError::NotConfigured) {
                "this process does not authenticate requests"
            } else {
                "authentication is temporarily unavailable"
            };
            let mut response =
                Problem::new(StatusCode::SERVICE_UNAVAILABLE, detail).into_response();
            response.headers_mut().insert(
                header::RETRY_AFTER,
                HeaderValue::from_static(RETRY_AFTER_SECS),
            );
            response
        }
    }
}

/// Refuses requests that are not authenticated (401, or 503 when authentication is unavailable)
/// and stores the [`Principal`] and its [`UserId`](orch_core::UserId) in the request extensions.
/// A credential that is present and bad is refused even when another would have served.
pub(crate) async fn require_identity<P: Ports>(
    State(app): State<Arc<App<P>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth = app.ports().auth();
    let credentials = Credentials {
        bearer: bearer(req.headers()),
        identity_header: identity_header(req.headers()),
    };
    let principal = match auth.authenticate(&credentials).await {
        Ok(principal) => principal,
        Err(error) => return refusal(&error, auth.accepts_bearer()),
    };
    req.extensions_mut().insert(principal.user.clone());
    req.extensions_mut().insert(principal);
    next.run(req).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn a_bearer_is_the_token_after_the_scheme() {
        assert_eq!(bearer(&headers(&[])), None);
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer abc.def.ghi")])),
            Some("abc.def.ghi")
        );
        assert_eq!(
            bearer(&headers(&[("authorization", "bearer  abc")])),
            Some("abc")
        );
        assert_eq!(
            bearer(&headers(&[("authorization", "BEARER abc")])),
            Some("abc")
        );
    }

    #[test]
    fn another_scheme_is_not_a_bearer() {
        assert_eq!(
            bearer(&headers(&[("authorization", "Basic dXNlcjpwYXNz")])),
            None
        );
        assert_eq!(bearer(&headers(&[("authorization", "Bearerx abc")])), None);
    }

    #[test]
    fn a_bearer_that_cannot_be_a_token_is_present_and_empty() {
        assert_eq!(bearer(&headers(&[("authorization", "Bearer")])), Some(""));
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer   ")])),
            Some("")
        );
        assert_eq!(
            bearer(&headers(&[
                ("authorization", "Bearer a"),
                ("authorization", "Bearer b")
            ])),
            Some("")
        );
        let mut raw = HeaderMap::new();
        raw.insert(
            header::AUTHORIZATION,
            HeaderValue::from_bytes(b"Bearer \xff").unwrap(),
        );
        assert_eq!(bearer(&raw), Some(""));
    }

    #[test]
    fn an_identity_header_that_is_not_text_is_present_and_empty() {
        assert_eq!(identity_header(&headers(&[])), None);
        assert_eq!(
            identity_header(&headers(&[(IDENTITY_HEADER, "a@b")])),
            Some("a@b")
        );
        let mut raw = HeaderMap::new();
        raw.insert(
            IDENTITY_HEADER,
            HeaderValue::from_bytes(b"\xffa@b").unwrap(),
        );
        assert_eq!(identity_header(&raw), Some(""));
    }
}
