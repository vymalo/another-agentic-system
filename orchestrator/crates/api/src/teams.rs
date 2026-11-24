//! `/api/teams`: a person's team presets (ADR 0074 decision 13, `docs/api/teams-v1.md`).
//!
//! | Route | What | Answer |
//! |---|---|---|
//! | `GET /api/teams?cursor=` | the caller's own presets, as summaries, by id | 200 `{limit, held, items, nextCursor?}` |
//! | `POST /api/teams` | saves one | 201 with the preset, `ETag` |
//! | `GET /api/teams/{id}` | one of the caller's | 200, `ETag` |
//! | `PUT /api/teams/{id}` | replaces the whole preset; `If-Match` required | 200, `ETag` |
//! | `DELETE /api/teams/{id}` | deletes the caller's own; `If-Match` optional | 204 |
//!
//! The rules are `orch_core::TeamPreset` and the application is `orch_app::App::*_team_preset`; this
//! module reads the request and writes the answer. **Nothing of a preset's words is logged**: a body
//! that does not parse is answered in fixed words (never serde's, which may quote the value). Every
//! answer under `/api/teams` is `Cache-Control: no-store` ([`no_store_for_teams`], the outermost
//! layer of the router).

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_MATCH};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_core::{
    DEFAULT_MAX_HANDOFFS, DEFAULT_PUSH_TOKENS, MAX_APPROVAL_SECONDS, MAX_CHARTER_BYTES,
    MAX_HANDOFFS_CEILING, MAX_MEMBERS, MAX_PUSH_TOKENS, MAX_ROLE_CHARS, MAX_TEAM_PRESETS,
    MIN_APPROVAL_SECONDS, MIN_MAX_HANDOFFS, MIN_MEMBERS, MIN_PUSH_TOKENS, TeamPresetId,
    TeamPresetInput,
};
use orch_ports::{Ports, Principal, StoredTeamPreset, TeamPresetRevision, TeamPresetSummary};
use serde::Serialize;

use crate::ApiState;
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// The largest request body: a charter of 4 KiB, eight roles and the rest, with room.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// `teams` of `GET /api/config`: its presence is the capability, its numbers what is enforced.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TeamsCapability {
    limit: usize,
    min_members: usize,
    max_members: usize,
    max_role_chars: usize,
    max_charter_bytes: usize,
    default_max_handoffs: u32,
    min_max_handoffs: u32,
    max_max_handoffs: u32,
    default_push_tokens: u32,
    min_push_tokens: u32,
    max_push_tokens: u32,
    min_approval_seconds: u16,
    max_approval_seconds: u16,
}

impl Default for TeamsCapability {
    fn default() -> Self {
        TeamsCapability {
            limit: MAX_TEAM_PRESETS,
            min_members: MIN_MEMBERS,
            max_members: MAX_MEMBERS,
            max_role_chars: MAX_ROLE_CHARS,
            max_charter_bytes: MAX_CHARTER_BYTES,
            default_max_handoffs: DEFAULT_MAX_HANDOFFS,
            min_max_handoffs: MIN_MAX_HANDOFFS,
            max_max_handoffs: MAX_HANDOFFS_CEILING,
            default_push_tokens: DEFAULT_PUSH_TOKENS,
            min_push_tokens: MIN_PUSH_TOKENS,
            max_push_tokens: MAX_PUSH_TOKENS,
            min_approval_seconds: MIN_APPROVAL_SECONDS,
            max_approval_seconds: MAX_APPROVAL_SECONDS,
        }
    }
}

fn bad_request(detail: &str) -> ApiError {
    Problem::bad_request(detail).with_code("bad_request").into()
}

fn parse_id(text: &str) -> ApiResult<TeamPresetId> {
    TeamPresetId::parse(text)
        .map_err(|_| bad_request("the team id is `t-` and 12 lowercase letters or digits"))
}

/// Every answer under `/api/teams`, problems included, is never kept by a cache.
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

/// `^[1-9][0-9]{0,18}$`, bare or as a quoted entity tag.
fn parse_revision(text: &str) -> ApiResult<TeamPresetRevision> {
    let text = text.trim().trim_matches('"');
    let digits = (1..=19).contains(&text.len())
        && text.bytes().all(|b| b.is_ascii_digit())
        && !text.starts_with('0');
    digits
        .then(|| text.parse::<u64>().ok())
        .flatten()
        .map(TeamPresetRevision::new)
        .ok_or_else(|| bad_request("`If-Match` is the revision the last read gave"))
}

