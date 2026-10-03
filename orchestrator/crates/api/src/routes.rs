use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_app::{Permission, Scope};
use orch_core::{
    AgentInfo, AgentTarget, ShareLevel, ThreadId, ThreadRecord, Timestamp, Visibility,
    check_description, check_title,
};
use orch_ports::{Authenticator, Ports, Principal};
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
    Extension(principal): Extension<Principal>,
) -> ApiResult<Json<Vec<AgentInfo>>> {
    Ok(Json(state.app.list_agents(&principal).await?.agents))
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
pub(crate) async fn registry_status<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
) -> ApiResult<Response> {
    let sources = state
        .app
        .registry_sources(&principal)
        .await?
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
    Ok(response)
}

#[derive(Deserialize)]
pub(crate) struct ListThreadsQuery {
    limit: Option<i64>,
    before: Option<String>,
    branches: Option<String>,
    owner: Option<String>,
}

pub(crate) async fn list_threads<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    ApiQuery(q): ApiQuery<ListThreadsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
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
    // Only the caller's own: there is no listing of another person's threads or of everyone's,
    // for any role (ADR 0039). A client that still asks is told, not silently given its own.
    if q.owner.is_some() {
        return Err(Problem::bad_request("owner is not supported (ADR 0039)").into());
    }
    let threads = state
        .app
        .list_threads(&principal, before, limit, include_edits)
        .await?;
    // The owner's own list says which of their threads are shared and at what: the sidebar shows
    // it. The link is not in a list, only in the thread itself.
    Ok(Json(
        threads
            .iter()
            .map(|thread| thread_json(&state, thread, false))
            .collect(),
    ))
}

/// The thread as the contract's `Thread`, with the `share` of its owner's view when it is shared
/// (ADR 0040): what it is stored as, what is served now and, when `with_url`, the link. Only the
/// owner ever gets the link, and these routes are the owner's.
fn thread_json<P: Ports>(
    state: &ApiState<P>,
    thread: &ThreadRecord,
    with_url: bool,
) -> serde_json::Value {
    let mut value = serde_json::to_value(thread).unwrap_or(serde_json::Value::Null);
    if let (Some(view), Some(object)) = (state.app.share_view(thread), value.as_object_mut()) {
        let mut share = serde_json::Map::new();
        share.insert("visibility".to_owned(), view.visibility.as_str().into());
        share.insert("effective".to_owned(), view.effective.as_str().into());
        if with_url && let Some(url) = view.url {
            share.insert("url".to_owned(), url.into());
        }
        object.insert("share".to_owned(), serde_json::Value::Object(share));
    }
    value
}

pub(crate) async fn get_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let thread = state
        .app
        .get_thread(&principal, parse_thread_id(&id)?)
        .await?;
    let shared = thread.share.is_some();
    let mut response = Json(thread_json(&state, &thread, true)).into_response();
    if shared {
        // It carries the link: never kept by a cache.
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
}

/// What the owner is told of a share: `PUT`, `POST …/rotate`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareResponse {
    visibility: ShareLevel,
    effective: Visibility,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    shared_at: Timestamp,
}

fn share_response(view: orch_app::ShareView) -> Response {
    let mut response = Json(ShareResponse {
        visibility: view.visibility,
        effective: view.effective,
        url: view.url,
        shared_at: view.shared_at,
    })
    .into_response();
    // It carries the link.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `PUT /api/threads/{threadId}/share` with `{"visibility": "internal" | "public"}`: shares the
/// thread, widens or narrows its share (see [`orch_app::App::share_thread`]). 200 with what the
/// owner is told, link included; the same visibility as the thread has is the current link and
/// writes nothing. 400 for a body that is not exactly that, `private` included (stop sharing with
/// `DELETE`), 403 without `thread.share` and 403 `sharing_disabled` under a `disabled` cap, 404 for
/// a thread that is not the caller's, 409 `over_cap` above the deployment's cap.
pub(crate) async fn put_share<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Map<String, serde_json::Value>>,
) -> ApiResult<Response> {
    let id = parse_thread_id(&id)?;
    let mut visibility = None;
    for (member, value) in body {
        match (member.as_str(), value) {
            ("visibility", serde_json::Value::String(text)) => visibility = Some(text),
            ("visibility", _) => {
                return Err(Problem::bad_request("`visibility` must be a string").into());
            }
            (other, _) => {
                return Err(Problem::bad_request(format!("unknown member `{other}`")).into());
            }
        }
    }
    let level = match visibility.as_deref() {
        Some("internal") => ShareLevel::Internal,
        Some("public") => ShareLevel::Public,
        Some("private") => {
            return Err(Problem::bad_request(
                "to stop sharing, DELETE the share; `visibility` is `internal` or `public`",
            )
            .into());
        }
        Some(_) => {
            return Err(Problem::bad_request("`visibility` is `internal` or `public`").into());
        }
        None => return Err(Problem::bad_request("`visibility` is required").into()),
    };
    Ok(share_response(
        state.app.share_thread(&principal, id, level).await?,
    ))
}

