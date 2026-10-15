//! The endpoints for people's own MCP servers (ADR 0046, slice 4b), over [`orch_app::App`]:
//!
//! | Route | Does | Success |
//! |---|---|---|
//! | `POST /api/user-tool-servers` | adds one (`tools.user.add`) | 201 with the server |
//! | `GET /api/user-tool-servers` | lists the person's own, with what each is now | 200 |
//! | `PATCH /api/user-tool-servers/{id}` | edits one (`tools.user.add`), re-keys it | 200 with the server |
//! | `DELETE /api/user-tool-servers/{id}` | deletes one; no permission needed | 204 |
//!
//! `GET /api/tool-servers` lists the same servers beside the deployment's (`routes.rs`).
//!
//! The routes are mounted always and are **404 when the feature is off** (a layer asks the
//! application before the body is read, so an off deployment does not parse a request it does not
//! have). Every handler calls one method of `App`, which holds the permission and owner checks; none
//! is given a port.
//!
//! # The secret
//!
//! The body of `POST` and `PATCH` carries the token or the key in `secret`. It is **write-only**: no
//! response, error, log line or event carries it. This module reads these bodies itself, with no
//! serde message in an answer (serde's `invalid type: string "…"` would quote the value), turns
//! `secret` into a [`ToolSecret`] by moving the string, and answers a body it cannot read with a
//! fixed sentence. Unknown members are refused without naming them (a secret put where a member name
//! goes would be echoed). The request logging records the method and the path, never a body. What the
//! wipe does not cover: the buffer the request body arrived in, and the JSON value a string passed
//! through before it moved.
//!
//! The URL is the person's own and is shown back to them (they must be able to edit it); it can hold
//! a secret in its path, so every response here is `Cache-Control: no-store` and nothing logs it.
//!
//! # A write runs to its end
//!
//! `POST`, `PATCH` and `DELETE` are sequences of writes (the definition, the broker, the definition
//! again) with compensations in between, and axum drops a handler's future when the client hangs up
//! and when the request timeout fires, at its next `await`: every compensation after it is skipped.
//! Each handler therefore runs its `App` call in a task of its own ([`to_the_end`]) and waits for it,
//! as the broker does for a refresh. A request that is dropped still finishes its sequence; its answer
//! goes nowhere.

use std::future::Future;
use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_app::{
    AddUserToolServer, AppError, EditUserToolServer, OwnServers, OwnToolServer, UserToolRefusal,
};
use orch_ports::{MAX_USER_TOOL_SERVERS, Ports, Principal, ServerRevision, ToolSecret, UserAuth};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::ApiState;
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// The largest body of these routes: a secret of 4 KiB, a URL of 2 KiB, 32 scopes of 128 bytes and
/// the rest, with room.
const MAX_BODY_BYTES: usize = 32 * 1024;

const ADD_MEMBERS: &str = "`name`, `url`, `auth` and `secret`";
const EDIT_MEMBERS: &str = "`name`, `url`, `auth`, `secret` and `revision`";
const AUTH_MEMBERS: &str = "`kind`, `header`, `scopes` and `clientId`";

// ------------------------------------------------------------------------- the write runs on

