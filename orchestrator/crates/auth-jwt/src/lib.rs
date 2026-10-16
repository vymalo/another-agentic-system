//! [`Authenticator`] over OAuth2 bearer tokens: a JWT signed by the issuer, validated here against
//! the issuer's keys (ADR 0033, `auth.mode: jwt`). The orchestrator is a resource server: it
//! trusts no header a client can send, only a token it can verify.
//!
//! ```mermaid
//! sequenceDiagram
//!   participant C as Client (the web through oauth2-proxy, a CLI, CI)
//!   participant A as JwtAuth
//!   participant F as Fetch task (one at a time)
//!   participant I as Issuer (discovery, JWKS)
//!   C->>A: Authorization: Bearer token
//!   A->>A: header: alg allowed? kid?
//!   alt keys held and fresh
//!     A->>A: pick the key of the kid
//!   else keys older than 10 minutes, within the hour of grace
//!     A->>A: pick the key of the kid in the stale copy, at once
//!     A-)F: start a refresh, unless one runs or one failed just now
//!     F->>I: GET discovery, GET jwks
//!     I-->>F: keys (recorded when they come)
//!   else never fetched, or past the grace
//!     A->>F: start a fetch, or join the one that runs
//!     F->>I: GET discovery, GET jwks
//!     I-->>F: keys
//!     F-->>A: done, the keys are recorded
//!   else the kid is unknown (at most once in 30 s)
//!     A->>F: join the fetch that runs, or start one
//!     F-->>A: done, the keys are recorded
//!   end
//!   A->>A: signature, exp, nbf, iss, aud, iat, email_verified, user
//!   A-->>C: the principal, or Invalid, or Unavailable
//! ```
//!
//! # What is checked
//!
//! - **Algorithm**: RS256, RS384, ES256 and EdDSA, and nothing else. `none` and the HMAC family are
//!   refused whatever key they name (the algorithm confusion attack), and the key a token is
//!   verified with must be of the algorithm's family.
//! - **Key**: the keys of the issuer's JWKS that the token's `kid` names, **every one of them** (two
//!   keys may share a `kid`; the token is good if any of them signed it). A JWKS key that cannot
//!   verify (symmetric, too small, for encryption, another curve) is skipped. The members of a key
//!   are read as the numbers they are: an RSA integer by its magnitude, a P-256 coordinate in its 32
//!   octets, an Ed25519 key in its 32 octets, however the issuer cut, padded or signed them.
//! - **Claims**, in this order: `exp` present and not past, `nbf` not in the future, both with 60
//!   seconds of leeway; `iss` **one text** equal to the configured issuer, exactly (a list that
//!   holds it is refused); the configured audience among `aud`; `iat` present; `email_verified`
//!   not `false`; the configured user claim a text that is not blank and has no control
//!   character. With several claims wrong, the reason is the first in this order.
//! - **Size**: a token of more than 16 KiB is refused unread.
//!
//! # DPoP
//!
//! With [`JwtConfig::with_dpop`] a request may instead carry `Authorization: DPoP <token>` and a
//! `DPoP` proof (RFC 9449, ADR 0054). The token is validated exactly as a bearer is, and must be
//! bound to the proof's key (`cnf.jkt`); see [`dpop`] for the proof's checks. **A token that carries
//! `cnf` presented as `Bearer` is refused** when DPoP is configured, and a `DPoP` request is refused
//! when it is not (fail closed).
//!
//! # The key cache
//!
//! The keys are a copy this process may lose at any time (ADR 0001). They are fetched on first use
//! (and by [`Authenticator::ready`], which `/readyz` calls), refreshed once they are 10 minutes
//! old, and fetched again when a token names a `kid` the copy lacks, **at most once in 30 seconds**
//! (a flood of invented ids is not a flood of requests to the issuer). A fetch that fails is not
//! tried again for 5 seconds. While no copy exists, or the copy is older than the refresh interval
//! plus an hour, every request is [`AuthError::Unavailable`] (fail closed, and not a refusal: a
//! caller may retry); between the two, the copy goes on verifying while the issuer is down.
//!
//! **A fetch runs on a task of its own, one at a time, and no lock is held while it runs** (the lock
//! is held to read or change the copy, never across a request to the issuer). A request waits for a
//! fetch only when it must: with no copy of the keys, or one past its grace, it waits for the fetch
//! (each request to the issuer times out) and is `Unavailable` if there is still nothing to verify with;
//! a request whose token names a `kid` the copy lacks waits for the fetch, and no other request
//! does. A copy older than the refresh interval and within the grace **is used at once** and is
//! refreshed in the background, so a slow or hanging issuer holds up nobody who has a copy, and
//! [`Authenticator::ready`] answers at once while a copy can verify. A request that is dropped
//! while it waits leaves the fetch running, and the keys it brings are recorded all the same. The
//! task holds the authenticator's state and nothing ends it early, so a fetch outlives the last
//! clone of the authenticator by at most what its requests may take (the discovery document, then
//! the keys: the request timeout each).
//!
//! The keys are read from `jwksUrl`, or from the `jwks_uri` of
//! `{issuer}/.well-known/openid-configuration`, whose `issuer` must be the configured one. No
//! redirect is followed, a body over 1 MiB is refused, and a request times out.

