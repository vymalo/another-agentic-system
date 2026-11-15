//! `POST /api/search/threads` (`searchThreads`, ADR 0068): a person searches their own threads.
//!
//! The rules are `docs/api/thread-search-v1.md` and `orch_core::search`; the store is only the index
//! ([`orch_ports::ThreadSearch`]). This module reads the request, decides the terms (the only text of a
//! query that goes anywhere), asks the application, cuts the snippets and writes the page.
//!
//! **The words of a query are in no log line, no trace and no error text**: the route is a `POST` so they
//! are in no URL, a body that does not parse answers fixed words (never the parser's, which may quote the
//! value), and the one line this module logs says how many terms and results there were.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Extension, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use orch_app::AppError;
use orch_core::ThreadId;
use orch_core::search::{
    MAX_QUERY_LENGTH, MAX_TERMS, QueryError, SearchRole, SearchTerms, Snippet, snippet,
};
use orch_ports::{
    ArchivedFilter, Ports, Principal, SearchPosition, SearchRank, SearchRequest, StoreError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ApiState;
use crate::problem::{ApiError, Problem};
use crate::routes::thread_json;

type ApiResult<T> = Result<T, ApiError>;

/// The most threads a page holds.
const MAX_LIMIT: i64 = 50;
/// The default page.
const DEFAULT_LIMIT: u32 = 20;
/// The longest cursor accepted: longer cannot be one this server made.
const MAX_CURSOR_BYTES: usize = 512;
/// The largest body: a query is at most 200 characters.
const MAX_BODY_BYTES: usize = 16 * 1024;
/// What a client is told to wait when the store could not answer.
const RETRY_AFTER_UNAVAILABLE: u64 = 5;

/// `search` of `GET /api/config`: its presence is the capability, its numbers what is enforced.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchCapability {
    max_query_length: usize,
    max_terms: usize,
}

impl Default for SearchCapability {
    fn default() -> Self {
        SearchCapability {
            max_query_length: MAX_QUERY_LENGTH,
            max_terms: MAX_TERMS,
        }
    }
}

/// The body of the request: only these members, whatever else is a 400.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchBody {
    q: String,
    limit: Option<i64>,
    cursor: Option<String>,
    archived: Option<String>,
    branches: Option<String>,
}

/// A 400 with the code `bad_request` and fixed words.
fn bad_request(detail: &str) -> ApiError {
    Problem::bad_request(detail).with_code("bad_request").into()
}

fn bad_cursor() -> ApiError {
    Problem::bad_request("the cursor is not the `next` of a page of this request")
        .with_code("bad_cursor")
        .into()
}

fn query_problem(error: QueryError) -> ApiError {
    Problem::bad_request(error.to_string())
        .with_code(error.code())
        .into()
}

/// The request as a JSON body: `application/json` or 415, a body of at most [`MAX_BODY_BYTES`], and
/// fixed words for whatever does not parse.
async fn read_body(request: Request) -> Result<SearchBody, ApiError> {
    let json = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| {
            let essence = essence.trim().to_ascii_lowercase();
            essence == "application/json" || essence.ends_with("+json")
        });
    if !json {
        return Err(Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the body is application/json",
        )
        .into());
    }
    let bytes: Bytes = axum::body::to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|_| bad_request("the request is too large or could not be read"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| bad_request("the body is not a search request: `q` and optionally `limit`, `cursor`, `archived`, `branches`"))
}

/// What a cursor is: where the page before ended, and which request it was for.
fn fingerprint(terms: &SearchTerms, archived: ArchivedFilter, edits: bool) -> String {
    let mut hash = Sha256::new();
    for term in terms.as_slice() {
        hash.update(term.as_bytes());
        hash.update([0x1f]);
    }
    hash.update(match archived {
        ArchivedFilter::Exclude => b"exclude".as_slice(),
        ArchivedFilter::Only => b"only".as_slice(),
        ArchivedFilter::Include => b"include".as_slice(),
    });
    hash.update([u8::from(edits)]);
    let bytes = hash.finalize();
    bytes.iter().take(8).fold(String::new(), |mut out, b| {
        out.push_str(&format!("{b:02x}"));
        out
    })
}

fn encode_cursor(position: SearchPosition, fingerprint: &str) -> String {
    format!(
        "v1-{}-{}-{fingerprint}",
        position.rank.number(),
        position.thread
    )
}

fn decode_cursor(cursor: &str, fingerprint: &str) -> Option<SearchPosition> {
    if cursor.len() > MAX_CURSOR_BYTES {
        return None;
    }
    let rest = cursor.strip_prefix("v1-")?;
    let (rank, rest) = rest.split_once('-')?;
    // a UUID has hyphens itself: the fingerprint is what follows the last one
    let (thread, print) = rest.rsplit_once('-')?;
    (print == fingerprint).then_some(())?;
    Some(SearchPosition {
        rank: SearchRank::from_number(rank.parse().ok()?)?,
        thread: thread.parse::<ThreadId>().ok()?,
    })
}

#[derive(Serialize)]
struct Query<'a> {
    terms: &'a [String],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    field: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<SearchRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<jiff::Timestamp>,
    snippet: Snippet,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadHit {
    thread: serde_json::Value,
    rank: u8,
    hits: Vec<Hit>,
    message_hits: u32,
}