/// Runs a write of `App` in a task of its own and waits for it, so that the request being dropped (the
/// client hangs up, the timeout fires) does not stop it between two of its writes. A task that
/// panicked, or that the runtime ended, is a 500 with the fixed body of every internal error; what
/// went wrong is in the log, by the fact and never by a value.
async fn to_the_end<T, F>(work: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T, AppError>> + Send + 'static,
{
    match tokio::spawn(work).await {
        Ok(done) => done.map_err(ApiError::from),
        Err(ended) => Err(AppError::internal(if ended.is_panic() {
            "a write of people's own MCP servers panicked"
        } else {
            "a write of people's own MCP servers was ended before it finished"
        })
        .into()),
    }
}

// -------------------------------------------------------------------- the feature is off

/// 404 for every route here when the deployment has people's own servers off (ADR 0046): the
/// feature does not exist there, and the body is not read.
pub(crate) async fn require_enabled<P: Ports>(
    State(state): State<ApiState<P>>,
    request: Request,
    next: Next,
) -> Response {
    if state.app.user_tool_servers_enabled() {
        next.run(request).await
    } else {
        ApiError::from(orch_app::AppError::from(UserToolRefusal::Off)).into_response()
    }
}

// ------------------------------------------------------------------------- the body reader

/// The body of a request that may carry a secret, as a JSON object.
///
/// Not [`ApiJson`](crate::ApiJson): its rejections are serde's messages, and serde quotes a string it
/// finds where it expected something else (`invalid type: string "…", expected a map`), which for
/// these routes can be the secret. Every refusal here is a fixed sentence.
pub(crate) struct SecretBody(pub Map<String, Value>);

impl<S: Send + Sync> FromRequest<S> for SecretBody {
    type Rejection = Problem;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json_content_type);
        if !is_json {
            return Err(Problem::bad_request(
                "the body must be JSON (Content-Type: application/json)",
            ));
        }
        let bytes = Bytes::from_request(request, state)
            .await
            .map_err(|_| Problem::bad_request("the body could not be read"))?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(Problem::bad_request(format!(
                "the body is larger than {MAX_BODY_BYTES} bytes"
            )));
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(members)) => Ok(SecretBody(members)),
            Ok(_) => Err(Problem::bad_request("the body must be a JSON object")),
            Err(_) => Err(Problem::bad_request("the body is not valid JSON")),
        }
    }
}

/// A content type is JSON when it is `application/json` or `application/*+json` (what axum's own
/// extractor accepts), parameters aside.
fn is_json_content_type(value: &str) -> bool {
    let essence = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence.split_once('/').is_some_and(|(kind, subtype)| {
        kind == "application" && (subtype == "json" || subtype.ends_with("+json"))
    })
}

// --------------------------------------------------------------------------- the requests

fn unknown_members(members: &str) -> Problem {
    Problem::bad_request(format!("unknown member; the members are {members}"))
}

/// A member that must be text, or `None` when it is not there.
fn text(body: &mut Map<String, Value>, member: &str) -> Result<Option<String>, Problem> {
    match body.remove(member) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(Problem::bad_request(format!("`{member}` must be a string"))),
    }
}

fn required(value: Option<String>, member: &str) -> Result<String, Problem> {
    value.ok_or_else(|| Problem::bad_request(format!("`{member}` is required")))
}

