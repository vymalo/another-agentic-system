//! `POST /webhooks/github`: GitHub's own webhook deliveries (`docs/api/webhooks.md`, ADR 0017).
//!
//! GitHub signs the raw body with `X-Hub-Signature-256` and nothing else, so the guard is
//! simpler than the generic one, and runs in this order (401, 408 and 413 are answered by it,
//! before anything is written):
//!
//! 1. the signature header is present (401);
//! 2. the body is within 5 MiB (413) and arrives within the read timeout (408);
//! 3. the signature is `HMAC-SHA-256(body)` under either secret (401).
//!
//! The handler then reads `X-GitHub-Event`. That header is **not signed**: it only selects which
//! parser reads the signed body, and the body has to be what that event's parser demands.
//!
//! | event | answer |
//! |---|---|
//! | none | 400 |
//! | `ping` | 204, nothing stored |
//! | `check_run`, `workflow_run` with `action` = `completed` | a [`CiReport`] is stored: 202, also for a repeated delivery |
//! | the same events with another action, and every other event (`check_suite` included) | 202, acknowledged and not stored, so GitHub neither retries nor flags them |
//!
//! Three more cases are acknowledged (202) and not stored, because the report must not count:
//!
//! * a `workflow_run` with no name (a required check is named, and "workflow" would name every
//!   unnamed one alike);
//! * a report whose **head repository is not the repository** (a run of a fork's code): its
//!   result says nothing about the commit the agent pushed to the repository, and a fork can
//!   choose its own workflow names;
//! * an event whose signed completion time is **older than the maximum age**
//!   (`WEBHOOK_GITHUB_MAX_AGE_SECS`, one day by default): a captured delivery is not replayable
//!   for ever.
//!
//! **The idempotency key comes from the signed body, not from `X-GitHub-Delivery`** (a header
//! that is not signed, so a captured delivery could be replayed under a new id): `check_run:<id>:
//! <completed_at>` and `workflow_run:<id>:<run_attempt>`. A redelivery, or a replay under any id,
//! is the same report.
//!
//! An accepted event that lacks the members it is read from is a 400. Whatever the payload says
//! is data: the conclusion is mapped onto the closed set (one GitHub adds that this build does
//! not know, and a missing one, is `failure`: an unknown outcome never passes), and the report is
//! matched to a job only by the commit, through the watch.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use jiff::Timestamp;
use orch_api::{ApiError, Problem, SurfaceRoutes};
use orch_app::App;
use orch_core::{CiConclusion, CiProvider, CiReport};
use orch_ports::{Clock, InboxPayload, Ports};
use serde_json::Value;

use crate::signature::verify_github;
use crate::wire::{RefusalLog, Verified, read_limited, text};
use crate::{Secrets, wire};

/// The route.
pub const PATH: &str = "/webhooks/github";
/// The body limit: 5 MiB. (GitHub caps a payload at 25 MB; this is our choice.)
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
/// The header with `sha256=` and the hex signature.
pub const SIGNATURE_HEADER: &str = "x-hub-signature-256";
/// The header that names the event. Not signed: it only selects the parser.
pub const EVENT_HEADER: &str = "x-github-event";
/// The header with the delivery id, unique per delivery. Not signed, so only logged.
pub const DELIVERY_HEADER: &str = "x-github-delivery";
/// The `source` of the inbox rows this route stores.
pub const SOURCE: &str = "github";
/// The most bytes of a report's summary that are kept.
pub const MAX_SUMMARY_BYTES: usize = crate::generic::MAX_SUMMARY_BYTES;
/// The longest check or workflow name kept; a longer one is cut (it will not match a required
/// name, which is the safe outcome).
pub const MAX_NAME_BYTES: usize = crate::generic::MAX_NAME_BYTES;
/// The default of `WEBHOOK_GITHUB_MAX_AGE_SECS`: one day.
pub const DEFAULT_MAX_AGE_SECS: u64 = 86_400;

/// What the GitHub route needs.
#[derive(Debug, Clone)]
pub struct GithubConfig {
    /// The webhook's secrets (`WEBHOOK_GITHUB_SECRETS`).
    pub secrets: Secrets,
    /// How old the signed completion time of an event may be (`WEBHOOK_GITHUB_MAX_AGE_SECS`).
    pub max_age: Duration,
    /// How long a delivery may take to arrive and be answered before it is cut off with 408.
    pub read_timeout: Duration,
}

