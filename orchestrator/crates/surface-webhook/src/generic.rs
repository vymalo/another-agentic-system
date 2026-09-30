//! `POST /webhooks/ci`: the generic signed CI report (`docs/api/webhooks.md`, ADR 0017).
//!
//! A guard, mounted as the route's machine guard, does what must happen before anything else, in
//! this order, and answers 401, 408 or 413 itself when it fails:
//!
//! 1. the signature and timestamp headers are there (401);
//! 2. the timestamp is Unix seconds within the skew window of the clock (401);
//! 3. the body is within 256 KiB (413) and arrives within the read timeout (408);
//! 4. the signature is `HMAC-SHA-256("<timestamp>.<body>")` under one of the secrets (401).
//!
//! Only then does the handler run, on the bytes the guard verified. It reads the JSON (400),
//! normalises it to a [`CiReport`] and hands it to [`App::receive`], which stores an inbox row
//! and returns; the answer is 202 for a new report and for a repeat alike.
//!
//! **The idempotency key is a digest of what was signed** (`"<timestamp>.<body>"`), not the
//! delivery id header: that header is not signed, so anyone who captured a request could replay
//! it under a new id and have it stored again. The same timestamp and body are one delivery
//! whatever the header says; `X-Vymalo-Delivery` is optional and only goes to the log.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Extension, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use orch_api::{ApiError, Problem, SurfaceRoutes};
use orch_app::App;
use orch_core::{CiConclusion, CiProvider, CiReport};
use orch_ports::{Clock, InboxPayload, Ports};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::signature::verify_generic;
use crate::wire::{RefusalLog, Verified, read_limited, text};
use crate::{Secrets, wire};

/// The route.
pub const PATH: &str = "/webhooks/ci";
/// The body limit: 256 KiB. A report is a few hundred bytes and a summary at most 16 KiB.
pub const MAX_BODY_BYTES: usize = 256 * 1024;
/// The optional header with the sender's delivery id. It is not signed, so it is only logged.
pub const DELIVERY_HEADER: &str = "x-vymalo-delivery";
/// The header with the Unix time in whole seconds the sender signed.
pub const TIMESTAMP_HEADER: &str = "x-vymalo-timestamp";
/// The header with `sha256=` and the hex signature.
pub const SIGNATURE_HEADER: &str = "x-vymalo-signature-256";
/// The default of `WEBHOOK_GENERIC_MAX_SKEW_SECS`.
pub const DEFAULT_MAX_SKEW_SECS: u64 = 300;
/// The `source` of the inbox rows this route stores.
pub const SOURCE: &str = "generic";
/// The most bytes of a report's summary that are kept.
pub const MAX_SUMMARY_BYTES: usize = 16 * 1024;
/// The longest check name: a name ends up in a card's id, so it is bounded.
pub const MAX_NAME_BYTES: usize = 256;

/// What the generic route needs.
#[derive(Debug, Clone)]
pub struct GenericConfig {
    /// The shared secrets (`WEBHOOK_GENERIC_SECRETS`).
    pub secrets: Secrets,
    /// How far the timestamp may be from the clock, either way (`WEBHOOK_GENERIC_MAX_SKEW_SECS`).
    pub max_skew: Duration,
    /// How long a delivery may take to arrive and be answered before it is cut off with 408.
    pub read_timeout: Duration,
}