/// `POST /api/threads/{threadId}/share/rotate`: a new link, so the old one is a 404 from now on, at
/// the visibility the thread has (see [`orch_app::App::rotate_share`]). 200 as `PUT`; 403 without
/// `thread.share` or `sharing_disabled`; 404 for a thread that is not the caller's; 409
/// `not_shared` for a thread that is private.
pub(crate) async fn rotate_share<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    Ok(share_response(
        state
            .app
            .rotate_share(&principal, parse_thread_id(&id)?)
            .await?,
    ))
}

/// `DELETE /api/threads/{threadId}/share`: takes the link down (see
/// [`orch_app::App::unshare_thread`]). 204, also for a thread that is not shared. **Needs only
/// ownership** and is never refused for the cap: 404 for a thread that is not the caller's, and
/// nothing else.
pub(crate) async fn delete_share<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state
        .app
        .unshare_thread(&principal, parse_thread_id(&id)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changes what a person writes about the thread (see [`orch_app::App::rename_thread`] and
/// [`orch_app::App::describe_thread`]): 200 with the thread, 400 for a title or a description that
/// cannot be used, 403 without `thread.write`, 404 for a thread that is not the caller's.
///
/// The body is an object with a `title` string and/or a `description` string (empty clears it) and
/// nothing else, and at least one: a member this API does not know is refused, so that a client
/// that thinks it can change more learns it cannot. Both are checked before either is written, so
/// a 400 changes nothing. (It is read as a map, not as a struct, because a struct also reads from
/// a JSON array.)
pub(crate) async fn patch_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
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
        thread = Some(state.app.rename_thread(&principal, id, &title).await?);
    }
    if let Some(description) = description {
        thread = Some(
            state
                .app
                .describe_thread(&principal, id, &description)
                .await?,
        );
    }
    match thread {
        Some(thread) => Ok(Json(thread)),
        None => Err(Problem::bad_request("`title` or `description` is required").into()),
    }
}

/// Contract `ToolServer`: a server a person may attach to a conversation (ADR 0024). The URL, the
/// headers, the credentials, the allow-list of its tools and the timeout are not here and never
/// will be: this is what the picker shows.
#[derive(Serialize)]
pub(crate) struct ToolServerView {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// A `data:` URI, drawn as it is; never a URL to fetch.
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    /// The agents it may be attached for; absent when it may be for every agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    agents: Option<Vec<String>>,
}

