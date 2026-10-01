//! The guard of the route: the token, then the thread. Fails closed.
//!
//! The checks of `docs/api/thread-tools-v1.md`, in order. Every failure of the token or of what
//! it names is the same answer, `401` with `WWW-Authenticate: Bearer error="invalid_token"` and no
//! detail, and nothing has been written or called; a request without a bearer token at all gets
//! `401` and `WWW-Authenticate: Bearer` with no error code (RFC 6750 section 3.1). A store that
//! fails is not a failed token: it is `503`, so that an agent does not take a transient fault for
//! a dead token.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_api::Problem;
use orch_app::App;
use orch_core::{Caller, ThreadId, UserId, report};
use orch_ports::{Clock, Ports};
use orch_thread_token::{Claims, ThreadToolsKeys, TokenError, verify};

/// The path of the route, with the thread in the middle.
pub const ROUTE: &str = "/thread-tools/{threadId}/mcp";

/// What the guard gives the tools: the verified claims, and whose thread it is.
#[derive(Debug, Clone)]
pub(crate) struct Verified {
    pub(crate) claims: Claims,
    pub(crate) owner: UserId,
}

/// The state of the guard.
pub(crate) struct GuardState<P: Ports> {
    pub(crate) app: Arc<App<P>>,
    pub(crate) keys: ThreadToolsKeys,
}

impl<P: Ports> Clone for GuardState<P> {
    fn clone(&self) -> Self {
        GuardState {
            app: Arc::clone(&self.app),
            keys: self.keys.clone(),
        }
    }
}

/// The token of an `Authorization: Bearer <token>` header. The scheme is case-insensitive
/// (RFC 9110); anything else, a missing header, a header that is not text and **more than one
/// `Authorization` header** are `None`: with two, a proxy and this server could read different
/// ones, so none is trusted.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut all = headers.get_all(header::AUTHORIZATION).iter();
    let value = all.next()?;
    if all.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

/// The thread of `/thread-tools/<thread>/mcp`.
fn thread_of(path: &str) -> Option<ThreadId> {
    path.strip_prefix("/thread-tools/")?
        .strip_suffix("/mcp")?
        .parse()
        .ok()
}

fn challenge(value: &'static str) -> Response {
    let mut response = Problem::unauthorized("a valid bearer token is required").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(value));
    response
}

/// No bearer token: RFC 6750 asks for no error code, since the request carried no credentials.
fn no_credentials() -> Response {
    challenge("Bearer")
}

/// A token that failed a check, or names a thread that is not its own: one answer for all.
fn invalid_token() -> Response {
    challenge(r#"Bearer error="invalid_token""#)
}

fn unavailable() -> Response {
    let mut response =
        Problem::new(StatusCode::SERVICE_UNAVAILABLE, "temporarily unavailable").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("2"));
    response
}

/// Why a request that carried a token is refused, for the operator's log (never the token).
#[derive(Debug, thiserror::Error)]
enum Refusal {
    #[error("{0}")]
    Token(TokenError),
    #[error("the path does not name a thread")]
    PathIsNotAThread,
    #[error("the token is for another thread")]
    OtherThread,
    #[error("the thread does not exist")]
    NoSuchThread,
    #[error("the token is for another agent than the thread's")]
    OtherAgent,
    #[error("the ask the token names is not on the job's ledger")]
    AskNotOnTheLedger,
}

/// The guard: checks the token and the thread, and carries [`Verified`] to the tools.
pub(crate) async fn require_grant<P: Ports>(
    State(state): State<GuardState<P>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer(request.headers()) else {
        tracing::debug!("a thread-tools request without a bearer token");
        return no_credentials();
    };
    let now = state.app.ports().clock().now();
    let claims = match verify(&state.keys, token, now) {
        Ok(claims) => claims,
        Err(e) => return refuse(Refusal::Token(e)),
    };
    let Some(path_thread) = thread_of(request.uri().path()) else {
        return refuse(Refusal::PathIsNotAThread);
    };
    if claims.thread != path_thread {
        return refuse(Refusal::OtherThread);
    }
    let thread = match state.app.thread_for_tools(claims.thread).await {
        Ok(Some(thread)) => thread,
        Ok(None) => return refuse(Refusal::NoSuchThread),
        Err(e) => {
            tracing::error!(thread = %claims.thread, error = %report(&e), "the thread of a thread-tools token could not be read");
            return unavailable();
        }
    };
    match claims.caller {
        // The thread's addressed agent: the token must have been minted for it.
        Caller::Main if thread.target.agent_id == claims.agent => {}
        Caller::Main => return refuse(Refusal::OtherAgent),
        // An asked agent (ask_agent, ADR 0026) is on a ledger of the job that nothing writes yet:
        // until a slice builds it, no `ask:<n>` token names an ask that exists.
        Caller::Ask(_) => return refuse(Refusal::AskNotOnTheLedger),
    }
    request.extensions_mut().insert(Verified {
        claims,
        owner: thread.owner,
    });
    next.run(request).await
}

fn refuse(why: Refusal) -> Response {
    tracing::warn!(reason = %why, "a thread-tools request was refused");
    invalid_token()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_bearer_scheme_is_case_insensitive_and_nothing_else_counts() {
        let with = |value: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
            h
        };
        assert_eq!(bearer(&with("Bearer abc")), Some("abc"));
        assert_eq!(bearer(&with("bearer abc")), Some("abc"));
        assert_eq!(bearer(&with("BEARER  abc ")), Some("abc"));
        assert_eq!(bearer(&with("Basic abc")), None);
        assert_eq!(bearer(&with("Bearer")), None);
        assert_eq!(bearer(&with("Bearer ")), None);
        assert_eq!(bearer(&with("abc")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
        // Two headers: none is trusted, even when both say the same.
        let mut two = with("Bearer abc");
        two.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(bearer(&two), None);
    }

    #[test]
    fn the_thread_is_the_middle_of_the_path() {
        let id = "01927a4e-3b00-7000-8000-000000000001";
        assert_eq!(
            thread_of(&format!("/thread-tools/{id}/mcp")),
            Some(id.parse().unwrap())
        );
        for bad in [
            "/thread-tools/not-a-uuid/mcp",
            "/thread-tools//mcp",
            "/thread-tools/01927a4e-3b00-7000-8000-000000000001",
            "/thread-tools/01927a4e-3b00-7000-8000-000000000001/mcp/",
            "/mcp",
            "",
        ] {
            assert_eq!(thread_of(bad), None, "{bad}");
        }
    }
}
