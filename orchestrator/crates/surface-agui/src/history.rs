//! `GET /agui/threads/{threadId}/history` and its two shared variants: a finite page of the
//! thread's AG-UI frames, from the newest settled chain back, or the settled chains after a point
//! (`docs/api/history.md`, ADR 0059).
//!
//! The page is [`orch_agui_projection::History`] driven by [`App::history_log`]: the log is read
//! from the first event in pages and folded until the page cannot change, so the cost is a
//! connect's read and fold (less the events after the page for a read that goes back from
//! `before`). The answer is one JSON document, never cached, and its `end` is a settled point: a
//! connect with `Last-Event-ID: end` says the rest.
//!
//! Every refusal is an RFC 9457 problem, as for the connect routes: 400 for a query that is not
//! understood, 404 for a thread or a link that does not work for the caller (one answer, whatever
//! the roles: ADR 0039), 403 for roles that hold no `thread.read`, 406 for an `Accept` that
//! excludes `application/json`, 429 for a public reader past the limits, 503 when the store cannot
//! answer.

use std::time::{Duration, Instant};

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use futures::stream::BoxStream;
use orch_agui_projection::{
    Anchor, Carry, Flow, Frame, History, HistoryLimits, PROJECTION_VERSION, Page, Window,
};
use orch_agui_proto as agui;
use orch_api::sse::shared_json_headers;
use orch_api::{ApiError, ApiQuery, Problem, PublicAccess, parse_thread_id, too_many_streams};
use orch_app::AppError;
use orch_core::{Event, ThreadRecord};
use orch_ports::{Ports, Principal};
use serde::{Deserialize, Serialize};

use crate::State as SurfaceState;
use crate::run::meta_of;

/// The longest one read may take. The public route has no request timeout of the API's (it sits
/// with the streams) and holds a stream permit for as long as it folds, so a store that does not
/// answer must not hold the permit for ever.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// What `ui.history.pageTurns` is when the configuration does not say.
const DEFAULT_PAGE_TURNS: usize = 20;

/// The query of the history routes, as text: a number that is not one is a 400 with its own words.
#[derive(Debug, Deserialize)]
pub(crate) struct HistoryQuery {
    before: Option<String>,
    limit: Option<String>,
    since: Option<String>,
    after: Option<String>,
}

/// An integer of 1 or more, or the 400 that says which parameter was not.
fn count(name: &str, raw: Option<&str>) -> Result<Option<i64>, Problem> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 1)
        .map(Some)
        .ok_or_else(|| Problem::bad_request(format!("{name} must be an integer of 1 or more")))
}

/// The window a query asks for, within what the deployment allows.
fn window_of(
    query: &HistoryQuery,
    limits: HistoryLimits,
    default_turns: usize,
) -> Result<Window, Problem> {
    let before = count("before", query.before.as_deref())?;
    let limit = count("limit", query.limit.as_deref())?;
    let since = count("since", query.since.as_deref())?;
    let after = count("after", query.after.as_deref())?;
    if [limit, since, after].iter().flatten().count() > 1 {
        return Err(Problem::bad_request(
            "limit, since and after exclude each other: ask for one",
        ));
    }
    if before.is_some() && after.is_some() {
        return Err(Problem::bad_request(
            "before goes with limit and since, not with after",
        ));
    }
    if let Some(after) = after {
        return Ok(Window::After { after });
    }
    if let Some(since) = since {
        return Ok(Window::Since { before, since });
    }
    let turns = match limit {
        Some(limit) => usize::try_from(limit)
            .ok()
            .filter(|n| *n <= limits.max_turns)
            .ok_or_else(|| {
                Problem::bad_request(format!(
                    "limit must be between 1 and {} (the most turns a page holds)",
                    limits.max_turns
                ))
            })?,
        None => default_turns.min(limits.max_turns).max(1),
    };
    Ok(Window::Turns { before, turns })
}

/// `application/json` is what the routes answer; no `Accept`, `*/*` and `application/*` accept.
fn check_accept_json(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(accept) = headers.get(header::ACCEPT) else {
        return Ok(());
    };
    let accepted = accept.to_str().is_ok_and(|accept| {
        accept.split(',').any(|range| {
            let media = range.split(';').next().unwrap_or("").trim();
            matches!(media, "application/json" | "application/*" | "*/*")
        })
    });
    if accepted {
        Ok(())
    } else {
        Err(Problem::new(
            StatusCode::NOT_ACCEPTABLE,
            "this endpoint answers application/json",
        )
        .into())
    }
}

