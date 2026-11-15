//! `/api/prompts`: a person's prompt library (ADR 0070, `docs/api/prompts-v1.md`).
//!
//! | Route | What | Answer |
//! |---|---|---|
//! | `GET /api/prompts?scope=&cursor=` | the caller's own prompts and the ones shared with their roles, as summaries (no body, no variables), by id | 200 `{limit, held, items, nextCursor?}` |
//! | `POST /api/prompts` | saves one | 201 with the prompt |
//! | `GET /api/prompts/{id}` | one, the caller's or shared with them | 200 |
//! | `PATCH /api/prompts/{id}` | `{revision, ...}`: replaces the members it names | 200 |
//! | `DELETE /api/prompts/{id}?revision=` | deletes the caller's own | 204 |
//! | `POST /api/prompts/{id}/render` | `{revision?, answers}`: the text of the prompt filled, **nothing sent, nothing written** | 200 `{text, revision}` |
//!
//! The rules are `orch_core::prompt` and the application is `orch_app::App::*_prompt`; this module reads
//! the request, in the order of the page (the media type, the id, the shape of the body, then the
//! application), and writes the answer. **Nothing of a prompt's words or of an answer is logged**: the
//! request log has the method and the path (ids only), a body that does not parse is answered in fixed
//! words (never serde's, which may quote the value), and no `tracing` field here holds a title, a body,
//! an answer or a role. **A shared prompt never names its author**: the views have no owner, and
//! `audience` and nothing else of the share is the owner's own.
//!
//! Every answer under `/api/prompts`, the problems included (a 401, a `no_access`, a 404 for a path
//! that is no route, a 405, the timeout's 503), is `Cache-Control: no-store`: [`no_store_for_prompts`]
//! is the outermost layer of the whole router, outside the guard and the identity layer.
//!
//! A member twice in any object of a request, a variable's or an answer's included, is a 400:
//! `serde_json` keeps the last of two, which is not what the person wrote ([`Strict`]).

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_app::{EditPrompt, RenderPrompt};
use orch_core::prompt::{
    MAX_BODY_BYTES, MAX_PROMPTS, MAX_RENDERED_BYTES, MAX_REQUEST_BYTES, MAX_VARIABLES, PromptId,
    PromptInput,
};
use orch_ports::{Ports, Principal, PromptRevision, PromptScope, PromptSummary, StoredPrompt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ApiState;
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// `prompts` of `GET /api/config`: its presence is the capability, its numbers what is enforced.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PromptsCapability {
    limit: usize,
    max_variables: usize,
    max_body_bytes: usize,
    max_rendered_bytes: usize,
}

impl Default for PromptsCapability {
    fn default() -> Self {
        PromptsCapability {
            limit: MAX_PROMPTS,
            max_variables: MAX_VARIABLES,
            max_body_bytes: MAX_BODY_BYTES,
            max_rendered_bytes: MAX_RENDERED_BYTES,
        }
    }
}

/// A member that is there is its type: `null` is not absent, it is the wrong type, as for any other.
fn not_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A 400 with the code `bad_request` and fixed words.
fn bad_request(detail: &str) -> ApiError {
    Problem::bad_request(detail).with_code("bad_request").into()
}

fn bad_id() -> ApiError {
    bad_request("the prompt id is `p-` and 12 lowercase letters or digits")
}

/// Every answer under `/api/prompts`, problems included, is never kept by a cache. It is the outermost
/// layer of the router, so the answers of the layers inside (the identity layer's 401, the guard's
/// `no_access`, the timeout's 503, the fallbacks' 404 and 405) carry it too.
pub(crate) async fn no_store_for_prompts(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let ours = path == "/api/prompts" || path.starts_with("/api/prompts/");
    let mut response = next.run(request).await;
    if ours {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// A JSON value that refuses an object with a member twice, at any depth. `serde_json::Value` keeps
/// the last of two, silently: a client that sent `"question"` twice meant one of them, and the server
/// cannot say which. The refusal is a bare error: the key is never quoted.
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = Value;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
                Ok(Value::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
                Ok(v.into())
            }
            fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
                Ok(v.into())
            }
            fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
                Ok(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
            }
            fn visit_str<E>(self, v: &str) -> Result<Value, E> {
                Ok(Value::String(v.to_owned()))
            }
            fn visit_string<E>(self, v: String) -> Result<Value, E> {
                Ok(Value::String(v))
            }
            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_none<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Value, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(item)) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Value::Array(items))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Value, A::Error> {
                let mut members = serde_json::Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if members.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("a member twice"));
                    }
                }
                Ok(Value::Object(members))
            }
        }
        deserializer.deserialize_any(Visit).map(Strict)
    }
}

/// `deserialize_with` of a member that is any JSON value: not `null`, and no member twice.
fn strict_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Strict::deserialize(deserializer).map(|Strict(v)| Some(v))
}