/// `GET /api/tool-servers`: the servers the deployment offers for attaching, in the order it lists
/// them. 403 for a person whose roles do not grant `thread.write`.
pub(crate) async fn list_tool_servers<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
) -> ApiResult<Response> {
    let servers: Vec<ToolServerView> = state
        .app
        .list_tool_servers(&principal)?
        .iter()
        .map(|s| ToolServerView {
            id: s.id.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            icon: s.icon.clone(),
            agents: s
                .agents
                .as_ref()
                .map(|agents| agents.iter().map(ToString::to_string).collect()),
        })
        .collect();
    let mut response = Json(servers).into_response();
    // The deployment's list changes with its configuration, not with the request.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Contract `ThreadTools`: the servers attached to a thread, after a change.
#[derive(Serialize)]
pub(crate) struct ThreadTools {
    servers: Vec<String>,
}

/// `PUT /api/threads/{threadId}/tools` with `{"servers": [ids]}`: sets the MCP servers attached to
/// a thread, whatever its state (see [`orch_app::App::set_tools`]). 200 with the set after the
/// change (the same set is a 200 with no change), 400 for a body that is not exactly an object with
/// a `servers` array of strings or an id that is not one, 403 without `thread.write`, 404 for a
/// thread that is not the caller's, 422 for a
/// server that is unknown or not offered for the thread's agent, and for more than 16.
pub(crate) async fn put_thread_tools<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Map<String, serde_json::Value>>,
) -> ApiResult<Json<ThreadTools>> {
    let id = parse_thread_id(&id)?;
    let mut servers = None;
    for (member, value) in body {
        match (member.as_str(), value) {
            ("servers", serde_json::Value::Array(items)) => {
                let ids = items
                    .into_iter()
                    .map(|item| match item {
                        serde_json::Value::String(id) => Ok(id),
                        _ => Err(Problem::bad_request(
                            "`servers` must be an array of strings",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                servers = Some(ids);
            }
            ("servers", _) => {
                return Err(Problem::bad_request("`servers` must be an array of strings").into());
            }
            (other, _) => {
                return Err(Problem::bad_request(format!("unknown member `{other}`")).into());
            }
        }
    }
    let servers = servers.ok_or_else(|| Problem::bad_request("`servers` is required"))?;
    let thread = state.app.set_tools(&principal, id, servers).await?;
    Ok(Json(ThreadTools {
        servers: thread.job.tools,
    }))
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
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.app.cancel(&principal, parse_thread_id(&id)?).await?;
    Ok(StatusCode::ACCEPTED)
}

/// The thread as one downloadable JSON document (see [`crate::export`]). Authorised exactly like
/// reading the thread: the same identity layer, the same `thread.read`, the same 404 for a thread
/// the caller may not read or that does not exist.
pub(crate) async fn export_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let id = parse_thread_id(&id)?;
    let export = state.app.export_thread(&principal, id).await?;
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

/// The longest `messageId` a fork's message may carry (`ForkRequest.messageId`, as AG-UI's ids).
const MAX_MESSAGE_ID_BYTES: usize = 256;

/// The body of `POST /api/threads/{id}/fork`: where to cut (`after`, with a `text` when the fork is
/// made with its first message, or `replace` with the new `text`), and optionally the agent the
/// fork talks to and the id of the new thread.
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
        if self
            .message_id
            .as_deref()
            .is_some_and(|id| id.len() > MAX_MESSAGE_ID_BYTES)
        {
            return Err(Problem::bad_request(format!(
                "`messageId` must be at most {MAX_MESSAGE_ID_BYTES} bytes"
            )));
        }
        let at = match (self.after, self.replace) {
            // The fork is made with its first message when there is one (ADR 0042, decision 8):
            // no mentions and no UI catalog travel on this route, the AG-UI run carries them.
            (Some(seq), None) => {
                if self.message_id.is_some() && self.text.is_none() {
                    return Err(Problem::bad_request("`messageId` goes with `text`"));
                }
                orch_app::ForkAt::AfterTurn {
                    seq,
                    first: self.text.map(|text| orch_app::FirstMessage {
                        text,
                        message_id: self.message_id,
                        run_id: None,
                        origin: orch_core::Origin::default(),
                        ui_catalog: None,
                        mentions: Vec::new(),
                    }),
                }
            }
            (None, Some(seq)) => orch_app::ForkAt::Replace {
                seq,
                text: self
                    .text
                    .ok_or_else(|| Problem::bad_request("`replace` needs the new `text`"))?,
                message_id: self.message_id,
            },
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
/// already (for `after` with `text`, the fork that very request made). `after` with `text` makes
/// the fork with its first message, `queued` (ADR 0042). 400 for a body that cannot be read or a text or target that cannot be used, 403 for an
/// agent their roles do not allow, 404 for a thread that is not the caller's, 409 while the turn is going on (`turn_open`) or for an id
/// another thread has, 422 for a point that is not in the log or not a person's message.
///
/// The body is read as an object first, so that an array or a member this API does not know is
/// refused, not skipped.
pub(crate) async fn fork_thread<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<serde_json::Map<String, serde_json::Value>>,
) -> ApiResult<Response> {
    let id = parse_thread_id(&id)?;
    let body: ForkBody = serde_json::from_value(serde_json::Value::Object(body))
        .map_err(|e| Problem::bad_request(format!("invalid body: {e}")))?;
    let forked = state
        .app
        .fork_thread(&principal, id, body.into_request()?)
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
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<BranchesView>> {
    let branches = state
        .app
        .branches(&principal, parse_thread_id(&id)?)
        .await?;
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

/// Contract `Me`: who the caller is and what their roles let them do.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Me {
    user: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    roles: Vec<String>,
    permissions: Vec<PermissionView>,
    agents: AgentsView,
    /// What the deployment lets this person share their threads as (ADR 0040): its cap when their
    /// roles hold `thread.share`, else `disabled`.
    sharing: &'static str,
}

/// Contract `Permission`.
#[derive(Serialize)]
struct PermissionView {
    permission: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'static str>,
}

/// Contract `MeAgents`: the agents the person's roles name, for each agent permission.
#[derive(Serialize)]
struct AgentsView {
    read: Vec<String>,
    invoke: Vec<String>,
}

/// `GET /api/me`: who the caller is, the roles that count, and what those roles grant. It is for
/// a client to show what its person may do and hide what they may not; it is never a check, the
/// orchestrator enforces every request. It answers a person whose roles grant nothing (every
/// other route is 403 for them), so a client can say why.
pub(crate) async fn me<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let access = state.app.access(&principal);
    let me = Me {
        user: principal.user.to_string(),
        email: principal.email.clone(),
        name: principal.name.clone(),
        roles: access.roles().map(ToString::to_string).collect(),
        permissions: access
            .permissions()
            .into_iter()
            .map(|(permission, scope)| PermissionView {
                permission: permission.as_str(),
                scope: scope.map(Scope::as_str),
            })
            .collect(),
        agents: AgentsView {
            read: access.agents(Permission::AgentRead).patterns(),
            invoke: access.agents(Permission::AgentInvoke).patterns(),
        },
        sharing: state.app.sharing_mode_for(&principal).as_str(),
    };
    let mut response = Json(me).into_response();
    // Who a person is and what they may do changes with their token: never kept.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