/// The body of a page: the contract's `HistoryPage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PageBody<'a> {
    thread_id: String,
    start: i64,
    end: i64,
    head: i64,
    earlier: bool,
    projection: u32,
    frames: Vec<FrameBody<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<AnchorBody<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    carry: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct FrameBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<i64>,
    event: &'a agui::Event,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnchorBody<'a> {
    seq: i64,
    run_id: &'a str,
}

fn body_of<'a>(thread: &ThreadRecord, page: &'a Page) -> PageBody<'a> {
    PageBody {
        thread_id: thread.id.to_string(),
        start: page.start,
        end: page.end,
        head: page.head,
        earlier: page.earlier,
        projection: PROJECTION_VERSION,
        frames: page
            .frames
            .iter()
            .map(|Frame { event, resume_id }| FrameBody {
                id: *resume_id,
                event,
            })
            .collect(),
        anchor: page
            .anchor
            .as_ref()
            .map(|Anchor { seq, run_id }| AnchorBody { seq: *seq, run_id }),
        carry: page.carry.as_ref().map(Carry::to_value),
    }
}

/// Folds `log` into the page `window` asks for, and counts what it cost.
async fn fold<P: Ports>(
    state: &SurfaceState<P>,
    thread: &ThreadRecord,
    window: Window,
    log: BoxStream<'static, Result<Event, AppError>>,
) -> Result<Page, ApiError> {
    let settings = state.app.history_settings();
    let limits = HistoryLimits {
        max_turns: settings.max_turns,
        max_page_bytes: settings.max_page_bytes,
    };
    let started = Instant::now();
    let mut history = History::new(meta_of(thread), window, limits, thread.last_seq);
    let mut folded = 0_u64;
    let mut log = log;
    while let Some(event) = log.next().await {
        let event = event?;
        folded += 1;
        if history.feed(&event) == Flow::Done {
            break;
        }
    }
    let page = history.finish();
    state.app.history_answered(folded, started.elapsed());
    Ok(page)
}

/// [`fold`] under [`READ_TIMEOUT`], as the JSON answer.
async fn answer<P: Ports>(
    state: &SurfaceState<P>,
    thread: &ThreadRecord,
    window: Window,
    log: BoxStream<'static, Result<Event, AppError>>,
    shared: bool,
) -> Result<Response, ApiError> {
    let page = match tokio::time::timeout(READ_TIMEOUT, fold(state, thread, window, log)).await {
        Ok(page) => page?,
        Err(_) => {
            let mut response = Problem::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "the thread's history could not be read in time",
            )
            .into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from(5_u64));
            return Ok(response);
        }
    };
    let body = body_of(thread, &page);
    let mut response = axum::Json(&body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    if shared {
        for (name, value) in shared_json_headers() {
            headers.insert(name, value);
        }
    }
    Ok(response)
}

fn default_turns<P: Ports>(state: &SurfaceState<P>) -> usize {
    state
        .app
        .public_config()
        .ui
        .history
        .map_or(DEFAULT_PAGE_TURNS, |h| h.page_turns as usize)
}

fn limits_of<P: Ports>(state: &SurfaceState<P>) -> HistoryLimits {
    let settings = state.app.history_settings();
    HistoryLimits {
        max_turns: settings.max_turns,
        max_page_bytes: settings.max_page_bytes,
    }
}

/// `GET /agui/threads/{threadId}/history`: the owner's.
pub(crate) async fn history<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_accept_json(&headers)?;
    let window = window_of(&query, limits_of(&state), default_turns(&state))?;
    // 404 is decided here, as for the connect stream.
    let thread = parse_thread_id(&id)?;
    let (record, log) = state.app.history_log(&principal, thread).await?;
    answer(&state, &record, window, log, false).await
}