mod claims;
pub mod dpop;
mod fetch;
mod keys;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use orch_ports::{
    AuthError, Authenticator, CredentialKind, Credentials, DpopCredentials, Principal,
};
use serde_json::{Map, Value};
use tokio::sync::watch;
use url::Url;

use crate::dpop::Dpop;
pub use crate::dpop::DpopConfig;
use crate::fetch::{Fetcher, Source};
use crate::keys::{ALLOWED, KeySet, Parsed, kind_of};

/// The longest token read.
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// How old the keys may be before they are fetched again (10 minutes).
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

/// The least time between two fetches that an unknown `kid` causes (30 seconds).
pub const KID_REFETCH_INTERVAL: Duration = Duration::from_secs(30);

/// The least time between a failed fetch and the next one (5 seconds).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// How long keys older than the refresh interval go on verifying while the issuer cannot be
/// reached (1 hour).
pub const STALE_GRACE: Duration = Duration::from_secs(3600);

/// The leeway of `exp` and `nbf` (60 seconds).
pub const LEEWAY_SECS: u64 = 60;

/// The configuration of the JWT authenticator (`auth.jwt`).
#[derive(Clone)]
pub struct JwtConfig {
    /// The issuer: `iss` must equal it exactly, and its discovery document is
    /// `{issuer}/.well-known/openid-configuration`.
    pub issuer: String,
    /// The audiences this API accepts: one of them must be among the token's `aud`.
    pub audiences: Vec<String>,
    /// Where the JWKS is, when not in the discovery document.
    pub jwks_url: Option<String>,
    /// The claim whose value is the user (default `email`).
    pub user_claim: String,
    /// The path of the claim that holds the roles, with dots for the objects on the way
    /// (`realm_access.roles`); none means no roles.
    pub roles_claim: Option<String>,
    /// How old the keys may be before they are fetched again.
    pub refresh_interval: Duration,
    /// The least time between two fetches that an unknown `kid` causes.
    pub kid_refetch_interval: Duration,
    /// The least time between a failed fetch and the next one.
    pub retry_interval: Duration,
    /// How long keys older than the refresh interval go on verifying while the issuer is down.
    pub stale_grace: Duration,
    /// The time one request to the issuer may take.
    pub timeout: Duration,
    /// Whether to honour `HTTP(S)_PROXY` from the environment, as the A2A client does.
    pub use_system_proxy: bool,
    /// DPoP-bound tokens (`auth.dpop`); `None` refuses the `DPoP` scheme.
    pub dpop: Option<DpopConfig>,
}

impl JwtConfig {
    /// A configuration for `issuer` and `audiences`: the user is the `email` claim, no roles,
    /// the intervals of this crate's constants and a 5 second timeout.
    pub fn new(
        issuer: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        JwtConfig {
            issuer: issuer.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
            jwks_url: None,
            user_claim: "email".to_owned(),
            roles_claim: None,
            refresh_interval: REFRESH_INTERVAL,
            kid_refetch_interval: KID_REFETCH_INTERVAL,
            retry_interval: RETRY_INTERVAL,
            stale_grace: STALE_GRACE,
            timeout: Duration::from_secs(5),
            use_system_proxy: true,
            dpop: None,
        }
    }

