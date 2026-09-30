//! `POST /webhooks/github`: GitHub's own webhook deliveries (`docs/api/webhooks.md`, ADR 0017).
//!
//! GitHub signs the raw body with `X-Hub-Signature-256` and nothing else, so the guard is
//! simpler than the generic one, and runs in this order (401 and 413 are answered by it, before
//! anything is written):
//!
//! 1. the signature header is present (401);
//! 2. the body is within 5 MiB (413);
//! 3. the signature is `HMAC-SHA-256(body)` under either secret (401).
//!
//! The handler then reads `X-GitHub-Event`:
//!
//! | event | answer |
//! |---|---|
//! | none | 400 |
//! | `ping` | 204, nothing stored |
//! | `check_suite`, `check_run`, `workflow_run` with `action` = `completed` | a [`CiReport`] is stored under `github:<X-GitHub-Delivery>`: 202, also for a repeated delivery |
//! | the same events with another action, and every other event | 202, acknowledged and not stored, so GitHub neither retries nor flags them |
//!
//! An accepted event that lacks the members it is read from, or has no delivery id, is a 400.
//! Whatever the payload says is data: the conclusion is mapped onto the closed set (one GitHub
//! adds that this build does not know, and a missing one, is `failure`: an unknown outcome never
//! passes), and the report is matched to a job only by the commit, through the watch.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use orch_api::{ApiError, Problem, SurfaceRoutes};
use orch_app::App;
use orch_core::{CiConclusion, CiProvider, CiReport};
use orch_ports::{InboxPayload, Ports};
use serde_json::Value;

use crate::signature::verify_github;
use crate::wire::{Verified, read_limited, text};
use crate::{Secrets, wire};

/// The route.
pub const PATH: &str = "/webhooks/github";
/// The body limit: 5 MiB. (GitHub caps a payload at 25 MB; this is our choice.)
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;
/// The header with `sha256=` and the hex signature.
pub const SIGNATURE_HEADER: &str = "x-hub-signature-256";
/// The header that names the event.
pub const EVENT_HEADER: &str = "x-github-event";
/// The header with the delivery id, unique per delivery.
pub const DELIVERY_HEADER: &str = "x-github-delivery";
/// The `source` of the inbox rows this route stores.
pub const SOURCE: &str = "github";
/// The most bytes of a report's summary that are kept.
pub const MAX_SUMMARY_BYTES: usize = crate::generic::MAX_SUMMARY_BYTES;
/// What a workflow run with no name is called.
const UNNAMED_WORKFLOW: &str = "workflow";

/// What the GitHub route needs.
#[derive(Debug, Clone)]
pub struct GithubConfig {
    /// The webhook's secrets (`WEBHOOK_GITHUB_SECRETS`).
    pub secrets: Secrets,
}

impl GithubConfig {
    /// `secrets` as the webhook's configuration.
    pub fn new(secrets: Secrets) -> Self {
        GithubConfig { secrets }
    }
}

struct Shared<P: Ports> {
    app: Arc<App<P>>,
    secrets: Secrets,
}

