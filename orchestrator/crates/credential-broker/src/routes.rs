//! The broker's internal run routes (ADR 0084 decisions 1, 2, 5 and 6).
//!
//! Four routes, served on the broker's **internal listener only** (the chart and the NetworkPolicy
//! are ANR-147b; they are not in `docs/api/chat-api.yaml`, and a test here holds that):
//!
//! | Route | Caller | Answer |
//! |---|---|---|
//! | `POST /internal/runs` | the orchestrator | `204`; `409 run_already_bound` for a different binding of the same run |
//! | `DELETE /internal/runs/{run}` | the orchestrator | `204`, idempotent |
//! | `GET /internal/runs/{run}/lease` | the orchestrator or the injector, never an agent | `200 {token, accountId, expiresAt}`, `Cache-Control: no-store` |
//! | `GET /internal/runs/{run}/account` | the operator's meter identity alone | `200 {accountId}`, `Cache-Control: no-store`; `run_not_bound` for an unknown run and for every other caller |
//!
//! A refusal is one of the ruled slugs: `run_not_bound` (`404`), `lease_expired` (`410`) and
//! `unavailable` (`503`), plus `unauthenticated` (`401`) and `not_allowed` (`403`) for the door.
//! The run id alone is never enough: the caller's identity comes from [`RunCallerAuth`], the
//! broker's own authentication of the request, and a caller that is not the run's agent is
//! `run_not_bound` — the answer never says whether a run exists.
//!
//! The lease value and the account it names are written to the response body and nothing else:
//! neither is ever logged, and no error holds either. `POST` and `DELETE` are the orchestrator's: an
//! agent that tries to bind is `not_allowed`, and a lookup is refused for the orchestrator itself.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use jiff::Timestamp;
use orch_core::AgentId;
use orch_ports::{RunBinding, RunCaller, RunId, RunLeaseError, RunLeases};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The longest field the bind route accepts: the ids the orchestrator sends are far shorter, and
/// the cap keeps a stranger's body from becoming a row.
pub const MAX_FIELD_CHARS: usize = 256;

/// Who the request is from, as the broker authenticates it (ADR 0084 decision 5). S9 replaces the
/// implementation with a TokenReview of a service-account token, or mTLS; the routes never read the
/// presented credential themselves.
pub trait RunCallerAuth: Send + Sync + 'static {
    /// The caller this request presents, or `None` when the request is not authenticated.
    fn caller(&self, headers: &HeaderMap) -> Option<RunCaller>;
}

/// Today's scheme: one bearer token for the orchestrator, one for the injector and one per agent,
/// from the deployment's configuration. Tokens are compared by SHA-256 digest so a wrong one costs
/// the same as a right one, and nothing here ever prints them.
#[derive(Debug)]
pub struct SharedTokens {
    orchestrator: [u8; 32],
    /// The injector's token, when the deployment runs one: the only caller besides the orchestrator
    /// that may read a run's lease (ADR 0084 decision 2, amended 2026-11-24).
    injector: Option<[u8; 32]>,
    /// The operator's pod-minute meter token, when the deployment runs one: the only caller of the
    /// account route, and refused everything else.
    meter: Option<[u8; 32]>,
    agents: BTreeMap<[u8; 32], AgentId>,
}

impl SharedTokens {
    /// The orchestrator's token and each agent's.
    pub fn new<I>(orchestrator: &str, agents: I) -> Self
    where
        I: IntoIterator<Item = (String, AgentId)>,
    {
        SharedTokens {
            orchestrator: digest(orchestrator),
            injector: None,
            meter: None,
            agents: agents
                .into_iter()
                .map(|(token, agent)| (digest(&token), agent))
                .collect(),
        }
    }
}

impl SharedTokens {
    /// The same scheme with the injector's token: the caller that may read a run's lease and may do
    /// nothing else.
    #[must_use]
    pub fn with_injector(mut self, injector: &str) -> Self {
        self.injector = Some(digest(injector));
        self
    }
}

impl SharedTokens {
    /// The same scheme with the meter's token: the caller that may ask which account a run is
    /// charged to and may do nothing else.
    #[must_use]
    pub fn with_meter(mut self, meter: &str) -> Self {
        self.meter = Some(digest(meter));
        self
    }
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl RunCallerAuth for SharedTokens {
    fn caller(&self, headers: &HeaderMap) -> Option<RunCaller> {
        let presented = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let presented = digest(presented);
        if presented == self.orchestrator {
            return Some(RunCaller::Orchestrator);
        }
        if self.injector == Some(presented) {
            return Some(RunCaller::Injector);
        }
        if self.meter == Some(presented) {
            return Some(RunCaller::Meter);
        }
        self.agents.get(&presented).cloned().map(RunCaller::Agent)
    }
}

/// The bind body: what the orchestrator sends at run creation (ADR 0084 decision 1). It names
/// people and agents and holds no value.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindBody {
    /// The root run's id.
    pub run: String,
    /// The agent the run is bound to.
    pub agent: String,
    /// The person the run spends for.
    pub person: String,
    /// The model endpoint the lease is for, when the binding names one.
    pub model: Option<String>,
    /// The run's deadline, RFC 3339. The broker caps it at 24 hours.
    pub expires_at: String,
}

/// The lease body: the one answer that holds a secret, beside the account whose call it is.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LeaseBody {
    token: String,
    /// The run's person (`UserId`), the key the usage ledger is kept by (ADR 0065). Never logged.
    account_id: String,
    expires_at: Timestamp,
}