    /// Takes DPoP-bound tokens too (RFC 9449, ADR 0054), and refuses a bound token sent as a bearer.
    #[must_use]
    pub fn with_dpop(mut self, dpop: DpopConfig) -> Self {
        self.dpop = Some(dpop);
        self
    }

    /// Reads the JWKS from `url` instead of the discovery document.
    #[must_use]
    pub fn with_jwks_url(mut self, url: impl Into<String>) -> Self {
        self.jwks_url = Some(url.into());
        self
    }

    /// The user is the claim `claim`.
    #[must_use]
    pub fn with_user_claim(mut self, claim: impl Into<String>) -> Self {
        self.user_claim = claim.into();
        self
    }

    /// The roles are at `path`.
    #[must_use]
    pub fn with_roles_claim(mut self, path: impl Into<String>) -> Self {
        self.roles_claim = Some(path.into());
        self
    }

    /// Reaches the issuer directly, whatever the environment's proxy says.
    #[must_use]
    pub fn without_system_proxy(mut self) -> Self {
        self.use_system_proxy = false;
        self
    }
}

impl std::fmt::Debug for JwtConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtConfig")
            .field("issuer", &self.issuer)
            .field("audiences", &self.audiences)
            .field("jwks_url", &self.jwks_url)
            .field("user_claim", &self.user_claim)
            .field("roles_claim", &self.roles_claim)
            .field("dpop", &self.dpop)
            .finish_non_exhaustive()
    }
}

/// Why a [`JwtAuth`] could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The issuer is not an absolute `http(s)` URL with a host, without credentials, a query or a
    /// fragment.
    #[error("the issuer is not an http(s) URL with a host, without credentials, query or fragment")]
    BadIssuer,
    /// The JWKS URL is not an absolute `http(s)` URL with a host and no credentials.
    #[error("the JWKS URL is not an http(s) URL with a host and no credentials")]
    BadJwksUrl,
    /// No audience, or an empty one: every token would be refused, or every one accepted.
    #[error("at least one audience is needed, and none may be empty")]
    NoAudience,
    /// The user claim or the roles claim is empty.
    #[error("a claim name is empty")]
    EmptyClaim,
    /// A DPoP public origin is not an `http(s)` origin (`https://host[:port]`) without credentials,
    /// path, query or fragment.
    #[error(
        "a DPoP public origin is not an http(s) origin without credentials, path, query or fragment"
    )]
    BadDpopOrigin,
    /// DPoP is configured with no public origin: every proof would be refused.
    #[error("DPoP needs at least one public origin")]
    NoDpopOrigin,
    /// The proof's age is zero, or the replay cache has no room.
    #[error("the DPoP proof's age and the replay cache's size must be above zero")]
    BadDpopWindow,
    /// The HTTP client could not be built.
    #[error("cannot build the HTTP client")]
    Http(#[source] Box<dyn std::error::Error + Send + Sync>),
}

fn http_url(text: &str) -> Option<Url> {
    let url = Url::parse(text.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// The copy of the keys, and what is known about fetching them. Behind a lock that is held to read or to
/// change this and for nothing else: never across a request to the issuer.
#[derive(Default)]
struct State {
    /// The keys of the last successful fetch, and when it started.
    keys: Option<(Arc<KeySet>, Instant)>,
    /// When the last fetch, successful or not, started.
    last_attempt: Option<Instant>,
    /// When the last fetch failed, until one succeeds.
    last_failure: Option<Instant>,
    /// The fetch that is running, if one is: what a request waits on. It ends when its sender is dropped.
    in_flight: Option<watch::Receiver<()>>,
}

impl State {
    /// The fetch that is running, if there is one.
    fn running(&mut self) -> Option<watch::Receiver<()>> {
        match &self.in_flight {
            // A fetch whose sender is gone is over; a task that was dropped before it ran leaves such a mark.
            Some(wait) if wait.has_changed().is_err() => {
                self.in_flight = None;
                None
            }
            Some(wait) => Some(wait.clone()),
            None => None,
        }
    }

    /// Whether the last fetch failed less than `retry` ago.
    fn backing_off(&self, retry: Duration) -> bool {
        self.last_failure
            .is_some_and(|failed| failed.elapsed() < retry)
    }
}

struct Inner {
    issuer: String,
    audiences: Vec<String>,
    user_claim: String,
    roles_claim: Option<String>,
    refresh_interval: Duration,
    kid_refetch_interval: Duration,
    retry_interval: Duration,
    stale_grace: Duration,
    fetcher: Fetcher,
    state: Mutex<State>,
    dpop: Option<Dpop>,
}

/// A running fetch, from inside the task that does it. However the task ends (it finished, it panicked, the
/// runtime stopped), the outcome is recorded and the fetch is over: the waiters wake after, and find it.
struct Flight {
    inner: Arc<Inner>,
    started: Instant,
    outcome: Option<Result<Parsed, String>>,
    /// Dropped after the outcome is recorded: the waiters wake when it is gone.
    _done: watch::Sender<()>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err("the fetch ended without an answer".to_owned()));
        self.inner.record(self.started, outcome);
    }
}