impl GenericConfig {
    /// `secrets`, the default skew of five minutes and the default read timeout of ten seconds.
    pub fn new(secrets: Secrets) -> Self {
        GenericConfig {
            secrets,
            max_skew: Duration::from_secs(DEFAULT_MAX_SKEW_SECS),
            read_timeout: wire::DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

struct State_<P: Ports> {
    app: Arc<App<P>>,
    secrets: Secrets,
    /// The skew in seconds, saturated.
    max_skew_secs: i64,
    read_timeout: Duration,
    refusals: RefusalLog,
}

/// The route `POST /webhooks/ci`, as a machine route: no identity layer, its own guard.
pub fn routes<P: Ports>(app: Arc<App<P>>, cfg: GenericConfig) -> SurfaceRoutes {
    let state = Arc::new(State_ {
        app,
        secrets: cfg.secrets,
        max_skew_secs: i64::try_from(cfg.max_skew.as_secs()).unwrap_or(i64::MAX),
        read_timeout: cfg.read_timeout,
        refusals: RefusalLog::new(PATH),
    });
    let router = Router::new()
        .route(PATH, post(accept::<P>))
        .with_state(Arc::clone(&state));
    SurfaceRoutes::new().machine(router, from_fn_with_state(state, guard::<P>))
}

async fn guard<P: Ports>(
    State(state): State<Arc<State_<P>>>,
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
async fn verify<P: Ports>(state: &State_<P>, req: Request) -> Result<Request, Problem> {
    let (mut parts, body) = req.into_parts();
    let unauthorized = |why| state.refusals.refuse(StatusCode::UNAUTHORIZED, why);

    let signature = text(&parts.headers, SIGNATURE_HEADER)
        .ok_or_else(|| unauthorized("missing or unreadable X-Vymalo-Signature-256"))?;
    let timestamp = text(&parts.headers, TIMESTAMP_HEADER)
        .ok_or_else(|| unauthorized("missing or unreadable X-Vymalo-Timestamp"))?;
    let delivery = wire::delivery_id(&parts.headers, DELIVERY_HEADER);

    let sent = parse_timestamp(timestamp)
        .ok_or_else(|| unauthorized("X-Vymalo-Timestamp is not Unix time in whole seconds"))?;
    let now = state.app.ports().clock().now().as_second();
    if now.abs_diff(sent) > state.max_skew_secs.unsigned_abs() {
        return Err(unauthorized(
            "X-Vymalo-Timestamp is outside the allowed skew of the clock",
        ));
    }

    let bytes = read_limited(&parts.headers, body, MAX_BODY_BYTES)
        .await
        .map_err(|p| state.refusals.refused(p))?;
    if !verify_generic(&state.secrets, timestamp, &bytes, signature) {
        return Err(unauthorized("the signature does not match"));
    }

    parts.extensions.insert(Verified {
        key: signed_digest(timestamp, &bytes),
        body: bytes,
        delivery,
        event: String::new(),
    });
    Ok(Request::from_parts(parts, axum::body::Body::empty()))
}

/// The lower-case hex SHA-256 of the signed string `"<timestamp>.<body>"`: the same timestamp and
/// body are the same delivery, whatever headers came with them.
fn signed_digest(timestamp: &str, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(timestamp.as_bytes());
    hasher.update(b".");
    hasher.update(body);
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `X-Vymalo-Timestamp`: ASCII digits only (no sign, no blank, no fraction), at most 12 of them.
fn parse_timestamp(text: &str) -> Option<i64> {
    if text.is_empty() || text.len() > 12 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

async fn accept<P: Ports>(
    State(state): State<Arc<State_<P>>>,
    verified: Option<Extension<Verified>>,
) -> Result<StatusCode, ApiError> {
    // Reached without the guard: refuse rather than accept an unverified body.
    let Some(Extension(verified)) = verified else {
        tracing::error!(
            route = PATH,
            "the generic webhook handler ran without its guard"
        );
        return Err(Problem::new(StatusCode::UNAUTHORIZED, "not verified").into());
    };
    let report = parse(&verified.body)?;
    let received = state
        .app
        .receive(SOURCE, &verified.key, InboxPayload::CiReport(report))
        .await?;
    tracing::info!(
        route = PATH,
        ?received,
        key = %verified.key,
        delivery = %verified.delivery,
        "webhook report received"
    );
    Ok(StatusCode::ACCEPTED)
}

/// The body of `POST /webhooks/ci`. Unknown members are ignored.
#[derive(Deserialize)]
struct Body {
    version: u64,
    repository: String,
    sha: String,
    #[serde(default)]
    branch: Option<String>,
    name: String,
    conclusion: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// The conclusions of the generic body: GitHub's check-run vocabulary, closed.
fn conclusion(name: &str) -> Option<CiConclusion> {
    Some(match name {
        "success" => CiConclusion::Success,
        "neutral" => CiConclusion::Neutral,
        "skipped" => CiConclusion::Skipped,
        "failure" => CiConclusion::Failure,
        "cancelled" => CiConclusion::Cancelled,
        "timed_out" => CiConclusion::TimedOut,
        "action_required" => CiConclusion::ActionRequired,
        "stale" => CiConclusion::Stale,
        _ => return None,
    })
}

/// The body of a verified delivery as a report, or the 400 that says what is wrong with it.
fn parse(bytes: &[u8]) -> Result<CiReport, Problem> {
    let body: Body = serde_json::from_slice(bytes)
        .map_err(|e| Problem::bad_request(format!("the body is not a CI report: {e}")))?;
    if body.version != 1 {
        return Err(Problem::bad_request(format!(
            "version {} is not supported (only 1)",
            body.version
        )));
    }
    let Some(conclusion) = conclusion(&body.conclusion) else {
        return Err(Problem::bad_request(
            "conclusion must be one of success, neutral, skipped, failure, cancelled, timed_out, \
             action_required, stale",
        ));
    };
    let name = body.name.trim();
    if name.is_empty() {
        return Err(Problem::bad_request("name must not be empty"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(Problem::bad_request(format!(
            "name is over {MAX_NAME_BYTES} bytes"
        )));
    }
    Ok(CiReport {
        provider: CiProvider::Generic,
        repository: body.repository,
        sha: body.sha,
        branch: body
            .branch
            .map(|b| b.trim().to_owned())
            .filter(|b| !b.is_empty()),
        name: name.to_owned(),
        conclusion,
        url: body.url.and_then(|u| wire::http_url(&u)),
        summary: body
            .summary
            .map(|s| wire::truncate(s, MAX_SUMMARY_BYTES))
            .filter(|s| !s.is_empty()),
    })
}
