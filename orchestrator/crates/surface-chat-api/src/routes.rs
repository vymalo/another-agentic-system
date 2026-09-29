use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_api::{ApiError, ApiJson, ApiQuery, Problem, parse_thread_id};
use orch_app::NewThread;
use orch_core::{AgentTarget, Event, UserId};
use orch_ports::Ports;
use serde::Deserialize;

use crate::State as SurfaceState;

type ApiResult<T> = Result<T, ApiError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewThreadBody {
    title: Option<String>,
    target: AgentTarget,
    text: String,
}

pub(crate) async fn create_thread<P: Ports>(
    State(state): State<SurfaceState<P>>,
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

#[derive(Deserialize)]
pub(crate) struct ListEventsQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

pub(crate) async fn list_events<P: Ports>(
    State(state): State<SurfaceState<P>>,
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
    State(state): State<SurfaceState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<NewMessageBody>,
) -> ApiResult<(StatusCode, Json<Event>)> {
    let id = parse_thread_id(&id)?;
    let event = state.app.post_message(&user, id, body.text).await?;
    Ok((StatusCode::ACCEPTED, Json(event)))
}