/// The JWT authenticator. Cheap to clone: clones share the keys.
#[derive(Clone)]
pub struct JwtAuth {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for JwtAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtAuth")
            .field("issuer", &self.inner.issuer)
            .field("audiences", &self.inner.audiences)
            .finish_non_exhaustive()
    }
}

impl JwtAuth {
    /// Builds the authenticator. It reads nothing: the keys are fetched on first use.
    ///
    /// # Errors
    /// [`BuildError`] for an issuer or a JWKS URL that is not an `http(s)` URL, no audience, an
    /// empty claim name, or a TLS backend that fails to start.
    pub fn new(cfg: JwtConfig) -> Result<Self, BuildError> {
        install_crypto_provider();
        let issuer_url = http_url(&cfg.issuer)
            .filter(|u| u.query().is_none() && u.fragment().is_none())
            .ok_or(BuildError::BadIssuer)?;
        if cfg.audiences.is_empty() || cfg.audiences.iter().any(|a| a.trim().is_empty()) {
            return Err(BuildError::NoAudience);
        }
        if cfg.user_claim.trim().is_empty()
            || cfg
                .roles_claim
                .as_deref()
                .is_some_and(|c| c.trim().is_empty())
        {
            return Err(BuildError::EmptyClaim);
        }
        let jwks_url = cfg
            .jwks_url
            .as_deref()
            .map(|url| http_url(url).ok_or(BuildError::BadJwksUrl))
            .transpose()?;
        // The keys every token is checked against travel in the clear: anyone on the network path can swap
        // them. Said once, here, and not as a refusal: a Keycloak in the cluster is often reached over plain
        // http, on purpose. The URL is not repeated (it is the operator's own, and may hold a secret).
        if issuer_url.scheme() == "http" || jwks_url.as_ref().is_some_and(|u| u.scheme() == "http")
        {
            tracing::warn!(
                "plain http is configured for the token issuer or its keys; anyone on the network path could swap the keys every token is checked against (use https unless the network is trusted)"
            );
        }
        let source = match jwks_url {
            Some(url) => Source::Url(url),
            None => {
                // OpenID Connect Discovery 1.0, 4: a terminating `/` of the issuer is removed (one, never all
                // of them), and the path is appended. `Url` writes a bare host as `http://host/`.
                let base = issuer_url.as_str();
                let base = base.strip_suffix('/').unwrap_or(base);
                let document = Url::parse(&format!("{base}/.well-known/openid-configuration"))
                    .map_err(|_| BuildError::BadIssuer)?;
                Source::Discover {
                    issuer: cfg.issuer.clone(),
                    document,
                }
            }
        };
        // Err means a provider is already installed, which is what we want.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let dpop = cfg.dpop.map(Dpop::new).transpose()?;
        let http = fetch::client(cfg.timeout, cfg.use_system_proxy)
            .map_err(|e| BuildError::Http(e.without_url().into()))?;
        Ok(JwtAuth {
            inner: Arc::new(Inner {
                issuer: cfg.issuer,
                audiences: cfg.audiences,
                user_claim: cfg.user_claim,
                roles_claim: cfg.roles_claim,
                refresh_interval: cfg.refresh_interval,
                kid_refetch_interval: cfg.kid_refetch_interval,
                retry_interval: cfg.retry_interval,
                stale_grace: cfg.stale_grace,
                fetcher: Fetcher::new(http, source),
                state: Mutex::new(State::default()),
                dpop,
            }),
        })
    }
}