impl GithubConfig {
    /// `secrets`, the default maximum age of a day and the default read timeout of ten seconds.
    pub fn new(secrets: Secrets) -> Self {
        GithubConfig {
            secrets,
            max_age: Duration::from_secs(DEFAULT_MAX_AGE_SECS),
            read_timeout: wire::DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

struct Shared<P: Ports> {
    app: Arc<App<P>>,
    secrets: Secrets,
    max_age_secs: i64,
    read_timeout: Duration,
    refusals: RefusalLog,
}

/// The route `POST /webhooks/github`, as a machine route: no identity layer, its own guard.
pub fn routes<P: Ports>(app: Arc<App<P>>, cfg: GithubConfig) -> SurfaceRoutes {
    let state = Arc::new(Shared {
        app,
        secrets: cfg.secrets,
        max_age_secs: i64::try_from(cfg.max_age.as_secs()).unwrap_or(i64::MAX),
        read_timeout: cfg.read_timeout,
        refusals: RefusalLog::new(PATH),
    });
    let router = Router::new()
        .route(PATH, post(accept::<P>))
        .with_state(Arc::clone(&state));
    SurfaceRoutes::new().machine(router, from_fn_with_state(state, guard::<P>))
}

async fn guard<P: Ports>(
    State(state): State<Arc<Shared<P>>>,
    req: Request,
    next: Next,
) -> Response {
    // The timeout covers the read of the body and the handler, this route only: the machine
    // routes as a whole have none (an MCP wait is long).
    let served = async {
        match verify(&state, req).await {
            Ok(req) => next.run(req).await,
            Err(refusal) => refusal.into_response(),
        }
    };
    match tokio::time::timeout(state.read_timeout, served).await {
        Ok(response) => response,
        Err(_) => state
            .refusals
            .refuse(
                StatusCode::REQUEST_TIMEOUT,
                "the request did not arrive in time",
            )
            .into_response(),
    }
}

/// The guard's checks. On success the request carries a [`Verified`] and an empty body.
async fn verify<P: Ports>(state: &Shared<P>, req: Request) -> Result<Request, Problem> {
    let (mut parts, body) = req.into_parts();
    let signature = text(&parts.headers, SIGNATURE_HEADER).ok_or_else(|| {
        state.refusals.refuse(
            StatusCode::UNAUTHORIZED,
            "missing or unreadable X-Hub-Signature-256",
        )
    })?;
    let bytes = read_limited(&parts.headers, body, MAX_BODY_BYTES)
        .await
        .map_err(|p| state.refusals.refused(p))?;
    if !verify_github(&state.secrets, &bytes, signature) {
        return Err(state
            .refusals
            .refuse(StatusCode::UNAUTHORIZED, "the signature does not match"));
    }
    let verified = Verified {
        event: text(&parts.headers, EVENT_HEADER)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        delivery: wire::delivery_id(&parts.headers, DELIVERY_HEADER),
        key: String::new(),
        body: bytes,
    };
    parts.extensions.insert(verified);
    Ok(Request::from_parts(parts, axum::body::Body::empty()))
}

async fn accept<P: Ports>(
    State(state): State<Arc<Shared<P>>>,
    verified: Option<Extension<Verified>>,
) -> Result<StatusCode, ApiError> {
    // Reached without the guard: refuse rather than accept an unverified body.
    let Some(Extension(verified)) = verified else {
        tracing::error!(
            route = PATH,
            "the GitHub webhook handler ran without its guard"
        );
        return Err(Problem::new(StatusCode::UNAUTHORIZED, "not verified").into());
    };
    let event = verified.event.as_str();
    match event {
        "" => Err(Problem::bad_request("X-GitHub-Event is missing").into()),
        "ping" => Ok(StatusCode::NO_CONTENT),
        "check_run" | "workflow_run" => {
            let (report, key, at) = match read(event, &verified.body)? {
                Read::Ignored(why) => {
                    tracing::debug!(
                        route = PATH,
                        event,
                        why,
                        "webhook event acknowledged, not stored"
                    );
                    return Ok(StatusCode::ACCEPTED);
                }
                Read::Report { report, key, at } => (report, key, at),
            };
            let now = state.app.ports().clock().now();
            let age = now.as_second().saturating_sub(at.as_second());
            if age > state.max_age_secs {
                tracing::info!(
                    route = PATH,
                    event,
                    key,
                    age_secs = age,
                    "webhook event older than WEBHOOK_GITHUB_MAX_AGE_SECS, acknowledged and not stored"
                );
                return Ok(StatusCode::ACCEPTED);
            }
            let received = state
                .app
                .receive(SOURCE, &key, InboxPayload::CiReport(report))
                .await?;
            tracing::info!(
                route = PATH,
                event,
                ?received,
                key,
                delivery = %verified.delivery,
                "webhook report received"
            );
            Ok(StatusCode::ACCEPTED)
        }
        other => {
            tracing::debug!(
                route = PATH,
                event = other,
                "webhook event acknowledged, not stored"
            );
            Ok(StatusCode::ACCEPTED)
        }
    }
}

/// GitHub's check-run vocabulary, plus `startup_failure` (a workflow run that could not start).
/// Anything else, and a `null`, is `failure`.
fn conclusion(name: Option<&str>) -> CiConclusion {
    match name {
        Some("success") => CiConclusion::Success,
        Some("neutral") => CiConclusion::Neutral,
        Some("skipped") => CiConclusion::Skipped,
        Some("failure") => CiConclusion::Failure,
        Some("cancelled") => CiConclusion::Cancelled,
        Some("timed_out") => CiConclusion::TimedOut,
        Some("action_required") => CiConclusion::ActionRequired,
        Some("stale") => CiConclusion::Stale,
        Some("startup_failure") => CiConclusion::StartupFailure,
        Some(other) => {
            tracing::warn!(
                conclusion = other,
                "unknown GitHub conclusion, counted as a failure"
            );
            CiConclusion::Failure
        }
        None => CiConclusion::Failure,
    }
}

/// The text at `path` (a JSON pointer) when it is a string.
fn str_at<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}

fn required<'a>(v: &'a Value, path: &str, what: &str) -> Result<&'a str, Problem> {
    str_at(v, path)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Problem::bad_request(format!("the payload has no {what} ({path})")))
}

