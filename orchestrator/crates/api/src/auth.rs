//! The identity layer: reads the credentials of a request, asks the [`Authenticator`] of the
//! application's ports who is calling (ADR 0033), and either lets the request through with the
//! [`Principal`] in its extensions or answers it. The only place that reads `Authorization`, `DPoP`
//! (RFC 9449, ADR 0054) and `X-Auth-Request-Email`.

use std::sync::Arc;

use axum::extract::{OriginalUri, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_app::App;
use orch_ports::{
    AuthError, Authenticator, CredentialKind, Credentials, DpopCredentials, Ports, Principal,
};

use crate::problem::Problem;

/// Header set by oauth2-proxy after login.
pub const IDENTITY_HEADER: &str = "x-auth-request-email";

/// The `realm` of a `WWW-Authenticate: Bearer` challenge.
const REALM: &str = "orchestrator";

/// The `algs` of a `WWW-Authenticate: DPoP` challenge (RFC 9449 §7.1): what a proof may be signed
/// with.
const DPOP_ALGS: &str = "ES256 EdDSA";

/// The header that carries a DPoP proof.
const DPOP_HEADER: &str = "dpop";

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
    scheme_token(first, "bearer")
}

/// The token of the one `Authorization: DPoP <token>` header, `Some("")` when it cannot be a token.
/// `None` when the scheme is not `DPoP`, and for a request with several `Authorization` headers,
/// which [`bearer`] has already made a refusal.
fn dpop_token(headers: &HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let first = all.next()?;
    if all.next().is_some() {
        return None;
    }
    scheme_token(first, "dpop")
}

/// The part of an `Authorization` value after `scheme` (compared without case), `Some("")` when it
/// is not text.
fn scheme_token<'a>(value: &'a HeaderValue, scheme: &str) -> Option<&'a str> {
    let Ok(text) = value.to_str() else {
        // Not text: a bearer of nothing, which is refused and never taken for an absent one.
        return (scheme == "bearer").then_some("");
    };
    let (given, rest) = text.split_once(' ').unwrap_or((text, ""));
    given
        .eq_ignore_ascii_case(scheme)
        .then(|| rest.trim_matches(' '))
}

/// Every `DPoP` header of the request, in order; one that is not text is `""`.
fn dpop_proofs(headers: &HeaderMap) -> Vec<&str> {
    headers
        .get_all(DPOP_HEADER)
        .iter()
        .map(|v| v.to_str().ok().unwrap_or(""))
        .collect()
}

/// The identity header, with one that is not text read as empty (so it is refused).
fn identity_header(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(IDENTITY_HEADER)
        .map(|v| v.to_str().ok().unwrap_or(""))
}

/// The `WWW-Authenticate` challenges of a 401, one header each (RFC 9110 §11.6.1).
fn challenges(error: &AuthError, accepts_bearer: bool, accepts_dpop: bool) -> Vec<String> {
    let dpop = |error: &str| {
        let error = if error.is_empty() {
            String::new()
        } else {
            format!("error=\"{error}\", ")
        };
        format!("DPoP {error}algs=\"{DPOP_ALGS}\"")
    };
    match error {
        // A bad proof, or a token that is bad as a DPoP one: RFC 9449 §7.1, and nothing but DPoP.
        AuthError::Invalid {
            credential: CredentialKind::DpopProof,
            ..
        } => vec![dpop("invalid_dpop_proof")],
        AuthError::Invalid {
            credential: CredentialKind::DpopToken,
            ..
        } => vec![dpop("invalid_token")],
        _ => {
            let mut all = Vec::new();
            if accepts_bearer {
                all.push(match error {
                    AuthError::Invalid {
                        credential: CredentialKind::Bearer,
                        ..
                    } => format!("Bearer realm=\"{REALM}\", error=\"invalid_token\""),
                    _ => format!("Bearer realm=\"{REALM}\""),
                });
            }
            if accepts_dpop {
                all.push(dpop(""));
            }
            all
        }
    }
}

