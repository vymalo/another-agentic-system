use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_core::{
    AgentInfo, AgentTarget, ThreadId, ThreadRecord, UserId, check_description, check_title,
};
use orch_ports::{Authenticator, Ports};
use serde::{Deserialize, Serialize};

use crate::ApiState;
use crate::extract::{ApiJson, ApiQuery};
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
    if !state.app.is_ready().await {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "not ready");
    }
    // Ready to serve means ready to authenticate: while the issuer's keys have never been
    // fetched, every request would be refused (ADR 0033).
    if let Err(error) = state.app.ports().auth().ready().await {
        tracing::warn!(%error, "not ready: cannot authenticate");
        return plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "not ready: cannot authenticate",
        );
    }
    plain(StatusCode::OK, "ready")
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
    Json(state.app.list_agents().await.agents)
}

/// Contract `Registry`: how each source of agents answered on the last read.
#[derive(Serialize)]
pub(crate) struct RegistryStatus {
    sources: Vec<SourceView>,
}

/// Contract `RegistrySource`.
#[derive(Serialize)]
pub(crate) struct SourceView {
    name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

/// `GET /api/registry`: whether each source of agents (the deployment's own list, the platform's
/// registry) could be read, so a client can say when the list is incomplete. The registry is read
/// now and the answer is never cached; no agent card is read, and no URL or credential is in it.
pub(crate) async fn registry_status<P: Ports>(State(state): State<ApiState<P>>) -> Response {
    let sources = state
        .app
        .registry_sources()
        .await
        .into_iter()
        .map(|source| SourceView {
            name: source.name,
            status: if source.available {
                "ok"
            } else {
                "unavailable"
            },
            // A source that answered has nothing to explain.
            detail: source.detail.filter(|_| !source.available),
        })
        .collect();
    let mut response = Json(RegistryStatus { sources }).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
pub(crate) struct ListThreadsQuery {
    limit: Option<i64>,
    before: Option<String>,
    branches: Option<String>,
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
    // The threads made by an edit are branches of one in the list: hidden unless asked for.
    let include_edits = match q.branches.as_deref() {
        None => false,
        Some("include") => true,
        Some(_) => return Err(Problem::bad_request("branches must be `include`").into()),
    };
    Ok(Json(
        state
            .app
            .list_threads(&user, before, limit, include_edits)
            .await?,
    ))
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

/// Changes what a person writes about the thread (see [`orch_app::App::rename_thread`] and
/// [`orch_app::App::describe_thread`]): 200 with the thread, 400 for a title or a description that
/// cannot be used, 404 for a thread that is not the caller's.
///
/// The body is an object with a `title` string and/or a `description` string (empty clears it) and
/// nothing else, and at least one: a member this API does not know is refused, so that a client
/// that thinks it can change more learns it cannot. Both are checked before either is written, so
/// a 400 changes nothing. (It is read as a map, not as a struct, because a struct also reads from
/// a JSON array.)
pub(crate) async fn patch_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Map<String, serde_json::Value>>,
) -> ApiResult<Json<ThreadRecord>> {
    let id = parse_thread_id(&id)?;
    let (mut title, mut description) = (None, None);
    for (member, value) in body {
        match (member.as_str(), value) {
            ("title", serde_json::Value::String(text)) => title = Some(text),
            ("title", _) => return Err(Problem::bad_request("`title` must be a string").into()),
            ("description", serde_json::Value::String(text)) => description = Some(text),
            ("description", _) => {
                return Err(Problem::bad_request("`description` must be a string").into());
            }
            (other, _) => {
                return Err(Problem::bad_request(format!("unknown member `{other}`")).into());
            }
        }
    }
    if title.is_none() && description.is_none() {
        return Err(Problem::bad_request("`title` or `description` is required").into());
    }
    // Both are checked before either is written (the application checks again).
    if let Some(title) = &title {
        check_title(title).map_err(|e| Problem::bad_request(e.to_string()))?;
    }
    if let Some(description) = &description {
        check_description(description).map_err(|e| Problem::bad_request(e.to_string()))?;
    }
    let mut thread = None;
    if let Some(title) = title {
        thread = Some(state.app.rename_thread(&user, id, &title).await?);
    }
    if let Some(description) = description {
        thread = Some(state.app.describe_thread(&user, id, &description).await?);
    }
    match thread {
        Some(thread) => Ok(Json(thread)),
        None => Err(Problem::bad_request("`title` or `description` is required").into()),
    }
}

/// `GET /api/config`: the public subset of the configuration, exactly `{"ui": {...}}`
/// ([ADR 0034](../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md)).
/// Behind the identity layer like every `/api` route. It changes only when the process restarts.
pub(crate) async fn public_config<P: Ports>(
    State(state): State<ApiState<P>>,
) -> Json<orch_app::PublicConfig> {
    Json(state.app.public_config().clone())
}

pub(crate) async fn cancel_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.app.cancel(&user, parse_thread_id(&id)?).await?;
    Ok(StatusCode::ACCEPTED)
}

/// The thread as one downloadable JSON document (see [`crate::export`]). Authorised exactly like
/// reading the thread: the same identity layer, the same owner-only read, the same 404 for a
/// thread that is someone else's or does not exist.
pub(crate) async fn export_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let id = parse_thread_id(&id)?;
    let export = state.app.export_thread(&user, id).await?;
    // Pretty, because a person opens it, and a developer diffs it.
    let body = serde_json::to_vec_pretty(&crate::export::document(&export))
        .map_err(|e| orch_app::AppError::internal(format!("export document: {e}")))?;
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    // The id is a UUID, so the file name needs no escaping.
    if let Ok(disposition) =
        HeaderValue::from_str(&format!("attachment; filename=\"thread-{id}.json\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    // The chat, with whatever it holds: never kept by a shared cache or the browser's.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The body of `POST /api/threads/{id}/fork`: where to cut (`after`, or `replace` with the new
/// `text`), and optionally the agent the fork talks to and the id of the new thread.
/// The longest `messageId` a fork's message may carry (`ForkRequest.messageId`, as AG-UI's ids).
const MAX_MESSAGE_ID_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ForkBody {
    after: Option<i64>,
    replace: Option<i64>,
    text: Option<String>,
    message_id: Option<String>,
    target: Option<AgentTarget>,
    id: Option<ThreadId>,
}

impl ForkBody {
    fn into_request(self) -> Result<orch_app::ForkRequest, Problem> {
        let at = match (self.after, self.replace) {
            (Some(seq), None) => {
                if self.text.is_some() || self.message_id.is_some() {
                    return Err(Problem::bad_request(
                        "`text` and `messageId` go with `replace`, not `after`",
                    ));
                }
                orch_app::ForkAt::AfterTurn { seq }
            }
            (None, Some(seq)) => {
                if self
                    .message_id
                    .as_deref()
                    .is_some_and(|id| id.len() > MAX_MESSAGE_ID_BYTES)
                {
                    return Err(Problem::bad_request(format!(
                        "`messageId` must be at most {MAX_MESSAGE_ID_BYTES} bytes"
                    )));
                }
                orch_app::ForkAt::Replace {
                    seq,
                    text: self
                        .text
                        .ok_or_else(|| Problem::bad_request("`replace` needs the new `text`"))?,
                    message_id: self.message_id,
                }
            }
            (Some(_), Some(_)) => {
                return Err(Problem::bad_request("give `after` or `replace`, not both"));
            }
            (None, None) => return Err(Problem::bad_request("give `after` or `replace`")),
        };
        Ok(orch_app::ForkRequest {
            at,
            target: self.target,
            id: self.id,
        })
    }
}

/// Forks the thread (see [`orch_app::App::fork_thread`]): 201 with the new thread and its
/// `Location`; 200 with the existing one when the body's `id` is a fork of this thread made
/// already. 400 for a body that cannot be read or a text or target that cannot be used, 404 for a
/// thread that is not the caller's, 409 while the turn is going on (`turn_open`) or for an id
/// another thread has, 422 for a point that is not in the log or not a person's message.
///
/// The body is read as an object first, so that an array or a member this API does not know is
/// refused, not skipped.
pub(crate) async fn fork_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Map<String, serde_json::Value>>,
) -> ApiResult<Response> {
    let id = parse_thread_id(&id)?;
    let body: ForkBody = serde_json::from_value(serde_json::Value::Object(body))
        .map_err(|e| Problem::bad_request(format!("invalid body: {e}")))?;
    let forked = state
        .app
        .fork_thread(&user, id, body.into_request()?)
        .await?;
    let status = if forked.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let location = format!("/api/threads/{}", forked.thread.id);
    let mut response = (status, Json(forked.thread)).into_response();
    if let Ok(location) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// Contract `Branches`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BranchesView {
    root: ThreadId,
    points: Vec<BranchPointView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchPointView {
    seq: i64,
    index: usize,
    siblings: Vec<SiblingBody>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SiblingBody {
    thread_id: ThreadId,
    seq: i64,
    title: String,
}

/// The messages of the thread that have other versions (see [`orch_app::App::branches`]).
pub(crate) async fn list_branches<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(user): Extension<UserId>,
    Path(id): Path<String>,
) -> ApiResult<Json<BranchesView>> {
    let branches = state.app.branches(&user, parse_thread_id(&id)?).await?;
    Ok(Json(BranchesView {
        root: branches.root,
        points: branches
            .points
            .into_iter()
            .map(|p| BranchPointView {
                seq: p.seq,
                index: p.index,
                siblings: p
                    .siblings
                    .into_iter()
                    .map(|s| SiblingBody {
                        thread_id: s.thread_id,
                        seq: s.seq,
                        title: s.title,
                    })
                    .collect(),
            })
            .collect(),
    }))
}