/// The account body: the run's person and nothing else.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountBody {
    /// The run's person (`UserId`). Never logged.
    account_id: String,
}

/// The refusal body: the ruled slug and nothing else.
#[derive(Serialize)]
struct Refusal {
    code: &'static str,
}

struct RouteState<L, A> {
    leases: Arc<L>,
    auth: Arc<A>,
}

// Hand-written: `derive` would require `L` and `A` to be `Clone`, and the two `Arc`s are the only
// state the handlers clone.
impl<L, A> Clone for RouteState<L, A> {
    fn clone(&self) -> Self {
        RouteState {
            leases: Arc::clone(&self.leases),
            auth: Arc::clone(&self.auth),
        }
    }
}

/// The three routes over `leases`, authenticated by `auth`.
pub fn router<L, A>(leases: Arc<L>, auth: Arc<A>) -> Router
where
    L: RunLeases,
    A: RunCallerAuth,
{
    let state = RouteState { leases, auth };
    Router::new()
        .route("/internal/runs", post(bind::<L, A>))
        .route("/internal/runs/{run}", delete(unbind::<L, A>))
        .route("/internal/runs/{run}/lease", get(lease::<L, A>))
        .route("/internal/runs/{run}/account", get(account::<L, A>))
        .with_state(state)
}

fn refuse(status: StatusCode, code: &'static str) -> Response {
    (status, Json(Refusal { code })).into_response()
}

fn unauthenticated() -> Response {
    let mut response = refuse(StatusCode::UNAUTHORIZED, "unauthenticated");
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    response
}

/// `true` when every id the body names fits the cap.
fn fits(body: &BindBody) -> bool {
    let fits = |value: &str| !value.is_empty() && value.chars().count() <= MAX_FIELD_CHARS;
    fits(&body.run)
        && fits(&body.agent)
        && fits(&body.person)
        && body.model.as_deref().is_none_or(fits)
}

async fn bind<L, A>(
    State(state): State<RouteState<L, A>>,
    headers: HeaderMap,
    body: Result<Json<BindBody>, axum::extract::rejection::JsonRejection>,
) -> Response
where
    L: RunLeases,
    A: RunCallerAuth,
{
    match state.auth.caller(&headers) {
        Some(RunCaller::Orchestrator) => {}
        Some(RunCaller::Agent(_) | RunCaller::Injector | RunCaller::Meter) => {
            return refuse(StatusCode::FORBIDDEN, "not_allowed");
        }
        None => return unauthenticated(),
    }
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return refuse(StatusCode::UNPROCESSABLE_ENTITY, "invalid_binding"),
    };
    if !fits(&body) {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "invalid_binding");
    }
    let Ok(expires_at) = body.expires_at.parse::<Timestamp>() else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "invalid_binding");
    };
    let binding = RunBinding {
        run: RunId::new(&body.run),
        agent: AgentId(body.agent),
        person: orch_core::UserId::new(&body.person),
        model: body.model,
        expires_at,
    };
    match state.leases.bind(binding).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(error),
    }
}

async fn unbind<L, A>(
    State(state): State<RouteState<L, A>>,
    headers: HeaderMap,
    Path(run): Path<String>,
) -> Response
where
    L: RunLeases,
    A: RunCallerAuth,
{
    match state.auth.caller(&headers) {
        Some(RunCaller::Orchestrator) => {}
        Some(RunCaller::Agent(_) | RunCaller::Injector | RunCaller::Meter) => {
            return refuse(StatusCode::FORBIDDEN, "not_allowed");
        }
        None => return unauthenticated(),
    }
    if run.is_empty() || run.chars().count() > MAX_FIELD_CHARS {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "invalid_binding");
    }
    match state.leases.unbind(&RunId::new(&run)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(error),
    }
}

async fn lease<L, A>(
    State(state): State<RouteState<L, A>>,
    headers: HeaderMap,
    Path(run): Path<String>,
) -> Response
where
    L: RunLeases,
    A: RunCallerAuth,
{
    let caller = match state.auth.caller(&headers) {
        Some(caller) => caller,
        None => return unauthenticated(),
    };
    match state.leases.lease(&RunId::new(&run), &caller).await {
        Ok(lease) => {
            let body = LeaseBody {
                token: lease.token().expose().to_owned(),
                account_id: lease.account().to_string(),
                expires_at: lease.expires_at(),
            };
            let mut response = (StatusCode::OK, Json(body)).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(error) => refusal(error),
    }
}

async fn account<L, A>(
    State(state): State<RouteState<L, A>>,
    headers: HeaderMap,
    Path(run): Path<String>,
) -> Response
where
    L: RunLeases,
    A: RunCallerAuth,
{
    let caller = match state.auth.caller(&headers) {
        Some(caller) => caller,
        None => return unauthenticated(),
    };
    match state.leases.account(&RunId::new(&run), &caller).await {
        Ok(person) => {
            let body = AccountBody {
                account_id: person.to_string(),
            };
            let mut response = (StatusCode::OK, Json(body)).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(error) => refusal(error),
    }
}

/// The ruled refusal: the slug is the contract's, the status the one a client acts on.
fn refusal(error: RunLeaseError) -> Response {
    match error {
        RunLeaseError::NotBound => refuse(StatusCode::NOT_FOUND, "run_not_bound"),
        RunLeaseError::Expired => refuse(StatusCode::GONE, "lease_expired"),
        RunLeaseError::Conflict => refuse(StatusCode::CONFLICT, "run_already_bound"),
        RunLeaseError::Unavailable(_) => refuse(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