fn if_match(headers: &HeaderMap) -> ApiResult<Option<TeamPresetRevision>> {
    headers
        .get(IF_MATCH)
        .map(|v| {
            v.to_str()
                .map_err(|_| bad_request("`If-Match` is the revision the last read gave"))
                .and_then(parse_revision)
        })
        .transpose()
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

/// The body, parsed with **fixed words** for whatever does not parse.
async fn read(request: Request) -> ApiResult<TeamPresetInput> {
    let bytes: Bytes = axum::body::to_bytes(request.into_body(), MAX_REQUEST_BYTES)
        .await
        .map_err(|_| bad_request("the request is too large or could not be read"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad_request("the body is not the JSON object of this route"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamView {
    id: String,
    #[serde(flatten)]
    preset: TeamPresetInput,
    revision: String,
    updated_at: jiff::Timestamp,
}

impl TeamView {
    fn of(stored: &StoredTeamPreset) -> Self {
        TeamView {
            id: stored.preset().id().to_string(),
            preset: stored.preset().to_input(),
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
    fn of(s: &TeamPresetSummary) -> Self {
        SummaryView {
            id: s.id.to_string(),
            name: s.name.clone(),
            title: s.title.clone(),
            lead: s.lead.as_str().to_owned(),
            member_count: s.member_count,
            revision: s.revision.to_string(),
            updated_at: s.updated_at,
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

fn with_etag(status: StatusCode, stored: &StoredTeamPreset) -> Response {
    let mut response = (status, Json(TeamView::of(stored))).into_response();
    if let Ok(tag) = HeaderValue::from_str(&format!("\"{}\"", stored.revision())) {
        response.headers_mut().insert(ETAG, tag);
    }
    response
}

fn query_cursor(uri: &Uri) -> ApiResult<Option<TeamPresetId>> {
    let cursor = uri.query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "cursor")
            .map(|(_, v)| v.into_owned())
    });
    cursor
        .map(|c| {
            TeamPresetId::parse(&c).map_err(|_| {
                ApiError::from(
                    Problem::bad_request("the cursor is the `nextCursor` of a page")
                        .with_code("bad_cursor"),
                )
            })
        })
        .transpose()
}

/// `GET /api/teams` (`listTeams`). 400 `bad_cursor` for a `cursor` that is not a team id.
pub(crate) async fn list<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    uri: Uri,
) -> ApiResult<Response> {
    let after = query_cursor(&uri)?;
    let page = state
        .app
        .list_team_presets(&principal, after.as_ref())
        .await?;
    let next_cursor = page
        .more
        .then(|| page.items.last().map(|last| last.id.to_string()))
        .flatten();
    Ok(Json(ListView {
        limit: orch_core::TEAM_PRESET_PAGE,
        held: page.held,
        items: page.items.iter().map(SummaryView::of).collect(),
        next_cursor,
    })
    .into_response())
}

/// `POST /api/teams` (`createTeam`): 201 with the preset. 415, then 400 for a body that is not the
/// object of this route, then the rules (400 with `code` and `at`), 403 for a member's agent the roles
/// do not name, 422 `unknown_agent` for one the audience hides, then 409 `name_taken` and
/// `too_many_teams`.
pub(crate) async fn create<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let input = read(request).await?;
    let stored = state.app.create_team_preset(&principal, input).await?;
    Ok(with_etag(StatusCode::CREATED, &stored))
}

/// `GET /api/teams/{id}` (`getTeam`): the caller's own; any other is 404.
pub(crate) async fn get<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let stored = state
        .app
        .get_team_preset(&principal, &parse_id(&id)?)
        .await?;
    Ok(with_etag(StatusCode::OK, &stored))
}

/// `PUT /api/teams/{id}` (`replaceTeam`): the whole preset, with `If-Match` (400 without). 404 for a
/// preset that is not the caller's, 409 `stale_revision`, then the rules of `createTeam`.
pub(crate) async fn replace<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    request: Request,
) -> ApiResult<Response> {
    require_json(&request)?;
    let id = parse_id(&id)?;
    let expected = if_match(request.headers())?
        .ok_or_else(|| bad_request("`If-Match` is the revision the last read gave"))?;
    let input = read(request).await?;
    let stored = state
        .app
        .replace_team_preset(&principal, &id, input, expected)
        .await?;
    Ok(with_etag(StatusCode::OK, &stored))
}

/// `DELETE /api/teams/{id}` (`deleteTeam`): 204. With `If-Match`, 409 `stale_revision` when the
/// preset has changed.
pub(crate) async fn delete<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let expected = if_match(&headers)?;
    state
        .app
        .delete_team_preset(&principal, &id, expected)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