#[derive(Serialize)]
struct Page<'a> {
    query: Query<'a>,
    results: Vec<ThreadHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
}

/// 429 for a search the limiter holds back, with `Retry-After`.
fn rate_limited(retry_after_secs: u64) -> Response {
    let mut response = Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too many searches: try again shortly",
    )
    .with_code("rate_limited")
    .into_response();
    let headers = response.headers_mut();
    headers.insert(RETRY_AFTER, HeaderValue::from(retry_after_secs));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `POST /api/search/threads` (`searchThreads`): see the module's docs and `docs/api/thread-search-v1.md`.
///
/// **Limited per person** ([`crate::ApiConfig::search_limits`], a burst of 20 and then two a second, in this
/// process): 429 with `Retry-After` and `code: rate_limited`, before anything else is looked at. 400 with
/// `query_empty`, `query_too_long`, `bad_cursor` or `bad_request`; 415 for a body that is not JSON; 403
/// without `thread.read`; 503 `search_unavailable` when the store could not answer.
pub(crate) async fn search_threads<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    request: Request,
) -> ApiResult<Response> {
    if let Err(limited) = state.search.admit(&principal.user) {
        return Ok(rate_limited(limited.retry_after_secs));
    }
    let body = read_body(request).await?;
    let terms = SearchTerms::parse(&body.q).map_err(query_problem)?;
    let limit = body.limit.unwrap_or(i64::from(DEFAULT_LIMIT));
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(bad_request("`limit` must be between 1 and 50"));
    }
    let limit = u32::try_from(limit).unwrap_or(DEFAULT_LIMIT);
    let archived = match body.archived.as_deref() {
        None | Some("include") => ArchivedFilter::Include,
        Some("exclude") => ArchivedFilter::Exclude,
        Some("only") => ArchivedFilter::Only,
        Some(_) => {
            return Err(bad_request(
                "`archived` must be `exclude`, `only` or `include`",
            ));
        }
    };
    let include_edits = match body.branches.as_deref() {
        None => false,
        Some("include") => true,
        Some(_) => return Err(bad_request("`branches` must be `include`")),
    };
    let print = fingerprint(&terms, archived, include_edits);
    let after = match body.cursor.as_deref() {
        None => None,
        Some(cursor) => Some(decode_cursor(cursor, &print).ok_or_else(bad_cursor)?),
    };
    let asked = SearchRequest {
        terms: terms.clone(),
        archived,
        include_edits,
        after,
        limit,
    };
    let rows = match state.app.search_threads(&principal, &asked).await {
        Ok(rows) => rows,
        Err(AppError::Store(StoreError::Unavailable { source })) => {
            // The cause is logged, never sent; and nothing of the query is in it.
            tracing::warn!(error = %source, "a search could not be answered");
            let mut response =
                Problem::new(StatusCode::SERVICE_UNAVAILABLE, "search is unavailable now")
                    .with_code("search_unavailable")
                    .into_response();
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from(RETRY_AFTER_UNAVAILABLE));
            return Ok(response);
        }
        Err(other) => return Err(other.into()),
    };
    let next = rows.more.then(|| rows.rows.last()).flatten().map(|last| {
        encode_cursor(
            SearchPosition {
                rank: last.rank,
                thread: last.thread.id,
            },
            &print,
        )
    });
    let results: Vec<ThreadHit> = rows
        .rows
        .iter()
        .map(|row| {
            let mut hits = Vec::new();
            // the title and the description are the thread's own words; the snippet is cut by the rules
            // of the page, so a hit exists only where the words really match
            if let Some(found) = snippet_if_matching(&row.thread.title, &terms) {
                hits.push(Hit {
                    field: "title",
                    seq: None,
                    role: None,
                    at: None,
                    snippet: found,
                });
            }
            if let Some(found) = row
                .thread
                .description
                .as_deref()
                .and_then(|d| snippet_if_matching(d, &terms))
            {
                hits.push(Hit {
                    field: "description",
                    seq: None,
                    role: None,
                    at: None,
                    snippet: found,
                });
            }
            for message in &row.messages {
                if let Some(found) = snippet_if_matching(&message.text, &terms) {
                    hits.push(Hit {
                        field: "message",
                        seq: Some(message.seq),
                        role: Some(message.role),
                        at: Some(message.at),
                        snippet: found,
                    });
                }
            }
            ThreadHit {
                thread: thread_json(&state, &row.thread, false),
                rank: row.rank.number(),
                hits,
                message_hits: row.message_hits,
            }
        })
        .collect();
    // how many terms and how many threads: nothing of what was typed
    tracing::debug!(
        terms = terms.len(),
        results = results.len(),
        "threads searched"
    );
    let page = Page {
        query: Query {
            terms: terms.as_slice(),
        },
        results,
        next,
    };
    let mut response = Json(page).into_response();
    // a person's words: never kept by a cache
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// The snippet of `text` when it matches every term (a document matches only as a whole).
fn snippet_if_matching(text: &str, terms: &SearchTerms) -> Option<Snippet> {
    if terms.matches(text) {
        snippet(text, terms)
    } else {
        None
    }
}