/// The response to a request that was not authenticated.
fn refusal(error: &AuthError, accepts_bearer: bool, accepts_dpop: bool) -> Response {
    tracing::debug!(%error, "request not authenticated");
    match error {
        AuthError::Missing | AuthError::Invalid { .. } => {
            let detail = match error {
                AuthError::Invalid { detail, .. } => detail.clone(),
                _ if accepts_bearer && accepts_dpop => "authentication required: send \
                     Authorization: Bearer <token>, or Authorization: DPoP <token> with a DPoP proof"
                    .to_owned(),
                _ if accepts_bearer => {
                    "authentication required: send Authorization: Bearer <token>".to_owned()
                }
                // Only the proxy's header is accepted: name it, as before ADR 0033.
                _ => "missing X-Auth-Request-Email".to_owned(),
            };
            let mut response = Problem::unauthorized(detail).into_response();
            for challenge in challenges(error, accepts_bearer, accepts_dpop) {
                if let Ok(value) = HeaderValue::from_str(&challenge) {
                    response
                        .headers_mut()
                        .append(header::WWW_AUTHENTICATE, value);
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
/// and stores the [`Principal`] in the request extensions (its `user` is what a person owns). A
/// credential that is present and bad is refused even when another would have served.
pub(crate) async fn require_identity<P: Ports>(
    State(app): State<Arc<App<P>>>,
    mut req: Request,
    next: Next,
) -> Response {
    let auth = app.ports().auth();
    let principal = {
        let headers = req.headers();
        let proofs = dpop_proofs(headers);
        // The path as the orchestrator received it (a nested router would have stripped its prefix
        // from `uri()`): the authenticator joins it to the public origins it is configured with.
        let path = req
            .extensions()
            .get::<OriginalUri>()
            .map_or_else(|| req.uri().path(), |original| original.0.path());
        let credentials = Credentials {
            bearer: bearer(headers),
            identity_header: identity_header(headers),
            dpop: dpop_token(headers).map(|token| DpopCredentials {
                token,
                proofs: &proofs,
                method: req.method().as_str(),
                path,
            }),
        };
        match auth.authenticate(&credentials).await {
            Ok(principal) => principal,
            Err(error) => return refusal(&error, auth.accepts_bearer(), auth.accepts_dpop()),
        }
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Refuses a person whose roles grant nothing at all (403): a valid token with no role the
/// configuration knows, and no `auth.defaultRole` to fall back on (ADR 0033). It wraps every route
/// but `GET /api/me`, which answers for such a person so that a client can say why. A person whose
/// roles grant something is let through to the route, whose own permission checks decide.
pub(crate) async fn require_access<P: Ports>(
    State(app): State<Arc<App<P>>>,
    req: Request,
    next: Next,
) -> Response {
    let granted = req
        .extensions()
        .get::<Principal>()
        .is_some_and(|principal| !app.access(principal).is_empty());
    if !granted {
        // Without a principal nothing was authenticated: the identity layer is missing, and this
        // must not be the one that lets the request through.
        return Problem::forbidden("your roles do not grant access to this API")
            .with_code("no_access")
            .into_response();
    }
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

    #[test]
    fn a_dpop_token_is_the_token_after_the_dpop_scheme_and_not_a_bearer() {
        let h = headers(&[("authorization", "DPoP abc.def.ghi")]);
        assert_eq!(dpop_token(&h), Some("abc.def.ghi"));
        assert_eq!(bearer(&h), None, "a DPoP request has no bearer");
        assert_eq!(
            dpop_token(&headers(&[("authorization", "dpop  abc")])),
            Some("abc")
        );
        assert_eq!(dpop_token(&headers(&[("authorization", "DPoP")])), Some(""));
        assert_eq!(
            dpop_token(&headers(&[("authorization", "DPoP  ")])),
            Some("")
        );
        assert_eq!(
            dpop_token(&headers(&[("authorization", "Bearer abc")])),
            None
        );
        assert_eq!(
            dpop_token(&headers(&[("authorization", "DPoPx abc")])),
            None
        );
        assert_eq!(dpop_token(&headers(&[])), None);
    }

    #[test]
    fn two_authorization_headers_are_a_refused_bearer_and_never_a_dpop_request() {
        let two = headers(&[("authorization", "DPoP a"), ("authorization", "DPoP b")]);
        assert_eq!(bearer(&two), Some(""));
        assert_eq!(dpop_token(&two), None);
        let mut raw = HeaderMap::new();
        raw.insert(
            header::AUTHORIZATION,
            HeaderValue::from_bytes(b"DPoP \xff").unwrap(),
        );
        assert_eq!(bearer(&raw), Some(""));
        assert_eq!(dpop_token(&raw), None);
    }

    #[test]
    fn every_dpop_header_is_a_proof_and_one_that_is_not_text_is_empty() {
        assert!(dpop_proofs(&headers(&[])).is_empty());
        assert_eq!(
            dpop_proofs(&headers(&[("dpop", "a.b.c"), ("DPoP", "d.e.f")])),
            ["a.b.c", "d.e.f"]
        );
        let mut raw = HeaderMap::new();
        raw.insert("dpop", HeaderValue::from_bytes(b"\xff").unwrap());
        assert_eq!(dpop_proofs(&raw), [""]);
    }

    #[test]
    fn the_challenges_say_what_is_read_and_what_went_wrong() {
        let challenge = |error: &AuthError, bearer, dpop| challenges(error, bearer, dpop);
        let algs = "algs=\"ES256 EdDSA\"";
        assert_eq!(
            challenge(&AuthError::invalid_dpop_proof("x"), true, true),
            [format!("DPoP error=\"invalid_dpop_proof\", {algs}")]
        );
        assert_eq!(
            challenge(&AuthError::invalid_dpop_token("x"), true, true),
            [format!("DPoP error=\"invalid_token\", {algs}")]
        );
        // Generic: both, when both are read; Bearer alone as before when DPoP is not.
        assert_eq!(
            challenge(&AuthError::Missing, true, true),
            [
                "Bearer realm=\"orchestrator\"".to_owned(),
                format!("DPoP {algs}")
            ]
        );
        assert_eq!(
            challenge(&AuthError::Missing, true, false),
            ["Bearer realm=\"orchestrator\""]
        );
        assert_eq!(
            challenge(&AuthError::invalid_bearer("x"), true, true),
            [
                "Bearer realm=\"orchestrator\", error=\"invalid_token\"".to_owned(),
                format!("DPoP {algs}")
            ]
        );
        assert!(challenge(&AuthError::Missing, false, false).is_empty());
    }
}