/// A whole number at `path`, as text (ids can exceed what a float holds exactly).
fn number_at(v: &Value, path: &str, what: &str) -> Result<String, Problem> {
    match v.pointer(path) {
        Some(Value::Number(n)) if n.is_u64() || n.is_i64() => Ok(n.to_string()),
        _ => Err(Problem::bad_request(format!(
            "the payload has no {what} ({path})"
        ))),
    }
}

fn time_at(v: &Value, path: &str, what: &str) -> Result<Timestamp, Problem> {
    required(v, path, what)?
        .parse()
        .map_err(|_| Problem::bad_request(format!("{what} is not a time ({path})")))
}

/// What a `completed` delivery comes to.
enum Read {
    /// Acknowledged, not stored, for this reason.
    Ignored(&'static str),
    /// The report, its idempotency key and the signed time it is dated by.
    Report {
        report: CiReport,
        key: String,
        at: Timestamp,
    },
}

/// Whether two repositories of a payload are one repository: by id when both have one, else by
/// full name (case-insensitively).
fn same_repo(a: &Value, b: &Value) -> Option<bool> {
    if let (Some(x), Some(y)) = (
        a.get("id").and_then(Value::as_u64),
        b.get("id").and_then(Value::as_u64),
    ) {
        return Some(x == y);
    }
    let name = |v: &Value| {
        v.get("full_name")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
    };
    Some(name(a)? == name(b)?)
}

/// The report of a `check_run` or `workflow_run` delivery: ignored when its action is not
/// `completed` or it must not count, a 400 when it lacks what it is read from.
///
/// | member of the report | `check_run` | `workflow_run` |
/// |---|---|---|
/// | `repository` | `repository.html_url` | same |
/// | `sha` | `check_run.head_sha` | `workflow_run.head_sha` |
/// | `branch` | `check_run.check_suite.head_branch` | `workflow_run.head_branch` |
/// | `name` | `check_run.name` | `workflow_run.name` (ignored when null or blank) |
/// | `conclusion` | `check_run.conclusion` | `workflow_run.conclusion` |
/// | `url` | `check_run.html_url` | `workflow_run.html_url` |
/// | `summary` | `check_run.output.summary` | none |
/// | idempotency key | `check_run:<check_run.id>:<check_run.completed_at>` | `workflow_run:<workflow_run.id>:<workflow_run.run_attempt>` |
/// | dated by | `check_run.completed_at` | `workflow_run.updated_at` |
/// | head repository | every `check_run.pull_requests[].head.repo` | `workflow_run.head_repository` (required) |
fn read(event: &str, body: &[u8]) -> Result<Read, Problem> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| Problem::bad_request(format!("the body is not JSON: {e}")))?;
    let Some(action) = str_at(&v, "/action") else {
        return Err(Problem::bad_request("the payload has no action"));
    };
    if action != "completed" {
        return Ok(Read::Ignored("the action is not completed"));
    }
    let repository = required(&v, "/repository/html_url", "repository address")?;
    let base = v.pointer("/repository").unwrap_or(&Value::Null);
    let (sha, branch, name, conclusion_text, url, summary, key, at) = if event == "check_run" {
        let name = required(&v, "/check_run/name", "check name")?;
        // A pull request from a fork runs its own code under names of its own choosing.
        let from_a_fork = v
            .pointer("/check_run/pull_requests")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|pr| pr.pointer("/head/repo"))
            .any(|head| same_repo(head, base) != Some(true));
        if from_a_fork {
            return Ok(Read::Ignored(
                "the check ran for a pull request from another repository",
            ));
        }
        let at = time_at(&v, "/check_run/completed_at", "completion time")?;
        (
            required(&v, "/check_run/head_sha", "commit")?,
            str_at(&v, "/check_run/check_suite/head_branch"),
            name,
            str_at(&v, "/check_run/conclusion"),
            str_at(&v, "/check_run/html_url"),
            str_at(&v, "/check_run/output/summary"),
            format!(
                "check_run:{}:{}",
                number_at(&v, "/check_run/id", "check run id")?,
                required(&v, "/check_run/completed_at", "completion time")?
            ),
            at,
        )
    } else {
        if !v.get("workflow_run").is_some_and(Value::is_object) {
            return Err(Problem::bad_request(
                "the payload has no workflow run (/workflow_run)",
            ));
        }
        let name = str_at(&v, "/workflow_run/name")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(name) = name else {
            return Ok(Read::Ignored("the workflow run has no name"));
        };
        // Fail closed: a run whose head repository is missing, or is another one, is not ours.
        let head = v.pointer("/workflow_run/head_repository");
        if head.is_none_or(|head| same_repo(head, base) != Some(true)) {
            return Ok(Read::Ignored(
                "the workflow run is not of this repository's own code",
            ));
        }
        let at = time_at(&v, "/workflow_run/updated_at", "update time")?;
        let attempt = v.pointer("/workflow_run/run_attempt").map_or_else(
            || Ok("1".to_owned()),
            |_| number_at(&v, "/workflow_run/run_attempt", "run attempt"),
        )?;
        (
            required(&v, "/workflow_run/head_sha", "commit")?,
            str_at(&v, "/workflow_run/head_branch"),
            name,
            str_at(&v, "/workflow_run/conclusion"),
            str_at(&v, "/workflow_run/html_url"),
            None,
            format!(
                "workflow_run:{}:{attempt}",
                number_at(&v, "/workflow_run/id", "workflow run id")?,
            ),
            at,
        )
    };
    Ok(Read::Report {
        report: CiReport {
            provider: CiProvider::Github,
            repository: repository.to_owned(),
            sha: sha.to_owned(),
            branch: branch
                .map(|b| b.trim().to_owned())
                .filter(|b| !b.is_empty()),
            name: wire::truncate(name.to_owned(), MAX_NAME_BYTES),
            conclusion: conclusion(conclusion_text),
            url: url.and_then(wire::http_url),
            summary: summary
                .map(|s| wire::truncate(s.to_owned(), MAX_SUMMARY_BYTES))
                .filter(|s| !s.trim().is_empty()),
        },
        key,
        at,
    })
}
