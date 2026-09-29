use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_core::{AgentInfo, ThreadId, ThreadRecord, UserId};
use orch_ports::Ports;
use serde::Deserialize;

use crate::ApiState;
use crate::extract::ApiQuery;
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// Parses a `{threadId}`; anything that is not a UUID is simply a thread that does not exist.
///
/// Public because every surface that addresses a thread by path needs the same rule.
pub fn parse_thread_id(raw: &str) -> Result<ThreadId, ApiError> {
    raw.parse::<ThreadId>()
        .map_err(|_| ApiError::App(orch_app::AppError::NotFound))
}

pub(crate) async fn healthz<P: Ports>(State(state): State<ApiState<P>>) -> Response {
    if state.app.is_shutting_down() {
        plain(StatusCode::SERVICE_UNAVAILABLE, "shutting down")
    } else {
        plain(StatusCode::OK, "ok")
    }
}

pub(crate) async fn readyz<P: Ports>(State(state): State<ApiState<P>>) -> Response {
    if state.app.is_ready().await {
        plain(StatusCode::OK, "ready")
    } else {
        plain(StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

fn plain(status: StatusCode, body: &'static str) -> Response {
    let mut r = (status, body).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

pub(crate) async fn not_found() -> Problem {
    Problem::not_found("no such route")
}

pub(crate) async fn method_not_allowed() -> Problem {
    Problem::new(StatusCode::METHOD_NOT_ALLOWED, "method not allowed")
}

pub(crate) async fn list_agents<P: Ports>(
    State(state): State<ApiState<P>>,
) -> Json<Vec<AgentInfo>> {
    Json(state.app.list_agents().await)
}

#[derive(Deserialize)]
pub(crate) struct ListThreadsQuery {
    limit: Option<i64>,
    before: Option<String>,
}

pub(crate) async fn list_threads<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    ApiQuery(q): ApiQuery<ListThreadsQuery>,
) -> ApiResult<Json<Vec<ThreadRecord>>> {
    let limit = q.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(Problem::bad_request("limit must be between 1 and 100").into());
    }
    let before = q
        .before
        .map(|b| {
            b.parse::<ThreadId>()
                .map_err(|_| Problem::bad_request("before must be a thread id"))
        })
        .transpose()?;
    let limit = u32::try_from(limit).unwrap_or(50);
    Ok(Json(state.app.list_threads(&user, before, limit).await?))
}

pub(crate) async fn get_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<Json<ThreadRecord>> {
    Ok(Json(
        state.app.get_thread(&user, parse_thread_id(&id)?).await?,
    ))
}

pub(crate) async fn cancel_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.app.cancel(&user, parse_thread_id(&id)?).await?;
    Ok(StatusCode::ACCEPTED)
}
