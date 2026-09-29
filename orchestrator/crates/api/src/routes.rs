use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_app::NewThread;
use orch_core::{AgentInfo, AgentTarget, Event, ThreadId, ThreadRecord, UserId};
use orch_ports::Ports;
use serde::Deserialize;

use crate::ApiState;
use crate::extract::{ApiJson, ApiQuery};
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// Parses a `{threadId}`; anything that is not a UUID is simply a thread that does not exist.
pub(crate) fn parse_thread_id(raw: &str) -> Result<ThreadId, ApiError> {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewThreadBody {
    title: Option<String>,
    target: AgentTarget,
    text: String,
}

pub(crate) async fn create_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    ApiJson(body): ApiJson<NewThreadBody>,
) -> ApiResult<Response> {
    let thread = state
        .app
        .create_thread(
            &user,
            NewThread {
                title: body.title,
                target: body.target,
                text: body.text,
            },
        )
        .await?;
    let location = format!("/api/threads/{}", thread.id);
    let mut response = (StatusCode::CREATED, Json(thread)).into_response();
    if let Ok(value) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    Ok(response)
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

#[derive(Deserialize)]
pub(crate) struct ListEventsQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

pub(crate) async fn list_events<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<ListEventsQuery>,
) -> ApiResult<Json<Vec<Event>>> {
    let id = parse_thread_id(&id)?;
    let after = q.after.unwrap_or(0);
    if after < 0 {
        return Err(Problem::bad_request("after must be at least 0").into());
    }
    let limit = q.limit.unwrap_or(200);
    if !(1..=500).contains(&limit) {
        return Err(Problem::bad_request("limit must be between 1 and 500").into());
    }
    let limit = u32::try_from(limit).unwrap_or(200);
    Ok(Json(state.app.list_events(&user, id, after, limit).await?))
}

#[derive(Deserialize)]
pub(crate) struct NewMessageBody {
    text: String,
}

pub(crate) async fn post_message<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<NewMessageBody>,
) -> ApiResult<(StatusCode, Json<Event>)> {
    let id = parse_thread_id(&id)?;
    let event = state.app.post_message(&user, id, body.text).await?;
    Ok((StatusCode::ACCEPTED, Json(event)))
}

pub(crate) async fn cancel_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.app.cancel(&user, parse_thread_id(&id)?).await?;
    Ok(StatusCode::ACCEPTED)
}
