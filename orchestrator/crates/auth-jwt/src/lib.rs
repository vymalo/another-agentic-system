//! [`Authenticator`] over OAuth2 bearer tokens: a JWT signed by the issuer, validated here against
//! the issuer's keys (ADR 0033, `auth.mode: jwt`). The orchestrator is a resource server: it
//! trusts no header a client can send, only a token it can verify.
//!
//! ```mermaid
//! sequenceDiagram
//!   participant C as Client (the web through oauth2-proxy, a CLI, CI)
//!   participant A as JwtAuth
//!   participant I as Issuer (discovery, JWKS)
//!   C->>A: Authorization: Bearer token
//!   A->>A: header: alg allowed? kid?
//!   alt keys held and fresh
//!     A->>A: pick the key of the kid
//!   else never fetched, or older than 10 minutes
//!     A->>I: GET discovery, GET jwks
//!     I-->>A: keys
//!   else the kid is unknown (at most once in 30 s)
//!     A->>I: GET jwks again
//!   end
//!   A->>A: signature, iss, aud, exp, nbf, iat, email_verified
//!   A-->>C: the principal, or Invalid, or Unavailable
//! ```
//!
//! # What is checked
//!
//! - **Algorithm**: RS256, RS384, ES256 and EdDSA, and nothing else. `none` and the HMAC family are
//!   refused whatever key they name (the algorithm confusion attack), and the key a token is
//!   verified with must be of the algorithm's family.
//! - **Key**: a key of the issuer's JWKS, found by the token's `kid`. A JWKS key that cannot verify
//!   (symmetric, too small, for encryption, another curve) is skipped.
//! - **Claims**: `iss` equal to the configured issuer, exactly; the configured audience among
//!   `aud`; `exp`, `iat`, `iss` and `aud` present; `nbf` honoured; 60 seconds of leeway;
//!   `email_verified` not `false`; the configured user claim a non-empty text.
//! - **Size**: a token of more than 16 KiB is refused unread.
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
//! The keys are read from `jwksUrl`, or from the `jwks_uri` of
//! `{issuer}/.well-known/openid-configuration`, whose `issuer` must be the configured one. No
//! redirect is followed, a body over 1 MiB is refused, and a request times out.

mod claims;
mod fetch;
mod keys;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Validation, decode, decode_header};
use orch_ports::{AuthError, Authenticator, Credentials, Principal};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use url::Url;

use crate::fetch::{Fetcher, Source};
use crate::keys::{ALLOWED, KeySet, kind_of};

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
        }
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

/// The copy of the keys, and what is known about fetching them.
#[derive(Default)]
struct State {
    /// The keys of the last successful fetch, and when it finished.
    keys: Option<(Arc<KeySet>, Instant)>,
    /// When the last fetch, successful or not, started.
    last_attempt: Option<Instant>,
    /// When the last fetch failed, until one succeeds.
    last_failure: Option<Instant>,
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
        let source = match &cfg.jwks_url {
            Some(url) => Source::Url(http_url(url).ok_or(BuildError::BadJwksUrl)?),
            None => {
                let base = issuer_url.as_str().trim_end_matches('/');
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
            }),
        })
    }
}

fn unavailable() -> AuthError {
    AuthError::unavailable("the token issuer's keys cannot be fetched")
}

impl Inner {
    /// Fetches the keys, under the lock, and records what came of it.
    async fn fetch(&self, state: &mut State) {
        let started = Instant::now();
        state.last_attempt = Some(started);
        let fetched = match self.fetcher.jwks().await {
            Ok(document) => match KeySet::parse(&document) {
                Some(parsed) if !parsed.set.is_empty() => Ok(parsed),
                Some(_) => Err("the key set has no key this build can verify with".to_owned()),
                None => Err("the answer is not a key set".to_owned()),
            },
            Err(error) => Err(error.to_string()),
        };
        match fetched {
            Ok(parsed) => {
                if state.last_failure.take().is_some() {
                    tracing::info!("the token issuer's keys can be fetched again");
                }
                if parsed.skipped > 0 {
                    tracing::debug!(
                        skipped = parsed.skipped,
                        "keys of the issuer this build cannot use"
                    );
                }
                state.keys = Some((Arc::new(parsed.set), started));
            }
            Err(cause) => {
                if state.last_failure.is_none() {
                    tracing::warn!(%cause, "the token issuer's keys cannot be fetched; tokens are refused until they can");
                } else {
                    tracing::debug!(%cause, "the token issuer's keys still cannot be fetched");
                }
                state.last_failure = Some(Instant::now());
            }
        }
    }