/// The route `POST /webhooks/github`, as a machine route: no identity layer, its own guard.
pub fn routes<P: Ports>(app: Arc<App<P>>, cfg: GithubConfig) -> SurfaceRoutes {
    let state = Arc::new(Shared {
        app,
        secrets: cfg.secrets,
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
    match verify(&state, req).await {
        Ok(req) => next.run(req).await,
        Err(refusal) => refusal.into_response(),
    }
}

fn refuse(status: StatusCode, why: &'static str) -> Problem {
    tracing::warn!(route = PATH, %status, why, "webhook delivery refused");
    Problem::new(status, why)
}

/// The guard's checks. On success the request carries a [`Verified`] and an empty body.
async fn verify<P: Ports>(state: &Shared<P>, req: Request) -> Result<Request, Problem> {
    let (mut parts, body) = req.into_parts();
    let signature = text(&parts.headers, SIGNATURE_HEADER).ok_or_else(|| {
        refuse(
            StatusCode::UNAUTHORIZED,
            "missing or unreadable X-Hub-Signature-256",
        )
    })?;
    let bytes = read_limited(&parts.headers, body, MAX_BODY_BYTES)
        .await
        .inspect_err(|p| {
            tracing::warn!(route = PATH, status = p.status, "webhook delivery refused")
        })?;
    if !verify_github(&state.secrets, &bytes, signature) {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "the signature does not match",
        ));
    }
    let header = |name| {
        text(&parts.headers, name)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let verified = Verified {
        event: header(EVENT_HEADER),
        delivery: header(DELIVERY_HEADER),
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
    match verified.event.as_str() {
        "" => Err(Problem::bad_request("X-GitHub-Event is missing").into()),
        "ping" => Ok(StatusCode::NO_CONTENT),
        event @ ("check_suite" | "check_run" | "workflow_run") => {
            let Some(report) = report(event, &verified.body)? else {
                tracing::debug!(
                    route = PATH,
                    event,
                    "webhook event acknowledged, not stored"
                );
                return Ok(StatusCode::ACCEPTED);
            };
            if verified.delivery.is_empty() {
                return Err(Problem::bad_request("X-GitHub-Delivery is missing").into());
            }
            let key = format!("github:{}", verified.delivery);
            let received = state
                .app
                .receive(SOURCE, &key, InboxPayload::CiReport(report))
                .await?;
            tracing::info!(route = PATH, event, ?received, delivery = %verified.delivery, "webhook report received");
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

/// The report of a `check_suite`, `check_run` or `workflow_run` delivery: `None` when its action
/// is not `completed`, a 400 when it lacks what it is read from.
///
/// | member of the report | `check_suite` | `check_run` | `workflow_run` |
/// |---|---|---|---|
/// | `repository` | `repository.html_url` | same | same |
/// | `sha` | `check_suite.head_sha` | `check_run.head_sha` | `workflow_run.head_sha` |
/// | `branch` | `check_suite.head_branch` | `check_run.check_suite.head_branch` | `workflow_run.head_branch` |
/// | `name` | `check_suite.app.slug` | `check_run.name` | `workflow_run.name` (`workflow` when null) |
/// | `conclusion` | `check_suite.conclusion` | `check_run.conclusion` | `workflow_run.conclusion` |
/// | `url` | none (the suite has no page of its own) | `check_run.html_url` | `workflow_run.html_url` |
/// | `summary` | none | `check_run.output.summary` | none |
fn report(event: &str, body: &[u8]) -> Result<Option<CiReport>, Problem> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|e| Problem::bad_request(format!("the body is not JSON: {e}")))?;
    let Some(action) = str_at(&v, "/action") else {
        return Err(Problem::bad_request("the payload has no action"));
    };
    if action != "completed" {
        return Ok(None);
    }
    let repository = required(&v, "/repository/html_url", "repository address")?;
    let (sha, branch, name, conclusion_text, url, summary) = match event {
        "check_suite" => (
            required(&v, "/check_suite/head_sha", "commit")?,
            str_at(&v, "/check_suite/head_branch"),
            required(&v, "/check_suite/app/slug", "app name")?,
            str_at(&v, "/check_suite/conclusion"),
            None,
            None,
        ),
        "check_run" => (
            required(&v, "/check_run/head_sha", "commit")?,
            str_at(&v, "/check_run/check_suite/head_branch"),
            required(&v, "/check_run/name", "check name")?,
            str_at(&v, "/check_run/conclusion"),
            str_at(&v, "/check_run/html_url"),
            str_at(&v, "/check_run/output/summary"),
        ),
        _ => (
            required(&v, "/workflow_run/head_sha", "commit")?,
            str_at(&v, "/workflow_run/head_branch"),
            str_at(&v, "/workflow_run/name")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(UNNAMED_WORKFLOW),
            str_at(&v, "/workflow_run/conclusion"),
            str_at(&v, "/workflow_run/html_url"),
            None,
        ),
    };
    Ok(Some(CiReport {
        provider: CiProvider::Github,
        repository: repository.to_owned(),
        sha: sha.to_owned(),
        branch: branch
            .map(|b| b.trim().to_owned())
            .filter(|b| !b.is_empty()),
        name: name.to_owned(),
        conclusion: conclusion(conclusion_text),
        url: url.and_then(wire::http_url),
        summary: summary
            .map(|s| wire::truncate(s.to_owned(), MAX_SUMMARY_BYTES))
            .filter(|s| !s.trim().is_empty()),
    }))
}
