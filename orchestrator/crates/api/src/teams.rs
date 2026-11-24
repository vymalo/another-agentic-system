//! `/api/teams`: a person's team presets (ADR 0074 decision 13, `docs/api/teams-v1.md`).
//!
//! | Route | What | Answer |
//! |---|---|---|
//! | `GET /api/teams?cursor=` | the caller's presets, as summaries, by id | 200 `{limit, held, items, nextCursor?}` |
//! | `POST /api/teams` | saves one | 201 with the preset |
//! | `GET /api/teams/{id}` | one of the caller's | 200 |
//! | `PUT /api/teams/{id}` | `{revision, ...the whole preset}`: replaces it | 200 |
//! | `DELETE /api/teams/{id}?revision=` | deletes one of the caller's | 204 |
//!
//! The rules are `orch_core::TeamPreset` and the application is `orch_app::App::*_team_preset`; this
//! module reads the request and writes the answer. **Nothing of a preset's words is logged**: a body
//! that does not parse is answered in fixed words, and no `tracing` field here holds a name, a role
//! or a charter. Every answer under `/api/teams`, the problems included, is `Cache-Control:
//! no-store` ([`no_store_for_teams`]).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_core::{
    DEFAULT_MAX_HANDOFFS, DEFAULT_PUSH_TOKENS, MAX_CHARTER_BYTES, MAX_HANDOFFS_CEILING,
    MAX_MEMBERS, MAX_PUSH_TOKENS, MAX_TEAM_PRESETS, MIN_MAX_HANDOFFS, MIN_MEMBERS, MIN_PUSH_TOKENS,
    TEAM_PRESET_PAGE, TeamPresetId, TeamPresetInput,
};
use orch_ports::{Ports, Principal, StoredTeamPreset, TeamPresetRevision, TeamPresetSummary};
use serde::Serialize;
use serde_json::Value;

use crate::ApiState;
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// The largest request body: a preset is at most eight members and a charter of 4 KiB.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// `teams` of `GET /api/config`: its presence is the capability, its numbers what is enforced.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamsCapability {
    limit: usize,
    min_members: usize,
    max_members: usize,
    max_charter_bytes: usize,
    min_max_handoffs: u32,
    max_max_handoffs: u32,
    default_max_handoffs: u32,
    min_push_tokens: u32,
    max_push_tokens: u32,
    default_push_tokens: u32,
}

impl Default for TeamsCapability {
    fn default() -> Self {
        TeamsCapability {
            limit: MAX_TEAM_PRESETS,
            min_members: MIN_MEMBERS,
            max_members: MAX_MEMBERS,
            max_charter_bytes: MAX_CHARTER_BYTES,
            min_max_handoffs: MIN_MAX_HANDOFFS,
            max_max_handoffs: MAX_HANDOFFS_CEILING,
            default_max_handoffs: DEFAULT_MAX_HANDOFFS,
            min_push_tokens: MIN_PUSH_TOKENS,
            max_push_tokens: MAX_PUSH_TOKENS,
            default_push_tokens: DEFAULT_PUSH_TOKENS,
        }
    }
}

fn bad_request(detail: &str) -> ApiError {
    Problem::bad_request(detail).with_code("bad_request").into()
}

/// Every answer under `/api/teams`, problems included, is never kept by a cache. It is the outermost
/// layer of the router, so the identity layer's 401 and the fallbacks' 404 and 405 carry it too.
pub(crate) async fn no_store_for_teams(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let ours = path == "/api/teams" || path.starts_with("/api/teams/");
    let mut response = next.run(request).await;
    if ours {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn parse_id(text: &str) -> ApiResult<TeamPresetId> {
    TeamPresetId::parse(text)
        .map_err(|_| bad_request("the team id is `t-` and 12 lowercase letters or digits"))
}

/// `^[1-9][0-9]{0,18}$`.
fn parse_revision(text: &str) -> ApiResult<TeamPresetRevision> {
    let digits = (1..=19).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_digit())
        && !text.starts_with('0');
    digits
        .then(|| text.parse::<u64>().ok())
        .flatten()
        .map(TeamPresetRevision::new)
        .ok_or_else(|| bad_request("a revision is the digits the last read gave"))
}

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

/// The body as a JSON value, at most [`MAX_REQUEST_BYTES`], with fixed words for whatever does not
/// parse: serde's messages may quote the value.
async fn read_value(request: Request) -> ApiResult<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), MAX_REQUEST_BYTES)
        .await
        .map_err(|_| bad_request("the request is too large or could not be read"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad_request("the body is not the JSON object of this route"))
}