    /// The copy, if it may still verify: no older than the refresh interval and the grace.
    fn usable(&self, state: &State) -> Option<Arc<KeySet>> {
        let (keys, at) = state.keys.as_ref()?;
        (at.elapsed() <= self.refresh_interval + self.stale_grace).then(|| Arc::clone(keys))
    }

    /// The keys to verify with: the copy while it is fresh, else a fetch (not sooner than the
    /// retry interval after a failure), else the stale copy within its grace.
    async fn current(&self) -> Result<Arc<KeySet>, AuthError> {
        let mut state = self.state.lock().await;
        if let Some((keys, at)) = &state.keys
            && at.elapsed() < self.refresh_interval
        {
            return Ok(Arc::clone(keys));
        }
        let waiting = state
            .last_failure
            .is_some_and(|failed| failed.elapsed() < self.retry_interval);
        if !waiting {
            self.fetch(&mut state).await;
        }
        self.usable(&state).ok_or_else(unavailable)
    }

    /// The keys after a token named a `kid` that `seen` lacks: fetched again, unless a fetch was
    /// tried within the interval, or someone else already did. Never fails: without a better
    /// copy, `seen` is it.
    async fn after_unknown_kid(&self, seen: &Arc<KeySet>) -> Arc<KeySet> {
        let mut state = self.state.lock().await;
        if let Some((keys, _)) = &state.keys
            && !Arc::ptr_eq(keys, seen)
        {
            return Arc::clone(keys);
        }
        let recent = state
            .last_attempt
            .is_some_and(|at| at.elapsed() < self.kid_refetch_interval);
        if !recent {
            self.fetch(&mut state).await;
        }
        self.usable(&state).unwrap_or_else(|| Arc::clone(seen))
    }

    fn validation(&self, alg: jsonwebtoken::Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = true;
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation
    }
}

/// Why a token was refused, in words fit for the caller. Never a value of the token.
fn refusal(kind: &ErrorKind) -> AuthError {
    let detail = match kind {
        ErrorKind::ExpiredSignature => "the token has expired".to_owned(),
        ErrorKind::ImmatureSignature => "the token is not valid yet".to_owned(),
        ErrorKind::InvalidIssuer | ErrorKind::InvalidAudience => {
            "the token is not for this API (issuer or audience)".to_owned()
        }
        ErrorKind::InvalidSignature => "the token's signature does not verify".to_owned(),
        ErrorKind::MissingRequiredClaim(claim) => format!("the token has no {claim:?} claim"),
        _ => "the token is not valid".to_owned(),
    };
    AuthError::invalid_bearer(detail)
}

impl Authenticator for JwtAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        let Some(token) = credentials.bearer else {
            return Err(AuthError::Missing);
        };
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
        let inner = &self.inner;
        let mut keys = inner.current().await?;
        let key = match keys.select(header.alg, header.kid.as_deref()) {
            Some(key) => key.decoding.clone(),
            None => {
                keys = inner.after_unknown_kid(&keys).await;
                match keys.select(header.alg, header.kid.as_deref()) {
                    Some(key) => key.decoding.clone(),
                    None => {
                        return Err(AuthError::invalid_bearer(
                            "the token's signing key is not the issuer's",
                        ));
                    }
                }
            }
        };
        debug_assert!(kind_of(header.alg).is_some());
        let data = decode::<Map<String, Value>>(token, &key, &inner.validation(header.alg))
            .map_err(|e| refusal(e.kind()))?;
        let claims = data.claims;
        // `iat` is required, though the library checks no time against it.
        if !claims.get("iat").is_some_and(Value::is_number) {
            return Err(AuthError::invalid_bearer("the token has no \"iat\" claim"));
        }
        claims::principal(&claims, &inner.user_claim, inner.roles_claim.as_deref())
    }

    async fn ready(&self) -> Result<(), AuthError> {
        self.inner.current().await.map(|_| ())
    }

    fn accepts_bearer(&self) -> bool {
        true
    }
}
