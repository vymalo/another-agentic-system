use std::time::Duration;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_app::{AppError, AttachmentRefusal, PromptRefusal, TeamPresetRefusal, UserToolRefusal};
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
    /// The extension members of the prompt library's problems (ADR 0070), boxed so that every
    /// `Result<_, Problem>` of this crate stays small.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub ext: Option<Box<ProblemExt>>,
}

/// The extension members of a problem of the prompt library (RFC 9457 allows them): where a save is
/// refused, or which answer and which rule. Names positions and ids, never a word of a prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProblemExt {
    /// Where a save is refused (`createPrompt`, `updatePrompt`): `name`, `body`,
    /// `variables.<i>.<member>`, `audience`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The UTF-16 offset in the body of the `{{` concerned (`bad_reference`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// `invalid_answer` (`renderPrompt`): the id of the variable whose answer is refused, or `*`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// `invalid_answer`: the rule that failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// `model_not_offered` and `reasoning_not_offered` (ADR 0086): the aliases or levels the agent's
    /// live card offers, empty when it lists none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offered: Option<Vec<String>>,
    /// `client_update_required`: the lowest application version this deployment serves.
    #[serde(rename = "minClientVersion", skip_serializing_if = "Option::is_none")]
    pub min_client_version: Option<String>,
    /// `over_limit` (ADR 0081): the name of the limit the person reached
    /// (`messages_per_minute`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// `over_limit`: how much of the limit the person used in its window.
    #[serde(rename = "inUse", skip_serializing_if = "Option::is_none")]
    pub in_use: Option<u32>,
    /// `over_limit`: the value of the limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
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
            ext: None,
        }
    }

    /// The same problem with a machine-readable `code`.
    #[must_use]
    pub fn with_code(mut self, code: &str) -> Self {
        self.code = Some(code.to_owned());
        self
    }

    /// The same problem with a stable `type` (RFC 9457), where a client matches on it.
    #[must_use]
    pub fn with_type(mut self, uri: &str) -> Self {
        self.r#type = uri.to_owned();
        self
    }

    /// The same problem saying the lowest application version this deployment serves.
    #[must_use]
    pub fn with_min_client_version(mut self, version: &str) -> Self {
        self.ext = Some(Box::new(ProblemExt {
            min_client_version: Some(version.to_owned()),
            ..ProblemExt::default()
        }));
        self
    }

    /// The same problem saying where a save is refused, and for a reference the UTF-16 offset.
    #[must_use]
    pub fn with_place(mut self, at: &str, offset: Option<u32>) -> Self {
        self.ext = Some(Box::new(ProblemExt {
            at: Some(at.to_owned()),
            offset,
            ..ProblemExt::default()
        }));
        self
    }

    /// The same problem saying which answer is refused and by which rule.
    #[must_use]
    pub fn with_answer(mut self, question: &str, rule: &str) -> Self {
        self.ext = Some(Box::new(ProblemExt {
            question: Some(question.to_owned()),
            rule: Some(rule.to_owned()),
            ..ProblemExt::default()
        }));
        self
    }

    /// The same problem naming the limit the person reached, its value and the current usage
    /// (ADR 0081, decision 4).
    #[must_use]
    pub fn with_limit(mut self, limit: &str, in_use: u32, max: u32) -> Self {
        self.ext = Some(Box::new(ProblemExt {
            limit: Some(limit.to_owned()),
            in_use: Some(in_use),
            max: Some(max),
            ..ProblemExt::default()
        }));
        self
    }

    /// 400.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// The unauthorized problem. Only the identity layer (`auth.rs`) answers it: the web sends a request
    /// again after one, which is safe only while no handler has run when it is answered
    /// (`tests/only_identity_answers_401.rs`).
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
#[derive(Debug)]
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
/// | `Forbidden` | 403, the domain message, with `code: forbidden` |
/// | `Invalid` | 400, the domain message |
/// | `Rejected` | 409 |
/// | something well formed that cannot be done: a tool server that is unknown or not offered for the thread's agent, or too many (`AppError::Unprocessable`) | 422 |
/// | a model or a reasoning effort the agent's live card does not offer (`AppError::PickNotOffered`, ADR 0086) | 422 with `code: model_not_offered` or `reasoning_not_offered` and `offered` |
/// | a change of the person's list that cannot be made (`AppError::Arrangement`), a fork or a share of a child session's thread, or what a person asks of a helper session that its state or the card does not allow (ADR 0072) | 422 with `code: bad_anchor`, `code: nested_row`, `code: child_thread`, `child_mentions`, `session_closes_on_stop`, `not_child_thread`, `no_child_sessions`, `unknown_helper` or `bad_description` |
/// | a helper session that is closed, a start the agent has no context for, or four starts that wait, or a thread that notes 256 sessions (`AppError::Blocked`) | 409 with `code: session_closed`, `no_context`, `too_many_starts` or `too_many_sessions` |
/// | a cut the thread does not allow (`AppError::Fork`) | 422 for a point that is not in the log or not a person's message, 409 with `code: turn_open` for a turn that is still going on |
/// | sharing under a `disabled` cap (`AppError::SharingDisabled`) | 403 with `code: sharing_disabled` |
/// | a person who spent the token allowance of their roles for the month (`AppError::AllowanceSpent`, ADR 0081) | 403 with `code: allowance_spent` |
/// | a share above the cap (`AppError::OverCap`), a new link for a thread that is not shared (`AppError::NotShared`) | 409 with `code: over_cap`, `code: not_shared` |
/// | a delete of a thread that works (`AppError::ThreadActive`) | 409 with `code: thread_active` |
/// | a refusal about people's own MCP servers (`AppError::UserTool`, ADR 0046) | 404 off or no such server, 409 `too_many_servers` or `stale_revision`, 422 `credentials_not_kept`, `oauth_not_configured`, `egress_refused`, `tool_server_needs_auth` or `tool_server_unavailable` (an attach of a server of the person's own that is not ready), 400 `url_refused`, 503 `broker_down` or `authorization_unavailable` + `Retry-After: 5`, 503 + `Retry-After: 1` and no code when other requests kept changing the server (`Busy`) |
/// | a message over the person's role's `messages_per_minute` (`AppError::MessageLimit`, ADR 0081) | 429 with `Retry-After` (the seconds until the oldest message in the window leaves it) and `code: over_limit`, `limit`, `inUse`, `max` |
/// | `Conflict` | 503 + `Retry-After: 1` |
/// | `Transient` (the store) | 503 + `Retry-After: 5` |
/// | the agent registry cannot say whether an agent exists | 503 "the agent registry is unreachable" + `Retry-After: 5` |
/// | the live card of an agent cannot be read, and the request needs to know whether it honours an access tier (`AppError::CardUnavailable`, ADR 0061) | 503 "the card of the agent <id> could not be read" + `Retry-After: 5` |
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
    if matches!(err, AppError::CardUnavailable { .. }) {
        // The caller did nothing wrong and nothing was written: the agent's card is not there, so
        // whether it honours the tier cannot be said. The detail names the agent and no URL.
        tracing::warn!(error = %report(err), "an agent card could not be read to check the access tier");
        return (
            Problem::new(StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
            Some(retry_after_secs(err.retry_after(), RETRY_AFTER_UNAVAILABLE)),
        );
    }
    // A message over the person's role's `messages_per_minute` (ADR 0081, decision 4): 429 with the
    // seconds until the oldest message in the window leaves it, and a body that names the limit,
    // its value and the current usage.
    if let AppError::MessageLimit {
        limit,
        in_use,
        max,
        retry_after,
    } = err
    {
        let problem = Problem::new(StatusCode::TOO_MANY_REQUESTS, err.to_string())
            .with_code("over_limit")
            .with_limit(limit, *in_use, *max);
        return (problem, Some(retry_after_secs(Some(*retry_after), 60)));
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
    // A pick the agent's card does not offer (ADR 0086): the code a client acts on, and what is
    // offered. The detail names the pick the person made and nothing of the card but its list.
    if let AppError::PickNotOffered(refusal) = err {
        let mut problem = Problem::new(StatusCode::UNPROCESSABLE_ENTITY, refusal.to_string())
            .with_code(refusal.code());
        problem.ext = Some(Box::new(ProblemExt {
            offered: Some(refusal.offered().to_vec()),
            ..ProblemExt::default()
        }));
        return (problem, None);
    }
    if let AppError::Unprocessable(detail) = err {
        return (
            Problem::new(StatusCode::UNPROCESSABLE_ENTITY, detail.clone()),
            None,
        );
    }
    // A person's own file (ADR 0067, slice 12): each refusal has the status and the code the
    // contract names, and a deployment with no store is a cause that clears, not a client's fault.
    if let AppError::Attachment(refusal) = err {
        let (status, retry) = match refusal {
            AttachmentRefusal::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, None),
            AttachmentRefusal::Empty
            | AttachmentRefusal::BadMediaType
            | AttachmentRefusal::BadFilename => (StatusCode::BAD_REQUEST, None),
            AttachmentRefusal::NoStore => (
                StatusCode::SERVICE_UNAVAILABLE,
                Some(RETRY_AFTER_UNAVAILABLE),
            ),
        };
        return (
            Problem::new(status, refusal.to_string()).with_code(refusal.code()),
            retry,
        );
    }
    // The person's list (ADR 0042): well formed, what it names is in the way, and the client acts on
    // which.
    if let AppError::Arrangement { code, detail } = err {
        return (
            Problem::new(StatusCode::UNPROCESSABLE_ENTITY, detail.clone()).with_code(code),
            None,
        );
    }
    // A helper session (ADR 0072, S3): what is in the way has a code of its own.
    if let AppError::Blocked { code, detail } = err {
        return (
            Problem::new(StatusCode::CONFLICT, detail.clone()).with_code(code),
            None,
        );
    }
    // People's own MCP servers (ADR 0046): what a client acts on has a code of its own. The detail of
    // a refusal names no URL, no secret and no other person's data.
    if let AppError::UserTool(refusal) = err {
        return user_tool_problem(refusal);
    }
    // A person's prompt library (ADR 0070): every refusal has its own code, and names positions and
    // ids and never a word of a prompt or an answer.
    if let AppError::Prompt(refusal) = err {
        return (prompt_problem(refusal), None);
    }
    // A person's team presets (ADR 0074): a code of their own, positions and never a word of a preset.
    if let AppError::TeamPreset(refusal) = err {
        return (team_preset_problem(refusal), None);
    }
    // Sharing (ADR 0040): what a client acts on has a code of its own.
    match err {
        AppError::SharingDisabled => {
            return (
                Problem::forbidden("sharing is disabled in this deployment")
                    .with_code("sharing_disabled"),
                None,
            );
        }
        // The allowance of the person's roles for the period is spent (ADR 0081): nothing was sent
        // to an agent; the body names the limit and what the person used.
        AppError::AllowanceSpent { .. } => {
            return (
                Problem::forbidden(err.to_string()).with_code("allowance_spent"),
                None,
            );
        }
        AppError::OverCap { cap } => {
            return (
                Problem::new(
                    StatusCode::CONFLICT,
                    format!("this deployment shares at most {cap}"),
                )
                .with_code("over_cap"),
                None,
            );
        }
        AppError::NotShared => {
            return (
                Problem::new(StatusCode::CONFLICT, "the thread is not shared")
                    .with_code("not_shared"),
                None,
            );
        }
        // A thread that works is not deleted (ADR 0043): the client offers to stop it first.
        AppError::ThreadActive(_) => {
            return (
                Problem::new(StatusCode::CONFLICT, err.to_string()).with_code("thread_active"),
                None,
            );
        }
        _ => {}
    }
    if let AppError::Forbidden { detail, .. } = err {
        return (
            Problem::forbidden(detail.clone()).with_code("forbidden"),
            None,
        );
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

/// The problem a refusal of the prompt library becomes.
fn prompt_problem(refusal: &PromptRefusal) -> Problem {
    let status = match refusal {
        PromptRefusal::NotFound => StatusCode::NOT_FOUND,
        PromptRefusal::Invalid(_) => StatusCode::BAD_REQUEST,
        PromptRefusal::NameTaken | PromptRefusal::TooMany { .. } | PromptRefusal::StaleRevision => {
            StatusCode::CONFLICT
        }
        PromptRefusal::InvalidAnswer(_) | PromptRefusal::RenderedTooLong => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        // a refusal added later: a bug until it is mapped, and not a client's mistake
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut problem = Problem::new(status, refusal.to_string());
    if let Some(code) = refusal.code() {
        problem = problem.with_code(code);
    }
    match refusal {
        PromptRefusal::Invalid(e) => problem.with_place(&e.at, e.offset),
        PromptRefusal::InvalidAnswer(a) => problem.with_answer(a.question_text(), a.rule),
        _ => problem,
    }
}

/// The problem a refusal of the team presets becomes.
fn team_preset_problem(refusal: &TeamPresetRefusal) -> Problem {
    let status = match refusal {
        TeamPresetRefusal::NotFound => StatusCode::NOT_FOUND,
        TeamPresetRefusal::Invalid(_) => StatusCode::BAD_REQUEST,
        TeamPresetRefusal::AgentRefused { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        TeamPresetRefusal::NameTaken
        | TeamPresetRefusal::TooMany { .. }
        | TeamPresetRefusal::StaleRevision => StatusCode::CONFLICT,
        // a refusal added later: a bug until it is mapped, and not a client's mistake
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut problem = Problem::new(status, refusal.to_string());
    if let Some(code) = refusal.code() {
        problem = problem.with_code(code);
    }
    match refusal {
        TeamPresetRefusal::Invalid(e) => problem.with_place(&e.at, None),
        TeamPresetRefusal::AgentRefused { at } => problem.with_place(at, None),
        _ => problem,
    }
}

/// The problem a refusal about people's own servers becomes, and its `Retry-After`.
fn user_tool_problem(refusal: &UserToolRefusal) -> (Problem, Option<u64>) {
    let (status, retry) = match refusal {
        UserToolRefusal::Off | UserToolRefusal::NotFound => (StatusCode::NOT_FOUND, None),
        UserToolRefusal::TooMany { .. } | UserToolRefusal::StaleRevision => {
            (StatusCode::CONFLICT, None)
        }
        // well formed, and what it names cannot be done here: the deployment keeps no credentials,
        // has no address for the callback, or a URL the authorization needs is refused by the guard
        UserToolRefusal::NoCredentials
        | UserToolRefusal::OAuthNotConfigured
        | UserToolRefusal::EgressRefused { .. } => (StatusCode::UNPROCESSABLE_ENTITY, None),
        // attaching a server of the person's own that is not ready: a call to action or a cause that
        // clears, and the code says which
        UserToolRefusal::NotReady { .. } => (StatusCode::UNPROCESSABLE_ENTITY, None),
        UserToolRefusal::UrlRefused { .. } => (StatusCode::BAD_REQUEST, None),
        UserToolRefusal::CredentialsUnavailable | UserToolRefusal::AuthorizationUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            Some(RETRY_AFTER_UNAVAILABLE),
        ),
        // the sentence is about the server: "the thread is busy" is for a thread's own contention
        UserToolRefusal::Busy => (StatusCode::SERVICE_UNAVAILABLE, Some(RETRY_AFTER_CONTENDED)),
        // a refusal added later: a bug until it is mapped, and not a client's mistake
        _ => (StatusCode::INTERNAL_SERVER_ERROR, None),
    };
    let problem = Problem::new(status, refusal.to_string());
    (
        match refusal.code() {
            Some(code) => problem.with_code(code),
            None => problem,
        },
        retry,
    )
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
    use orch_app::OwnReason;
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
    fn a_helper_sessions_refusals_carry_their_codes() {
        for (error, status, code) in [
            (
                AppError::blocked("session_closed", "closed"),
                409,
                "session_closed",
            ),
            (AppError::blocked("no_context", "first"), 409, "no_context"),
            (
                AppError::blocked("too_many_starts", "wait"),
                409,
                "too_many_starts",
            ),
            (
                AppError::arrangement("child_mentions"),
                422,
                "child_mentions",
            ),
            (
                AppError::arrangement("session_closes_on_stop"),
                422,
                "session_closes_on_stop",
            ),
            (
                AppError::arrangement("not_child_thread"),
                422,
                "not_child_thread",
            ),
            (
                AppError::arrangement("no_child_sessions"),
                422,
                "no_child_sessions",
            ),
        ] {
            let (problem, retry) = problem_for(&error);
            assert_eq!(problem.status, status, "{error}");
            assert_eq!(problem.code.as_deref(), Some(code), "{error}");
            assert_eq!(retry, None);
        }
    }

    #[test]
    fn status_mapping_table() {
        let coder = AgentId::new("coder");
        let table: Vec<(AppError, u16, Option<&str>)> = vec![
            (AppError::NotFound, 404, None),
            (AppError::Store(StoreError::NotFound), 404, None),
            (AppError::Invalid("bad".into()), 400, None),
            (AppError::Unprocessable("no server `x`".into()), 422, None),
            (AppError::arrangement("bad_anchor"), 422, None),
            (AppError::arrangement("nested_row"), 422, None),
            (AppError::child_thread(), 422, None),
            (AppError::arrangement("child_mentions"), 422, None),
            (AppError::arrangement("session_closes_on_stop"), 422, None),
            (AppError::arrangement("not_child_thread"), 422, None),
            (AppError::blocked("session_closed", "closed"), 409, None),
            (AppError::Finished, 409, None),
            (
                AppError::Transition(TransitionError::InvalidInState {
                    state: ThreadState::Queued,
                    input: "cancel",
                }),
                409,
                None,
            ),
            (
                AppError::MessageLimit {
                    limit: "messages_per_minute",
                    in_use: 2,
                    max: 2,
                    retry_after: Duration::from_secs(30),
                },
                429,
                Some("30"),
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
            // the web sends a request again after a 401: an error of a handler is never one (an agent's own 401 is a 502)
            assert_ne!(status, 401, "{label}");
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

    #[test]
    fn a_server_that_keeps_changing_is_a_503_that_does_not_blame_a_thread() {
        // an add or an edit of one of the person's own servers that another request kept winning
        // against (ADR 0046): the sentence for a thread's own contention does not fit it
        let (problem, retry_after) = problem_for(&AppError::UserTool(UserToolRefusal::Busy));
        assert_eq!(problem.status, 503);
        assert_eq!(
            problem.detail.as_deref(),
            Some("the server is being changed by another request; try again")
        );
        assert_eq!(retry_after, Some(1));
        assert!(problem.code.is_none());
        // the thread's own contention says what it always said
        let (problem, retry_after) = problem_for(&AppError::Contended);
        assert_eq!(
            problem.detail.as_deref(),
            Some("the thread is busy; try again")
        );
        assert_eq!(retry_after, Some(1));
    }

    #[test]
    fn attaching_a_server_that_is_not_ready_is_a_422_whose_code_says_whether_the_person_can_act() {
        // ADR 0046, decision 4: a call to action, or a cause that clears by itself
        for (reason, code) in [
            (OwnReason::NotConnected, "tool_server_needs_auth"),
            (OwnReason::Reauthorize, "tool_server_needs_auth"),
            (OwnReason::Disconnected, "tool_server_needs_auth"),
            (OwnReason::NotPermitted, "tool_server_unavailable"),
            (OwnReason::EgressRefused, "tool_server_unavailable"),
            (OwnReason::BrokerDown, "tool_server_unavailable"),
        ] {
            let (problem, retry_after) =
                problem_for(&AppError::UserTool(UserToolRefusal::NotReady {
                    id: "u-abcdefghij".to_owned(),
                    reason,
                }));
            assert_eq!(problem.status, 422, "{reason:?}");
            assert_eq!(problem.code.as_deref(), Some(code), "{reason:?}");
            assert_eq!(retry_after, None);
            let detail = problem.detail.unwrap();
            assert!(detail.contains("u-abcdefghij"), "{detail}");
            assert!(detail.contains(reason.sentence()), "{detail}");
        }
    }

    #[test]
    fn an_unreadable_card_is_a_503_that_can_be_retried_and_says_which_agent() {
        let (problem, retry_after) = problem_for(&AppError::CardUnavailable {
            agent: AgentId::new("coder"),
        });
        assert_eq!(problem.status, 503);
        assert_eq!(
            problem.detail.as_deref(),
            Some("the card of the agent coder could not be read")
        );
        assert_eq!(retry_after, Some(5));
        // a tier that is not allowed is a 422 and says so
        let (problem, retry_after) = problem_for(&AppError::Unprocessable(
            "this agent does not take the tier 'plan'".to_owned(),
        ));
        assert_eq!(problem.status, 422);
        assert_eq!(retry_after, None);
    }

    #[test]
    fn sharing_refusals_carry_the_codes_a_client_acts_on() {
        use orch_app::SharingMode;
        for (error, status, code) in [
            (AppError::SharingDisabled, 403, "sharing_disabled"),
            (
                AppError::AllowanceSpent {
                    limit: 10,
                    used: 12,
                },
                403,
                "allowance_spent",
            ),
            (
                AppError::OverCap {
                    cap: SharingMode::Internal,
                },
                409,
                "over_cap",
            ),
            (AppError::NotShared, 409, "not_shared"),
            (
                AppError::ThreadActive(orch_core::NotDeletable::Active(ThreadState::Working)),
                409,
                "thread_active",
            ),
            (
                AppError::ThreadActive(orch_core::NotDeletable::AskRunning),
                409,
                "thread_active",
            ),
        ] {
            let (problem, retry) = problem_for(&error);
            assert_eq!(
                (problem.status, problem.code.as_deref()),
                (status, Some(code))
            );
            assert_eq!(retry, None);
        }
        // the cap is named, a secret never is
        let (problem, _) = problem_for(&AppError::OverCap {
            cap: SharingMode::Internal,
        });
        assert_eq!(
            problem.detail.as_deref(),
            Some("this deployment shares at most internal")
        );
    }
}