/// `deserialize_with` of the list of variables: not `null`, and no member twice in any of them.
fn strict_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<Value>>, D::Error> {
    match Strict::deserialize(deserializer)?.0 {
        Value::Array(items) => Ok(Some(items)),
        _ => Err(serde::de::Error::custom("not a list")),
    }
}

fn parse_id(text: &str) -> ApiResult<PromptId> {
    PromptId::parse(text).map_err(|_| bad_id())
}

/// `^[1-9][0-9]{0,18}$`.
fn parse_revision(text: &str) -> ApiResult<PromptRevision> {
    let digits = (1..=19).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_digit())
        && !text.starts_with('0');
    digits
        .then(|| text.parse::<u64>().ok())
        .flatten()
        .map(PromptRevision::new)
        .ok_or_else(|| bad_request("a revision is the digits the last read gave"))
}

/// The request's media type: exactly `application/json` (parameters such as `charset` allowed, the
/// case ignored), else 415.
fn require_json(request: &Request) -> ApiResult<()> {
    let json = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"));
    if json {
        Ok(())
    } else {
        Err(Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the body is application/json",
        )
        .into())
    }
}

/// The body, at most [`MAX_REQUEST_BYTES`], parsed into `T` with **fixed words** for whatever does not
/// parse: serde's messages may quote the value.
async fn read<T: serde::de::DeserializeOwned>(request: Request) -> ApiResult<T> {
    let bytes: Bytes = axum::body::to_bytes(request.into_body(), MAX_REQUEST_BYTES)
        .await
        .map_err(|_| bad_request("the request is too large or could not be read"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad_request("the body is not the JSON object of this route"))
}

/// The pairs of the query string, in order.
fn query_pairs(uri: &Uri) -> Vec<(String, String)> {
    uri.query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn first<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

// ------------------------------------------------------------------------------------ the views

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PromptView {
    id: String,
    name: String,
    title: String,
    description: String,
    body: String,
    variables: Vec<Value>,
    own: bool,
    /// The owner's own view only: a person the prompt is shared with never learns the roles.
    #[serde(skip_serializing_if = "Option::is_none")]
    audience: Option<Vec<String>>,
    revision: String,
    updated_at: jiff::Timestamp,
}

impl PromptView {
    fn of(stored: &StoredPrompt) -> Self {
        let p = stored.prompt();
        PromptView {
            id: p.id().to_string(),
            name: p.name().as_str().to_owned(),
            title: p.title().to_owned(),
            description: p.description().to_owned(),
            body: p.body().to_owned(),
            variables: p.variables().as_slice().to_vec(),
            own: stored.own(),
            audience: stored.own().then(|| p.audience().values()),
            revision: stored.revision().to_string(),
            updated_at: stored.updated_at(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SummaryView {
    id: String,
    name: String,
    title: String,
    description: String,
    variable_count: u8,
    own: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    audience: Option<Vec<String>>,
    revision: String,
    updated_at: jiff::Timestamp,
}

impl SummaryView {
    fn of(summary: &PromptSummary) -> Self {
        SummaryView {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            title: summary.title.clone(),
            description: summary.description.clone(),
            variable_count: summary.variable_count,
            own: summary.own,
            audience: summary.audience.as_ref().map(|a| a.values()),
            revision: summary.revision.to_string(),
            updated_at: summary.updated_at,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListView {
    limit: usize,
    held: u32,
    items: Vec<SummaryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct RenderedView {
    text: String,
    revision: String,
}

// ---------------------------------------------------------------------------------- the bodies

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    name: String,
    title: String,
    #[serde(default, deserialize_with = "not_null")]
    description: Option<String>,
    body: String,
    #[serde(default, deserialize_with = "strict_list")]
    variables: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "not_null")]
    audience: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    revision: String,
    #[serde(default, deserialize_with = "not_null")]
    name: Option<String>,
    #[serde(default, deserialize_with = "not_null")]
    title: Option<String>,
    #[serde(default, deserialize_with = "not_null")]
    description: Option<String>,
    #[serde(default, deserialize_with = "not_null")]
    body: Option<String>,
    #[serde(default, deserialize_with = "strict_list")]
    variables: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "not_null")]
    audience: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderBody {
    #[serde(default, deserialize_with = "not_null")]
    revision: Option<String>,
    #[serde(default, deserialize_with = "strict_value")]
    answers: Option<Value>,
    #[serde(default, deserialize_with = "strict_value")]
    note: Option<Value>,
}

// --------------------------------------------------------------------------------- the handlers

/// `GET /api/prompts` (`listPrompts`): see the module's docs. 400 `bad_scope` for a `scope` that is not
/// `all`, `mine` or `shared`, 400 `bad_cursor` for a `cursor` that is not a prompt id.
pub(crate) async fn list<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    uri: Uri,
) -> ApiResult<Response> {
    let pairs = query_pairs(&uri);
    let scope = match first(&pairs, "scope") {
        None | Some("all") => PromptScope::All,
        Some("mine") => PromptScope::Mine,
        Some("shared") => PromptScope::Shared,
        Some(_) => {
            return Err(Problem::bad_request("`scope` is `all`, `mine` or `shared`")
                .with_code("bad_scope")
                .into());
        }
    };
    let after = match first(&pairs, "cursor") {
        None => None,
        Some(cursor) => Some(PromptId::parse(cursor).map_err(|_| {
            ApiError::from(
                Problem::bad_request("the cursor is the `nextCursor` of a page")
                    .with_code("bad_cursor"),
            )
        })?),
    };
    let page = state
        .app
        .list_prompts(&principal, scope, after.as_ref())
        .await?;
    let next_cursor = page
        .more
        .then(|| page.items.last().map(|last| last.id.to_string()))
        .flatten();
    Ok(Json(ListView {
        limit: orch_core::prompt::PAGE,
        held: page.held,
        items: page.items.iter().map(SummaryView::of).collect(),
        next_cursor,
    })
    .into_response())
}

/// `POST /api/prompts` (`createPrompt`): 201 with the prompt. 415, then 400 for a body that is not the
/// object of this route, then 403 `forbidden` for a share without `prompts.share`, then the rules of
/// the page (400 with `code`, `at` and for a reference `offset`), then 409 `name_taken` and
/// `too_many_prompts`.
pub(crate) async fn create<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let body: CreateBody = read(request).await?;
    let input = PromptInput {
        name: body.name,
        title: body.title,
        description: body.description.unwrap_or_default(),
        body: body.body,
        variables: body.variables.unwrap_or_default(),
        audience: body.audience.unwrap_or_default(),
    };
    let stored = state.app.create_prompt(&principal, input).await?;
    Ok((StatusCode::CREATED, Json(PromptView::of(&stored))).into_response())
}

/// `GET /api/prompts/{id}` (`getPrompt`): the caller's own prompt, or one shared with their roles; any
/// other is 404, the answer for an id that never existed. 400 for an id that is not one.
pub(crate) async fn get<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let stored = state.app.get_prompt(&principal, &parse_id(&id)?).await?;
    Ok(Json(PromptView::of(&stored)).into_response())
}

/// `PATCH /api/prompts/{id}` (`updatePrompt`): `revision` (required) and the members to replace, at
/// least one. 404 for a prompt that is not the caller's, 403 `forbidden` when the result is shared and
/// the caller lacks `prompts.share`, 409 `stale_revision`, then the rules of the page.
pub(crate) async fn edit<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let id = parse_id(&id)?;
    let body: EditBody = read(request).await?;
    let revision = parse_revision(&body.revision)?;
    let edit = EditPrompt {
        revision: Some(revision),
        name: body.name,
        title: body.title,
        description: body.description,
        body: body.body,
        variables: body.variables,
        audience: body.audience,
    };
    if edit.name.is_none()
        && edit.title.is_none()
        && edit.description.is_none()
        && edit.body.is_none()
        && edit.variables.is_none()
        && edit.audience.is_none()
    {
        return Err(bad_request("there is nothing to change"));
    }
    let stored = state.app.edit_prompt(&principal, &id, edit).await?;
    Ok(Json(PromptView::of(&stored)).into_response())
}

/// `DELETE /api/prompts/{id}?revision=` (`deletePrompt`): 204. Needs no `prompts.share`. 409
/// `stale_revision` when a revision is given and the prompt has changed.
pub(crate) async fn delete<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    uri: Uri,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let pairs = query_pairs(&uri);
    let expected = first(&pairs, "revision").map(parse_revision).transpose()?;
    state.app.delete_prompt(&principal, &id, expected).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/prompts/{id}/render` (`renderPrompt`): the text. It writes nothing and sends nothing; it
/// is allowed for the caller's own prompts and the ones shared with them. 422 `invalid_answer` with
/// `question` and `rule`, 422 `rendered_too_long`, 409 `stale_revision`.
pub(crate) async fn render<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let id = parse_id(&id)?;
    let body: RenderBody = read(request).await?;
    let revision = body.revision.as_deref().map(parse_revision).transpose()?;
    // `null` is the wrong type, for the answers as for any member
    if matches!(body.answers, Some(Value::Null)) || matches!(body.note, Some(Value::Null)) {
        return Err(bad_request("a member of this request is not null"));
    }
    let rendered = state
        .app
        .render_prompt(
            &principal,
            &id,
            RenderPrompt {
                revision,
                answers: body.answers,
                note: body.note.is_some(),
            },
        )
        .await?;
    Ok(Json(RenderedView {
        text: rendered.text,
        revision: rendered.revision.to_string(),
    })
    .into_response())
}