fn to_input(value: Value) -> ApiResult<TeamPresetInput> {
    serde_json::from_value(value)
        .map_err(|_| bad_request("the body is not the team preset of this route"))
}

fn query_value(uri: &Uri, name: &str) -> Option<String> {
    uri.query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    })
}

// ------------------------------------------------------------------------------------ the views

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresetView {
    id: String,
    #[serde(flatten)]
    input: TeamPresetInput,
    revision: String,
    updated_at: jiff::Timestamp,
}

impl PresetView {
    fn of(stored: &StoredTeamPreset) -> Self {
        PresetView {
            id: stored.preset().id().to_string(),
            input: stored.preset().to_input(),
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
    lead: String,
    member_count: u8,
    revision: String,
    updated_at: jiff::Timestamp,
}

impl SummaryView {
    fn of(summary: &TeamPresetSummary) -> Self {
        SummaryView {
            id: summary.id.to_string(),
            name: summary.name.clone(),
            title: summary.title.clone(),
            lead: summary.lead.as_str().to_owned(),
            member_count: summary.member_count,
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

// --------------------------------------------------------------------------------- the handlers

/// `GET /api/teams` (`listTeams`): 400 `bad_cursor` for a `cursor` that is not a preset id.
pub(crate) async fn list<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    uri: Uri,
) -> ApiResult<Response> {
    let after = match query_value(&uri, "cursor") {
        None => None,
        Some(cursor) => Some(TeamPresetId::parse(&cursor).map_err(|_| {
            ApiError::from(
                Problem::bad_request("the cursor is the `nextCursor` of a page")
                    .with_code("bad_cursor"),
            )
        })?),
    };
    let page = state
        .app
        .list_team_presets(&principal, after.as_ref())
        .await?;
    let next_cursor = page
        .more
        .then(|| page.items.last().map(|last| last.id.to_string()))
        .flatten();
    Ok(Json(ListView {
        limit: TEAM_PRESET_PAGE,
        held: page.held,
        items: page.items.iter().map(SummaryView::of).collect(),
        next_cursor,
    })
    .into_response())
}

/// `POST /api/teams` (`createTeam`): 201 with the preset. 415, 400 for a body that is not the
/// object of this route, 400 with a `code` and `at` for a rule of the page, 422 `member_agent_refused`
/// for an agent the caller may not use, 409 `name_taken` and `too_many_teams`.
pub(crate) async fn create<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let input = to_input(read_value(request).await?)?;
    let stored = state.app.create_team_preset(&principal, input).await?;
    Ok((StatusCode::CREATED, Json(PresetView::of(&stored))).into_response())
}

/// `GET /api/teams/{id}` (`getTeam`): 404 for a preset that is not the caller's.
pub(crate) async fn get<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let stored = state
        .app
        .get_team_preset(&principal, &parse_id(&id)?)
        .await?;
    Ok(Json(PresetView::of(&stored)).into_response())
}

/// `PUT /api/teams/{id}` (`replaceTeam`): `revision` and the whole preset. 404 for a preset that is
/// not the caller's, 409 `stale_revision`, then the rules of `POST`.
pub(crate) async fn replace<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let id = parse_id(&id)?;
    let mut value = read_value(request).await?;
    let revision = match value.as_object_mut().and_then(|o| o.remove("revision")) {
        Some(Value::String(text)) => parse_revision(&text)?,
        _ => return Err(bad_request("`revision` is required, as a string")),
    };
    let input = to_input(value)?;
    let stored = state
        .app
        .replace_team_preset(&principal, &id, revision, input)
        .await?;
    Ok(Json(PresetView::of(&stored)).into_response())
}

/// `DELETE /api/teams/{id}?revision=` (`deleteTeam`): 204.
pub(crate) async fn delete<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    uri: Uri,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let expected = query_value(&uri, "revision")
        .map(|r| parse_revision(&r))
        .transpose()?;
    state
        .app
        .delete_team_preset(&principal, &id, expected)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