/// `GET /agui/shared/{token}/history`: for a signed-in reader of an `internal` or `public` link,
/// over the reader's projection.
pub(crate) async fn history_shared<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(principal): Extension<Principal>,
    Path(token): Path<String>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_accept_json(&headers)?;
    let window = window_of(&query, limits_of(&state), default_turns(&state))?;
    let read = state.app.open_shared(&principal, &token).await?;
    let log = state.app.shared_history_log(&read);
    answer(&state, read.thread(), window, log, true).await
}

/// `GET /agui/public/shared/{token}/history`: for anybody, outside the identity layer, for a
/// `public` link. It holds one of the link's stream permits for as long as it folds, as an open
/// connect does, so the permit is what bounds how many folds run at once (429 when the link, or
/// all links together, have theirs).
pub(crate) async fn history_public<P: Ports>(
    State(state): State<SurfaceState<P>>,
    Extension(access): Extension<PublicAccess>,
    Path(token): Path<String>,
    ApiQuery(query): ApiQuery<HistoryQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_accept_json(&headers)?;
    let window = window_of(&query, limits_of(&state), default_turns(&state))?;
    let read = state.app.open_public(&token).await?;
    // After the link is known to work: a token that does not takes no room from a link that does.
    let permit = match access.stream_permit() {
        Ok(permit) => permit,
        Err(limited) => return Ok(too_many_streams(limited)),
    };
    let log = state.app.shared_history_log(&read);
    let response = answer(&state, read.thread(), window, log, true).await;
    // released on every exit, the refusals and the timeout included
    drop(permit);
    response
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> HistoryQuery {
        let get = |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        };
        HistoryQuery {
            before: get("before"),
            limit: get("limit"),
            since: get("since"),
            after: get("after"),
        }
    }

    fn window(pairs: &[(&str, &str)]) -> Result<Window, String> {
        window_of(&query(pairs), HistoryLimits::default(), 20)
            .map_err(|p| p.detail.unwrap_or_default())
    }

    #[test]
    fn a_query_is_a_window() {
        assert_eq!(
            window(&[]),
            Ok(Window::Turns {
                before: None,
                turns: 20
            })
        );
        assert_eq!(
            window(&[("limit", "5"), ("before", "9")]),
            Ok(Window::Turns {
                before: Some(9),
                turns: 5
            })
        );
        assert_eq!(
            window(&[("since", "7")]),
            Ok(Window::Since {
                before: None,
                since: 7
            })
        );
        assert_eq!(window(&[("after", "3")]), Ok(Window::After { after: 3 }));
    }

    #[test]
    fn what_is_not_a_count_is_a_400_with_its_name() {
        for (name, value) in [
            ("before", "0"),
            ("before", "-1"),
            ("before", "x"),
            ("since", "0"),
            ("after", "0"),
            ("after", "1.5"),
            ("limit", "0"),
            ("limit", "101"),
            ("limit", "many"),
        ] {
            let error = window(&[(name, value)]).unwrap_err();
            assert!(error.contains(name), "{name}={value}: {error}");
        }
    }

    #[test]
    fn parameters_that_exclude_each_other_are_a_400() {
        for pairs in [
            &[("limit", "3"), ("since", "4")][..],
            &[("limit", "3"), ("after", "4")],
            &[("since", "3"), ("after", "4")],
            &[("before", "3"), ("after", "4")],
        ] {
            assert!(window(pairs).is_err(), "{pairs:?}");
        }
    }

    #[test]
    fn the_default_is_bounded_by_the_deployment() {
        let small = HistoryLimits {
            max_turns: 7,
            max_page_bytes: 1,
        };
        assert_eq!(
            window_of(&query(&[]), small, 20),
            Ok(Window::Turns {
                before: None,
                turns: 7
            })
        );
        assert!(window_of(&query(&[("limit", "8")]), small, 20).is_err());
    }

    #[test]
    fn json_is_what_is_answered() {
        let with = |value: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ACCEPT, value.parse().unwrap());
            h
        };
        assert!(check_accept_json(&HeaderMap::new()).is_ok());
        for ok in [
            "application/json",
            "*/*",
            "application/*",
            "text/html, application/json;q=0.5",
        ] {
            assert!(check_accept_json(&with(ok)).is_ok(), "{ok}");
        }
        for bad in ["text/event-stream", "text/html", ""] {
            assert!(check_accept_json(&with(bad)).is_err(), "{bad:?}");
        }
    }
}