fn unavailable() -> AuthError {
    AuthError::unavailable("the token issuer's keys cannot be fetched")
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        // A panic with the lock held (a spawn outside a runtime) leaves a state that was valid: nothing is half-written.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the issuer for its keys. Holds no lock: the state is touched after, by [`record`](Self::record).
    async fn fetch_keys(&self) -> Result<Parsed, String> {
        let document = self.fetcher.jwks().await.map_err(|e| e.to_string())?;
        match KeySet::parse(&document) {
            Some(parsed) if !parsed.set.is_empty() => Ok(parsed),
            Some(_) => Err("the key set has no key this build can verify with".to_owned()),
            None => Err("the answer is not a key set".to_owned()),
        }
    }

    /// Records what came of the fetch that started at `started`, and ends it.
    fn record(&self, started: Instant, outcome: Result<Parsed, String>) {
        let (recovered, skipped, failed) = {
            let mut state = self.state();
            state.in_flight = None;
            match outcome {
                Ok(parsed) => {
                    let recovered = state.last_failure.take().is_some();
                    let skipped = parsed.skipped;
                    state.keys = Some((Arc::new(parsed.set), started));
                    (recovered, skipped, None)
                }
                Err(cause) => {
                    let first = state.last_failure.is_none();
                    state.last_failure = Some(Instant::now());
                    (false, 0, Some((cause, first)))
                }
            }
        };
        if recovered {
            tracing::info!("the token issuer's keys can be fetched again");
        }
        if skipped > 0 {
            tracing::debug!(skipped, "keys of the issuer this build cannot use");
        }
        match failed {
            Some((cause, true)) => {
                tracing::warn!(%cause, "the token issuer's keys cannot be fetched; tokens are refused until they can");
            }
            Some((cause, false)) => {
                tracing::debug!(%cause, "the token issuer's keys still cannot be fetched");
            }
            None => {}
        }
    }

    /// Starts a fetch of the keys on a task of its own, and gives what to wait on for it to be over. **A fetch
    /// belongs to nobody who waits for it**: a request that is dropped while it waits leaves it running, and the
    /// keys it brings are recorded all the same. Called with the lock held, and not to be called while a fetch is
    /// running (`state.running()`).
    fn start_fetch(self: &Arc<Self>, state: &mut State) -> watch::Receiver<()> {
        let started = Instant::now();
        let (done, wait) = watch::channel(());
        let inner = Arc::clone(self);
        // The task cannot record before this returns: `record` takes the lock the caller holds.
        tokio::spawn(async move {
            let mut flight = Flight {
                inner,
                started,
                outcome: None,
                _done: done,
            };
            flight.outcome = Some(flight.inner.fetch_keys().await);
        });
        state.last_attempt = Some(started);
        state.in_flight = Some(wait.clone());
        wait
    }

    /// The copy, if it may still verify: no older than the refresh interval and the grace.
    fn usable(&self, state: &State) -> Option<Arc<KeySet>> {
        let (keys, at) = state.keys.as_ref()?;
        (at.elapsed() <= self.refresh_interval + self.stale_grace).then(|| Arc::clone(keys))
    }

    /// The keys to verify with.
    ///
    /// - A fresh copy is used at once.
    /// - A copy older than the refresh interval and within its grace is used at once too, and a refresh is
    ///   started in the background if none is running and the last one did not fail just now: **a request does
    ///   not wait for a refresh while a copy can verify**.
    /// - With no copy, or one past its grace, the request waits for the fetch that is running (it starts one
    ///   unless the last failed just now) and fails closed if there is still nothing to verify with.
    async fn current(self: &Arc<Self>) -> Result<Arc<KeySet>, AuthError> {
        let wait = {
            let mut state = self.state();
            if let Some((keys, at)) = &state.keys {
                let age = at.elapsed();
                if age < self.refresh_interval {
                    return Ok(Arc::clone(keys));
                }
                if age <= self.refresh_interval + self.stale_grace {
                    let keys = Arc::clone(keys);
                    if state.running().is_none() && !state.backing_off(self.retry_interval) {
                        self.start_fetch(&mut state);
                    }
                    return Ok(keys);
                }
            }
            match state.running() {
                Some(wait) => wait,
                None if state.backing_off(self.retry_interval) => return Err(unavailable()),
                None => self.start_fetch(&mut state),
            }
        };
        finished(wait).await;
        let state = self.state();
        self.usable(&state).ok_or_else(unavailable)
    }

    /// The keys after a token named a `kid` that `seen` lacks: the fetch that is running, or one of its own
    /// unless a fetch was tried within the interval (any fetch counts), or someone else already brought
    /// better keys. Only the request that names the unknown key waits. Never fails: without a better copy,
    /// `seen` is it.
    async fn after_unknown_kid(self: &Arc<Self>, seen: &Arc<KeySet>) -> Arc<KeySet> {
        let wait = {
            let mut state = self.state();
            if let Some((keys, _)) = &state.keys
                && !Arc::ptr_eq(keys, seen)
            {
                return Arc::clone(keys);
            }
            match state.running() {
                Some(wait) => Some(wait),
                None => {
                    let recent = state
                        .last_attempt
                        .is_some_and(|at| at.elapsed() < self.kid_refetch_interval);
                    (!recent).then(|| self.start_fetch(&mut state))
                }
            }
        };
        if let Some(wait) = wait {
            finished(wait).await;
        }
        let state = self.state();
        self.usable(&state).unwrap_or_else(|| Arc::clone(seen))
    }

    /// What the library checks: the signature, then `exp` (required, and not past) and `nbf`, both with the
    /// leeway. `iss` and `aud` are checked after, by [`check_issuer_and_audience`], in that order: the library's
    /// own checks of them read a list where a text is meant (an `iss` that is a list holding ours passes), and
    /// the first of several required claims it finds missing is the first in the order of a hash set, so the
    /// reason for a token that lacks several would change from one request to the next.
    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = false;
        validation.set_required_spec_claims(&["exp"]);
        validation
    }

    /// The token decoded with the first of `keys` that verifies its signature. Only a bad signature moves on to
    /// the next key; any other reason (the times, a claim) is of a token that a key did verify, and ends it.
    fn decode_with_any(
        &self,
        token: &str,
        alg: Algorithm,
        keys: &[DecodingKey],
    ) -> Result<TokenData<Map<String, Value>>, AuthError> {
        let validation = self.validation(alg);
        for key in keys {
            match decode::<Map<String, Value>>(token, key, &validation) {
                Ok(data) => return Ok(data),
                Err(e) if matches!(e.kind(), ErrorKind::InvalidSignature) => {}
                Err(e) => return Err(refusal(e.kind())),
            }
        }
        Err(refusal(&ErrorKind::InvalidSignature))
    }
}

