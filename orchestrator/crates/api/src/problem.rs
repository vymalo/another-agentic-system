use std::time::Duration;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_app::AppError;
use orch_core::{Classify, ErrorClass, ForkError, report};
use serde::Serialize;

/// RFC 9457 problem details (`application/problem+json`), the contract's `Problem` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// Always `about:blank` (the status says it all).
    #[serde(rename = "type")]
    pub r#type: String,
    /// Short summary.
    pub title: String,
    /// HTTP status.
    pub status: u16,
    /// Human-readable explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A stable, machine-readable name for the problem, where a client acts on one (`turn_open`:
    /// try again when the turn has ended). RFC 9457 allows extension members.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl Problem {
    /// A problem with the canonical title of `status`.
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Problem {
            r#type: "about:blank".to_owned(),
            title: status.canonical_reason().unwrap_or("Error").to_owned(),
            status: status.as_u16(),
            detail: Some(detail.into()),
            code: None,
        }
    }

    /// The same problem with a machine-readable `code`.
    #[must_use]
    pub fn with_code(mut self, code: &str) -> Self {
        self.code = Some(code.to_owned());
        self
    }

    /// 400.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// 401.
    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, detail)
    }

    /// 404.
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    /// 403.
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, detail)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&self).unwrap_or_default();
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

/// Retry-After (seconds) sent with a 503 for a lost optimistic race: the next attempt re-reads.
const RETRY_AFTER_CONTENDED: u64 = 1;
/// Retry-After (seconds) sent with a 503 when nothing better is known.
const RETRY_AFTER_UNAVAILABLE: u64 = 5;

/// What a handler can `?`: an application failure, or a problem already shaped by the API
/// layer (a malformed query, an unknown route) that must keep its own status.
pub enum ApiError {
    /// Mapped by [`Classify::class`] (RFC 9457, see [`problem_for`]).
    App(AppError),
    /// Already a problem.
    Problem(Problem),
}

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError::App(e)
    }
}

impl From<Problem> for ApiError {
    fn from(p: Problem) -> Self {
        ApiError::Problem(p)
    }
}

/// Whole seconds for a `Retry-After` header, rounded up, at least 1.
fn retry_after_secs(wait: Option<Duration>, default: u64) -> u64 {
    wait.map_or(default, |d| {
        d.as_secs().saturating_add(u64::from(d.subsec_nanos() > 0))
    })
    .max(1)
}