/// `auth` of a request: the shape of the authentication and nothing secret, the same object a
/// response writes.
fn parse_auth(value: Value) -> Result<UserAuth, Problem> {
    let Value::Object(mut auth) = value else {
        return Err(Problem::bad_request("`auth` must be an object"));
    };
    if auth
        .keys()
        .any(|k| !["kind", "header", "scopes", "clientId"].contains(&k.as_str()))
    {
        return Err(Problem::bad_request(format!(
            "unknown member of `auth`; the members are {AUTH_MEMBERS}"
        )));
    }
    let kind =
        text(&mut auth, "kind")?.ok_or_else(|| Problem::bad_request("`auth.kind` is required"))?;
    let parsed = match kind.as_str() {
        "none" => Ok(UserAuth::None),
        "bearer" => Ok(UserAuth::Bearer),
        "api_key" => {
            let header = text(&mut auth, "header")?
                .ok_or_else(|| Problem::bad_request("`auth.header` is required for `api_key`"))?;
            UserAuth::api_key(&header).map_err(|e| Problem::bad_request(e.to_string()))
        }
        "oauth2" => {
            let scopes = match auth.remove("scopes") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .into_iter()
                    .map(|item| match item {
                        Value::String(scope) => Ok(scope),
                        _ => Err(Problem::bad_request(
                            "`auth.scopes` must be an array of strings",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(Problem::bad_request(
                        "`auth.scopes` must be an array of strings",
                    ));
                }
            };
            let client_id = text(&mut auth, "clientId")?;
            UserAuth::oauth2(scopes, client_id).map_err(|e| Problem::bad_request(e.to_string()))
        }
        _ => Err(Problem::bad_request(
            "`auth.kind` is `none`, `bearer`, `api_key` or `oauth2`",
        )),
    }?;
    // what is left is a member of another kind
    if !auth.is_empty() {
        return Err(Problem::bad_request(
            "`auth` has a member that is not for this kind",
        ));
    }
    Ok(parsed)
}

fn parse_add(mut body: Map<String, Value>) -> Result<AddUserToolServer, Problem> {
    if body
        .keys()
        .any(|k| !["name", "url", "auth", "secret"].contains(&k.as_str()))
    {
        return Err(unknown_members(ADD_MEMBERS));
    }
    let name = required(text(&mut body, "name")?, "name")?;
    let url = required(text(&mut body, "url")?, "url")?;
    let auth = parse_auth(
        body.remove("auth")
            .ok_or_else(|| Problem::bad_request("`auth` is required"))?,
    )?;
    let secret = text(&mut body, "secret")?.map(ToolSecret::new);
    Ok(AddUserToolServer {
        name,
        url,
        auth,
        secret,
    })
}

fn parse_edit(mut body: Map<String, Value>) -> Result<EditUserToolServer, Problem> {
    if body
        .keys()
        .any(|k| !["name", "url", "auth", "secret", "revision"].contains(&k.as_str()))
    {
        return Err(unknown_members(EDIT_MEMBERS));
    }
    let name = text(&mut body, "name")?;
    let url = text(&mut body, "url")?;
    let auth = body.remove("auth").map(parse_auth).transpose()?;
    let secret = text(&mut body, "secret")?.map(ToolSecret::new);
    let revision = text(&mut body, "revision")?
        .map(|text| {
            // the schema says `^[0-9]+$`; `u64::from_str` also takes a leading plus
            let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
            digits
                .then(|| text.parse::<u64>().ok())
                .flatten()
                .map(ServerRevision::new)
                .ok_or_else(|| Problem::bad_request("`revision` is the revision the listing gave"))
        })
        .transpose()?;
    Ok(EditUserToolServer {
        name,
        url,
        auth,
        secret,
        revision,
    })
}

// -------------------------------------------------------------------------- the responses

/// Contract `UserToolAuth`: the shape of the authentication, never a secret.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthView<'a> {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scopes: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_id: Option<&'a str>,
}

impl<'a> AuthView<'a> {
    fn of(auth: &'a UserAuth) -> Self {
        AuthView {
            kind: auth.kind(),
            header: auth.header_name(),
            scopes: matches!(auth, UserAuth::OAuth2 { .. }).then(|| auth.scopes()),
            client_id: auth.client_id(),
        }
    }
}

/// Contract `UserToolServer`: one of the person's own servers, with what it is to them now.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserToolServerView<'a> {
    id: &'a str,
    name: &'a str,
    own: bool,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    url: &'a str,
    auth: AuthView<'a>,
    state: &'static str,
    has_credential: bool,
    revision: String,
}

impl<'a> UserToolServerView<'a> {
    pub(crate) fn of(server: &'a OwnToolServer) -> Self {
        UserToolServerView {
            id: server.id().as_str(),
            name: server.name(),
            own: true,
            status: server.status().as_str(),
            reason: server.reason().map(orch_app::OwnReason::as_str),
            url: server.url(),
            auth: AuthView::of(server.auth()),
            state: server.state().as_str(),
            has_credential: server.has_credential(),
            revision: server.revision().get().to_string(),
        }
    }
}

/// Contract `UserToolServers`: the person's list, and why it is empty when it could not be read.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListView<'a> {
    servers: Vec<UserToolServerView<'a>>,
    limit: usize,
    kinds: &'static [&'static str],
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ListError>,
}

#[derive(Serialize)]
struct ListError {
    code: &'static str,
    detail: &'static str,
}