/// Waits for a fetch to be over: its sender is dropped, and nothing is ever sent.
async fn finished(mut wait: watch::Receiver<()>) {
    let _ = wait.changed().await;
}

/// Why a token was refused, in words fit for the caller. Never a value of the token.
fn refusal(kind: &ErrorKind) -> AuthError {
    match kind {
        ErrorKind::ExpiredSignature => AuthError::invalid_bearer("the token has expired"),
        ErrorKind::ImmatureSignature => AuthError::invalid_bearer("the token is not valid yet"),
        ErrorKind::InvalidSignature => {
            AuthError::invalid_bearer("the token's signature does not verify")
        }
        ErrorKind::MissingRequiredClaim(claim) => missing_claim(claim),
        _ => AuthError::invalid_bearer("the token is not valid"),
    }
}

fn missing_claim(claim: &str) -> AuthError {
    AuthError::invalid_bearer(format!("the token has no {claim:?} claim"))
}

fn not_for_this_api() -> AuthError {
    AuthError::invalid_bearer("the token is not for this API (issuer or audience)")
}

/// `iss` and `aud` of a token whose signature and times have passed, in that order. `iss` is one text, equal to
/// the configured issuer (RFC 7519 4.1.1: a StringOrURI): a list that holds it is not it, and a token that names
/// another issuer beside ours is not ours. `aud` is a text, or a list of texts, and one of the configured
/// audiences is among them.
fn check_issuer_and_audience(
    claims: &Map<String, Value>,
    issuer: &str,
    audiences: &[String],
) -> Result<(), AuthError> {
    match claims.get("iss") {
        None => return Err(missing_claim("iss")),
        Some(Value::String(iss)) if iss == issuer => {}
        Some(_) => return Err(not_for_this_api()),
    }
    let ours = |aud: &str| audiences.iter().any(|ours| ours == aud);
    let named = match claims.get("aud") {
        None => return Err(missing_claim("aud")),
        Some(Value::String(aud)) => ours(aud),
        Some(Value::Array(auds)) => {
            auds.iter().all(Value::is_string) && auds.iter().filter_map(Value::as_str).any(ours)
        }
        Some(_) => false,
    };
    if named {
        Ok(())
    } else {
        Err(not_for_this_api())
    }
}