/// The problem an application failure becomes, and the `Retry-After` it carries.
///
/// The status follows the error's class (the design's table):
///
/// | class | status |
/// |---|---|
/// | `NotFound` | 404 |
/// | `Forbidden` | 403, the domain message, with `code: forbidden` (`read_only` for a thread the person may read and not change) |
/// | `Invalid` | 400, the domain message |
/// | `Rejected` | 409 |
/// | a cut the thread does not allow (`AppError::Fork`) | 422 for a point that is not in the log or not a person's message, 409 with `code: turn_open` for a turn that is still going on |
/// | `Conflict` | 503 + `Retry-After: 1` |
/// | `Transient` (the store) | 503 + `Retry-After: 5` |
/// | the agent registry cannot say whether an agent exists | 503 "the agent registry is unreachable" + `Retry-After: 5` |
/// | an agent that rate limits | 503 + its `Retry-After` |
/// | any other agent failure | 502 |
/// | `Corrupt`, `Internal`, anything else | 500 |
///
/// The detail of a 5xx is fixed text; the cause is logged, never sent.
pub(crate) fn problem_for(err: &AppError) -> (Problem, Option<u64>) {
    let class = err.class();
    if let AppError::Upstream { agent, source } = err {
        tracing::warn!(%agent, error = %report(err), class = ?class, "an agent failed a request");
        return if class == ErrorClass::RateLimited {
            let secs = retry_after_secs(source.retry_after(), RETRY_AFTER_UNAVAILABLE);
            (
                Problem::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
                Some(secs),
            )
        } else {
            (Problem::new(StatusCode::BAD_GATEWAY, err.to_string()), None)
        };
    }
    if matches!(err, AppError::RegistryUnavailable { .. }) {
        // The caller did nothing wrong and the cause is not storage: say what is down. The
        // detail is fixed text; the source and the reason are logged.
        tracing::warn!(error = %report(err), "the agent registry could not be read");
        return (
            Problem::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
            Some(retry_after_secs(err.retry_after(), RETRY_AFTER_UNAVAILABLE)),
        );
    }
    if let AppError::Fork(fork) = err {
        // A cut the thread does not allow: the request is well formed, what it names is not there
        // (422), or not yet (409, `turn_open`).
        return match fork {
            ForkError::OutOfRange | ForkError::NotAMessage => (
                Problem::new(StatusCode::UNPROCESSABLE_ENTITY, err.to_string()),
                None,
            ),
            ForkError::TurnOpen => (
                Problem::new(StatusCode::CONFLICT, err.to_string()).with_code("turn_open"),
                None,
            ),
        };
    }
    if let AppError::Forbidden {
        detail, read_only, ..
    } = err
    {
        let code = if *read_only { "read_only" } else { "forbidden" };
        return (Problem::forbidden(detail.clone()).with_code(code), None);
    }
    match class {
        ErrorClass::NotFound => (Problem::not_found("no such thread"), None),
        ErrorClass::Invalid => (Problem::bad_request(err.to_string()), None),
        ErrorClass::Rejected => {
            let detail = match err {
                AppError::Finished => {
                    "This card belongs to a finished request; write a message to start the next one"
                        .to_owned()
                }
                other => other.to_string(),
            };
            (Problem::new(StatusCode::CONFLICT, detail), None)
        }
        ErrorClass::Conflict => (
            Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "the thread is busy; try again",
            ),
            Some(retry_after_secs(err.retry_after(), RETRY_AFTER_CONTENDED)),
        ),
        ErrorClass::Transient | ErrorClass::RateLimited => {
            tracing::error!(error = %report(err), class = ?class, "storage is unavailable");
            (
                Problem::new(StatusCode::SERVICE_UNAVAILABLE, "storage is unavailable"),
                Some(retry_after_secs(err.retry_after(), RETRY_AFTER_UNAVAILABLE)),
            )
        }
        // `Corrupt`, `Internal`, and any class added later: a bug or a broken deployment.
        _ => {
            tracing::error!(error = %report(err), class = ?class, "request failed");
            (
                Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
                None,
            )
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Problem(p) => p.into_response(),
            ApiError::App(e) => {
                let (problem, retry_after) = problem_for(&e);
                let mut response = problem.into_response();
                if let Some(secs) = retry_after {
                    response
                        .headers_mut()
                        .insert(header::RETRY_AFTER, HeaderValue::from(secs));
                }
                response
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::{AgentId, ThreadState, TransitionError};
    use orch_ports::{AgentError, RegistryError, StoreError};

    use super::*;

    fn io(msg: &str) -> std::io::Error {
        std::io::Error::other(msg.to_owned())
    }

    fn respond(e: impl Into<ApiError>) -> Response {
        e.into().into_response()
    }

    fn retry_after(r: &Response) -> Option<&str> {
        r.headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap())
    }

    #[test]
    fn status_mapping_table() {
        let coder = AgentId::new("coder");
        let table: Vec<(AppError, u16, Option<&str>)> = vec![
            (AppError::NotFound, 404, None),
            (AppError::Store(StoreError::NotFound), 404, None),
            (AppError::Invalid("bad".into()), 400, None),
            (AppError::Finished, 409, None),
            (
                AppError::Transition(TransitionError::InvalidInState {
                    state: ThreadState::Queued,
                    input: "cancel",
                }),
                409,
                None,
            ),
            (AppError::Contended, 503, Some("1")),
            (AppError::Store(StoreError::VersionConflict), 503, Some("1")),
            (
                AppError::Store(StoreError::unavailable(io("pool timed out"))),
                503,
                Some("5"),
            ),
            (AppError::Store(StoreError::corrupt("bad row")), 500, None),
            (
                AppError::Store(StoreError::internal(io("syntax error"))),
                500,
                None,
            ),
            (AppError::internal("broken"), 500, None),
            (
                AppError::upstream(&coder, AgentError::unreachable("card")),
                502,
                None,
            ),
            (
                AppError::upstream(&coder, AgentError::unauthenticated("401")),
                502,
                None,
            ),
            (
                AppError::upstream(&coder, AgentError::Rejected("nope".into())),
                502,
                None,
            ),
            (
                AppError::upstream(
                    &coder,
                    AgentError::RateLimited {
                        retry_after: Some(Duration::from_millis(7_200)),
                    },
                ),
                503,
                Some("8"),
            ),
            (
                AppError::upstream(&coder, AgentError::RateLimited { retry_after: None }),
                503,
                Some("5"),
            ),
            (
                AppError::RegistryUnavailable {
                    source: RegistryError::unavailable("platform", "could not be reached"),
                },
                503,
                Some("5"),
            ),
        ];
        for (err, status, retry) in table {
            let label = format!("{err:?}");
            let r = respond(err);
            assert_eq!(r.status().as_u16(), status, "{label}");
            assert_eq!(retry_after(&r), retry, "{label}");
            assert_eq!(
                r.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/problem+json",
                "{label}"
            );
        }
    }

    #[test]
    fn a_problem_passed_through_keeps_its_status() {
        assert_eq!(respond(Problem::not_found("no such route")).status(), 404);
        assert_eq!(respond(Problem::unauthorized("who?")).status(), 401);
        assert_eq!(
            respond(Problem::new(StatusCode::METHOD_NOT_ALLOWED, "no")).status(),
            405
        );
        assert_eq!(respond(Problem::bad_request("x")).status(), 400);
    }

    #[test]
    fn the_detail_of_a_5xx_is_fixed_text_never_the_cause() {
        let secret = "postgres://user:hunter2@db.internal/orch";
        for err in [
            AppError::Store(StoreError::unavailable(io(secret))),
            AppError::Store(StoreError::internal(io(secret))),
            AppError::Store(StoreError::corrupt_with("row", io(secret))),
            AppError::upstream(
                &AgentId::new("coder"),
                AgentError::unreachable("card").with_source(io(secret)),
            ),
            AppError::RegistryUnavailable {
                source: RegistryError::unavailable("platform", "the registry could not be reached")
                    .with_source(io(secret)),
            },
        ] {
            let (problem, _) = problem_for(&err);
            let text = serde_json::to_string(&problem).unwrap();
            assert!(!text.contains("hunter2"), "{text}");
            assert!(!text.contains("db.internal"), "{text}");
        }
        let (problem, _) = problem_for(&AppError::upstream(
            &AgentId::new("coder"),
            AgentError::unreachable("card"),
        ));
        assert_eq!(
            problem.detail.as_deref(),
            Some("agent coder is unavailable")
        );
        let (problem, _) = problem_for(&AppError::RegistryUnavailable {
            source: RegistryError::unavailable("platform", "the registry could not be reached"),
        });
        assert_eq!(
            problem.detail.as_deref(),
            Some("the agent registry is unreachable")
        );
    }
}