/// An answer that carries the person's URLs: never kept by a cache.
fn no_store(status: StatusCode, body: impl Serialize) -> Response {
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

// ---------------------------------------------------------------------------- the handlers

/// `POST /api/user-tool-servers` (`createUserToolServer`): adds a server of the caller's own (see
/// [`orch_app::App::add_user_tool_server`]). The body is `{name, url, auth, secret?}`; `secret` is
/// the token or the key of a `bearer` or `api_key` server and write-only. 201 with the server.
///
/// 400 for a body or a value that cannot be used (and for a URL the egress guard refuses, with
/// `code: url_refused` and the guard's public reason), 403 without `tools.user.add`, 404 when the
/// feature is off, 409 `too_many_servers` at the limit of eight, 422 `credentials_not_kept` when the
/// deployment keeps no credentials and the server wants one, 503 `broker_down` (with `Retry-After`)
/// when the broker cannot keep the secret now; nothing is left behind then.
pub(crate) async fn create<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    SecretBody(body): SecretBody,
) -> ApiResult<Response> {
    let request = parse_add(body)?;
    let app = Arc::clone(&state.app);
    let server =
        to_the_end(async move { app.add_user_tool_server(&principal, request).await }).await?;
    Ok(no_store(
        StatusCode::CREATED,
        UserToolServerView::of(&server),
    ))
}

/// `GET /api/user-tool-servers` (`listUserToolServers`): the caller's own servers, sorted by name,
/// each with `status` and `reason`, the limit, and the kinds this deployment accepts. Needs no
/// permission beyond the access every route needs. When a definition of theirs does not read back,
/// 200 with no servers and an `error` (`unreadable`): each server can still be deleted by its id.
/// 404 when the feature is off.
pub(crate) async fn list<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
) -> ApiResult<Response> {
    match state.app.list_own_tool_servers(&principal).await? {
        OwnServers::Listed(servers) => Ok(no_store(
            StatusCode::OK,
            ListView {
                servers: servers.iter().map(UserToolServerView::of).collect(),
                limit: MAX_USER_TOOL_SERVERS,
                kinds: state.app.user_tool_kinds(),
                error: None,
            },
        )),
        OwnServers::Unreadable => Ok(no_store(
            StatusCode::OK,
            ListView {
                servers: Vec::new(),
                limit: MAX_USER_TOOL_SERVERS,
                kinds: state.app.user_tool_kinds(),
                error: Some(ListError {
                    code: "unreadable",
                    detail: "a server of yours does not read back, so none can be listed; you \
                             can still delete one by its id, or ask the operator, who can find the \
                             id of the damaged one in the orchestrator's log",
                }),
            },
        )),
        // `Off` and whatever is added later: the feature is not here
        _ => Err(orch_app::AppError::from(UserToolRefusal::Off).into()),
    }
}

/// `PATCH /api/user-tool-servers/{id}` (`updateUserToolServer`): changes a server of the caller's own
/// (see [`orch_app::App::edit_user_tool_server`]). The body has `name`, `url`, `auth`, `secret`
/// (re-keys) and `revision` (the one the listing gave: a server that changed since is 409
/// `stale_revision`), at least one of the first four. A new URL, kind, scopes or client revokes the
/// credential and leaves the server `added`. 200 with the server.
///
/// As `POST`, and 404 for an id that is not the caller's. A change that cannot be made because the
/// deployment keeps no credentials is 422 `credentials_not_kept`.
pub(crate) async fn edit<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    SecretBody(body): SecretBody,
) -> ApiResult<Response> {
    let request = parse_edit(body)?;
    let app = Arc::clone(&state.app);
    let server =
        to_the_end(async move { app.edit_user_tool_server(&principal, &id, request).await })
            .await?;
    Ok(no_store(StatusCode::OK, UserToolServerView::of(&server)))
}

/// `DELETE /api/user-tool-servers/{id}` (`deleteUserToolServer`): deletes a server of the caller's own
/// and forgets its credential (see [`orch_app::App::delete_user_tool_server`]). **Needs no
/// `tools.user.add`**: a person whose permission was taken away can still remove what they added.
/// 204; 404 for an id that is not the caller's and for the second delete; 503 `broker_down` when the
/// credential could not be revoked (the server stays, and they try again).
pub(crate) async fn delete<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let app = Arc::clone(&state.app);
    to_the_end(async move { app.delete_user_tool_server(&principal, &id).await }).await?;
    Ok(StatusCode::NO_CONTENT)
}