/// Makes the process-level crypto provider of `jsonwebtoken` this crate's, aws-lc-rs. The library chooses it from
/// its features when exactly one backend is enabled, and **panics on the first verification when two are**, as
/// they are in a binary that links another crate with the `rust_crypto` backend (Cargo unifies the features of
/// the library across the whole build). Idempotent: the provider can be installed once per process, and one that
/// is already there (ours, or one the binary chose) is left alone. Called before any key is read or token
/// checked: the first use fixes the choice.
fn install_crypto_provider() {
    let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
}

impl Inner {
    /// The claims of `token`, once its header, key, signature and claims have passed; the same for a
    /// bearer and for a DPoP token. A refusal is [`CredentialKind::Bearer`]'s.
    async fn verify(self: &Arc<Self>, token: &str) -> Result<Map<String, Value>, AuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::invalid_bearer("the token is malformed"));
        }
        // The header says which key to use, and is read before anything is trusted. A token that
        // is not three base64url parts of JSON, and `alg: none` (which is no algorithm of ours),
        // end here: no fetch, no key.
        let header = decode_header(token)
            .map_err(|_| AuthError::invalid_bearer("the token is malformed"))?;
        if !ALLOWED.contains(&header.alg) {
            return Err(AuthError::invalid_bearer(
                "the token's algorithm is not allowed",
            ));
        }
        let candidates = |keys: &KeySet| -> Vec<DecodingKey> {
            keys.select(header.alg, header.kid.as_deref())
                .into_iter()
                .map(|key| key.decoding.clone())
                .collect()
        };
        let mut keys = self.current().await?;
        let mut decoding = candidates(&keys);
        if decoding.is_empty() {
            keys = self.after_unknown_kid(&keys).await;
            decoding = candidates(&keys);
            if decoding.is_empty() {
                return Err(AuthError::invalid_bearer(
                    "the token's signing key is not the issuer's",
                ));
            }
        }
        debug_assert!(kind_of(header.alg).is_some());
        let claims = self.decode_with_any(token, header.alg, &decoding)?.claims;
        check_issuer_and_audience(&claims, &self.issuer, &self.audiences)?;
        // `iat` is required, though the library checks no time against it.
        if !claims.get("iat").is_some_and(Value::is_number) {
            return Err(AuthError::invalid_bearer("the token has no \"iat\" claim"));
        }
        Ok(claims)
    }
}

/// A refusal of the token of a DPoP request is the DPoP one (`error="invalid_token"`); anything
/// else (the issuer's keys are down) is what it was.
fn as_dpop_token(error: AuthError) -> AuthError {
    match error {
        AuthError::Invalid {
            credential: CredentialKind::Bearer,
            detail,
        } => AuthError::invalid_dpop_token(detail),
        other => other,
    }
}

/// The thumbprint a token is bound to (`cnf.jkt`), when it has one.
fn bound_to(claims: &Map<String, Value>) -> Option<&str> {
    claims.get("cnf")?.get("jkt")?.as_str()
}

impl JwtAuth {
    /// A request with `Authorization: DPoP` (RFC 9449 §7): the proof, the token, and that the token is
    /// bound to the proof's key.
    async fn authenticate_dpop(
        &self,
        request: &DpopCredentials<'_>,
    ) -> Result<Principal, AuthError> {
        let inner = &self.inner;
        let Some(dpop) = &inner.dpop else {
            // Fail closed: a deployment that did not configure DPoP does not read it.
            return Err(AuthError::invalid_bearer("the DPoP scheme is not accepted"));
        };
        if request.token.is_empty() || request.token.len() > MAX_TOKEN_BYTES {
            return Err(AuthError::invalid_dpop_token("the token is malformed"));
        }
        let proven = dpop.check_proof(request, orch_core::Timestamp::now().as_second())?;
        let claims = inner.verify(request.token).await.map_err(as_dpop_token)?;
        match bound_to(&claims) {
            None => {
                return Err(AuthError::invalid_dpop_token(
                    "the token is not bound to a key (no cnf.jkt)",
                ));
            }
            Some(jkt) if jkt != proven.jkt => {
                return Err(AuthError::invalid_dpop_token(
                    "the token is bound to another key than the proof's",
                ));
            }
            Some(_) => {}
        }
        // Last: a proof is only remembered when everything else holds, so a caller with no valid
        // token cannot fill the cache.
        dpop.admit(&proven, Instant::now())?;
        claims::principal(&claims, &inner.user_claim, inner.roles_claim.as_deref())
    }
}

impl Authenticator for JwtAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        if let Some(request) = &credentials.dpop {
            return self.authenticate_dpop(request).await;
        }
        let Some(token) = credentials.bearer else {
            return Err(AuthError::Missing);
        };
        let inner = &self.inner;
        let claims = inner.verify(token).await?;
        // RFC 9449 §7.1: a token bound to a key is not a bearer token. Checked only where DPoP is
        // configured, so that a deployment without it is exactly what it was.
        if inner.dpop.is_some() && claims.contains_key("cnf") {
            return Err(AuthError::invalid_dpop_token(
                "a token bound to a key must be sent with the DPoP scheme",
            ));
        }
        claims::principal(&claims, &inner.user_claim, inner.roles_claim.as_deref())
    }

    async fn ready(&self) -> Result<(), AuthError> {
        self.inner.current().await.map(|_| ())
    }

    fn accepts_bearer(&self) -> bool {
        true
    }

    fn accepts_dpop(&self) -> bool {
        self.inner.dpop.is_some()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    const ISSUER: &str = "https://idp.example/realms/x";

    fn claims(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn audiences() -> Vec<String> {
        vec!["web".to_owned(), "cli".to_owned()]
    }

    /// The reason a token is refused for, or `None` when it passes.
    fn reason(value: Value) -> Option<String> {
        match check_issuer_and_audience(&claims(value), ISSUER, &audiences()) {
            Ok(()) => None,
            Err(AuthError::Invalid { detail, .. }) => Some(detail),
            Err(other) => panic!("{other:?}"),
        }
    }

    #[test]
    fn iss_is_one_text_equal_to_the_issuer() {
        assert_eq!(reason(json!({"iss": ISSUER, "aud": "web"})), None);
        for wrong in [
            json!("https://idp.example/realms/x/"),
            json!("HTTPS://IDP.EXAMPLE/realms/x"),
            json!(""),
            json!(7),
            json!(null),
            // The library lets a list through when it holds the issuer.
            json!([ISSUER]),
            json!([ISSUER, "https://evil.example"]),
            json!(["https://evil.example", ISSUER]),
            json!([]),
        ] {
            let why = reason(json!({"iss": wrong, "aud": "web"}));
            assert!(why.unwrap().contains("not for this API"), "iss {wrong}");
        }
    }

    #[test]
    fn one_of_the_audiences_is_among_aud() {
        for fine in [
            json!("web"),
            json!("cli"),
            json!(["x", "cli"]),
            json!(["web"]),
        ] {
            assert_eq!(reason(json!({"iss": ISSUER, "aud": fine})), None, "{fine}");
        }
        for wrong in [
            json!("x"),
            json!(""),
            json!([]),
            json!(["x", "y"]),
            json!(["web", 5]),
            json!(5),
            json!(null),
            json!({"web": true}),
        ] {
            let why = reason(json!({"iss": ISSUER, "aud": wrong}));
            assert!(why.unwrap().contains("not for this API"), "aud {wrong}");
        }
    }

    #[test]
    fn a_claim_that_is_absent_is_named_and_the_issuer_comes_first() {
        let why = |value| reason(value).unwrap();
        assert!(why(json!({"aud": "web"})).contains("\"iss\""));
        assert!(why(json!({"iss": ISSUER})).contains("\"aud\""));
        assert!(
            why(json!({})).contains("\"iss\""),
            "the issuer is the first asked"
        );
        // Wrong in both: the word is the same, and the issuer's is the one given.
        assert!(why(json!({"iss": "x", "aud": "y"})).contains("not for this API"));
        assert!(why(json!({"iss": "x"})).contains("not for this API"));
    }
}
