//! [`PlatformRegistry`]: the [`AgentRegistry`] over the platform's `agent-registry/v1`, read over
//! HTTP.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use orch_core::{AgentId, AgentSource, BoxError, report};
use orch_ports::{
    AgentEndpoint, AgentListing, AgentRegistry, RegistryEntry, RegistryError, SourceStatus,
};
use reqwest::StatusCode;
use reqwest::header::{
    ACCEPT, AGE, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HeaderMap,
    HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED,
};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use crate::freshness::{self, CacheHeaders, Policy, Validator};
use crate::linkset::{self, Document, MAX_BODY_BYTES, Skipped};

/// The name of this source in [`SourceStatus`] and in [`RegistryError`].
pub const SOURCE: &str = "platform";

/// What a person is told when the registry cannot be reached.
const UNREACHABLE: &str = "the registry could not be reached";
/// What a person is told when it refused the orchestrator's credentials.
const REFUSED: &str = "the registry refused the orchestrator's credentials";
/// What a person is told when it answered with a document this build cannot use.
const UNREADABLE: &str = "a registry document this build cannot read";

/// The media type of the registry document (the `profile` parameter is not read: the contract
/// version is the document's own `profile` link).
const MEDIA_TYPE: &str = "application/linkset+json";

/// How to reach the registry.
#[derive(Clone)]
pub struct PlatformConfig {
    /// The registry's URL, in full (`https://platform.example.com/registry/v1/agents`): the
    /// client never derives it. `http` or `https`, with a host and no user name or password.
    pub url: String,
    /// The bearer token for the registry, when it wants one.
    pub token: Option<SecretString>,
    /// The bearer token sent to **every agent the registry lists**: each entry's endpoint carries
    /// it (one deployment-wide credential until open question 11 is answered).
    pub agent_token: Option<SecretString>,
    /// How long one read of the registry may take, connecting and answering together.
    pub timeout: Duration,
    /// The longest a copy of the document is kept fresh, whatever the registry allows.
    pub max_age: Duration,
    /// Whether to honour `HTTP(S)_PROXY` from the environment, as the A2A client does.
    pub use_system_proxy: bool,
}

impl PlatformConfig {
    /// A configuration for `url`: no credentials, a 3 second timeout, copies kept fresh for at
    /// most 60 seconds, the environment's proxy honoured.
    pub fn new(url: impl Into<String>) -> Self {
        PlatformConfig {
            url: url.into(),
            token: None,
            agent_token: None,
            timeout: Duration::from_secs(3),
            max_age: Duration::from_secs(60),
            use_system_proxy: true,
        }
    }

    /// Sets the bearer token for the registry.
    #[must_use]
    pub fn with_token(mut self, token: SecretString) -> Self {
        self.token = Some(token);
        self
    }

    /// Sets the bearer token every listed agent is sent.
    #[must_use]
    pub fn with_agent_token(mut self, token: SecretString) -> Self {
        self.agent_token = Some(token);
        self
    }

    /// Sets how long one read may take.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the longest a copy stays fresh.
    #[must_use]
    pub fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Reaches the registry directly, whatever the environment's proxy says.
    #[must_use]
    pub fn without_system_proxy(mut self) -> Self {
        self.use_system_proxy = false;
        self
    }
}

impl fmt::Debug for PlatformConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformConfig")
            .field("url", &display_url(&self.url))
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field(
                "agent_token",
                &self.agent_token.as_ref().map(|_| "<redacted>"),
            )
            .field("timeout", &self.timeout)
            .field("max_age", &self.max_age)
            .field("use_system_proxy", &self.use_system_proxy)
            .finish()
    }
}

/// The URL as it may be shown: without a query (which may carry a secret) or credentials.
fn display_url(raw: &str) -> String {
    match Url::parse(raw.trim()) {
        Ok(url) => {
            let mut shown = format!("{}://", url.scheme());
            shown.push_str(url.host_str().unwrap_or(""));
            if let Some(port) = url.port() {
                shown.push_str(&format!(":{port}"));
            }
            shown.push_str(url.path());
            shown
        }
        Err(_) => "<not a URL>".to_owned(),
    }
}

/// The registry client could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The URL is not an absolute `http(s)` URL with a host and no user name or password.
    #[error(
        "the registry URL must be an absolute http:// or https:// URL with a host and no user name or password"
    )]
    BadUrl,
    /// A token cannot be sent in a header (a newline in it).
    #[error("the {0} token cannot be sent as a header")]
    BadToken(&'static str),
    /// The HTTP client could not be built (TLS backend initialisation).
    #[error("cannot build the registry HTTP client")]
    Http(#[source] BoxError),
}

/// Why a read failed: what a person is told, and what an operator is.
#[derive(Debug)]
struct Failure {
    detail: &'static str,
    cause: String,
}

impl Failure {
    fn new(detail: &'static str, cause: impl Into<String>) -> Self {
        Failure {
            detail,
            cause: cause.into(),
        }
    }

    /// A transport failure, without the URL (which may carry a secret in its query).
    fn transport(error: reqwest::Error) -> Self {
        let cause = report(&error.without_url());
        Failure::new(UNREACHABLE, cause)
    }
}

type Agents = Arc<[RegistryEntry]>;
type Outcome = Result<Agents, Arc<Failure>>;

/// The copy of the last document, and what asks the registry whether it is still good.
struct Copy {
    agents: Agents,
    fresh_until: Instant,
    lifetime: Duration,
    validator: Option<Validator>,
}

#[derive(Default)]
struct State {
    copy: Option<Copy>,
    /// The outcome of the latest fetch, which readers that waited for it share.
    last: Option<Outcome>,
    /// The skipped items already reported, so that a registry read on every request does not
    /// repeat the warning for the same ones.
    reported_skips: Vec<Skipped>,
}

struct Inner {
    http: reqwest::Client,
    url: Url,
    authorization: Option<HeaderValue>,
    agent_token: Option<String>,
    max_age: Duration,
    state: Mutex<State>,
    /// Bumped by every fetch that finishes, so a reader that waited for the lock knows that a
    /// fetch happened while it waited and shares its outcome (single flight).
    generation: AtomicU64,
}

/// The platform's agent registry, read live (ADR 0022).
///
/// A document is read over HTTP at most as often as its cache headers allow (`max-age` less
/// `Age`, at most [`PlatformConfig::max_age`]), asked again with the `ETag` or `Last-Modified` it
/// came with, and held **in this process only**. Concurrent readers share one fetch. A read that
/// fails (no connection, a timeout, a status other than 200 or 304, an unreadable document) makes
/// the whole source unavailable and **drops the copy**: a registry that is down lists no agents,
/// never the ones it listed before.
///
/// Cheap to clone (the state is shared).
#[derive(Clone)]
pub struct PlatformRegistry {
    inner: Arc<Inner>,
}

impl fmt::Debug for PlatformRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformRegistry")
            .field("url", &display_url(self.inner.url.as_str()))
            .field(
                "authorization",
                &self.inner.authorization.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "agent_token",
                &self.inner.agent_token.as_ref().map(|_| "<redacted>"),
            )
            .field("max_age", &self.inner.max_age)
            .finish_non_exhaustive()
    }
}

/// A bearer header value, marked sensitive. An empty token is none.
fn bearer(
    token: Option<&SecretString>,
    which: &'static str,
) -> Result<Option<HeaderValue>, BuildError> {
    match token {
        Some(token) if !token.expose_secret().is_empty() => {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| BuildError::BadToken(which))?;
            value.set_sensitive(true);
            Ok(Some(value))
        }
        Some(_) | None => Ok(None),
    }
}

impl PlatformRegistry {
    /// Builds the client and installs the `rustls` crypto provider if none is installed yet. It
    /// reads nothing: the first read is the first `list` or `get`.
    ///
    /// # Errors
    /// [`BuildError`] for a URL that is not an absolute `http(s)` URL with a host and no
    /// credentials in it, a token that is not a header value, or a TLS backend that fails to
    /// start.
    pub fn new(cfg: PlatformConfig) -> Result<Self, BuildError> {
        let url = Url::parse(cfg.url.trim()).map_err(|_| BuildError::BadUrl)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.has_host()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(BuildError::BadUrl);
        }
        let authorization = bearer(cfg.token.as_ref(), "registry")?;
        // The agent token is sent by the A2A client, as a header too: refuse here what it could
        // not send, not on the first message.
        bearer(cfg.agent_token.as_ref(), "agent")?;
        let agent_token = cfg
            .agent_token
            .as_ref()
            .map(|t| t.expose_secret().to_owned())
            .filter(|t| !t.is_empty());
        // Err means a provider is already installed, which is what we want.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let builder = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .connect_timeout(cfg.timeout)
            // The token goes to the configured URL and nowhere else.
            .redirect(reqwest::redirect::Policy::none());
        let builder = if cfg.use_system_proxy {
            builder
        } else {
            builder.no_proxy()
        };
        let http = builder
            .build()
            .map_err(|e| BuildError::Http(e.without_url().into()))?;
        Ok(PlatformRegistry {
            inner: Arc::new(Inner {
                http,
                url,
                authorization,
                agent_token,
                max_age: cfg.max_age,
                state: Mutex::new(State::default()),
                generation: AtomicU64::new(0),
            }),
        })
    }

    /// The agents of the registry now: from the copy while it is fresh, else from the registry.
    async fn read(&self) -> Outcome {
        let inner = &self.inner;
        let seen = inner.generation.load(Ordering::Acquire);
        let mut state = inner.state.lock().await;
        // A fetch finished while this reader waited for the lock: it shares that outcome rather
        // than asking again (and, if the registry is down, waiting for its own timeout).
        if inner.generation.load(Ordering::Acquire) != seen
            && let Some(outcome) = &state.last
        {
            return outcome.clone();
        }
        if let Some(copy) = &state.copy
            && Instant::now() < copy.fresh_until
        {
            return Ok(Arc::clone(&copy.agents));
        }
        let outcome = inner.fetch(&mut state).await;
        state.last = Some(outcome.clone());
        inner.generation.fetch_add(1, Ordering::Release);
        outcome
    }
}

/// What one request to the registry came to.
enum Fetched {
    /// `304`: the copy is still good, and the headers it came with.
    NotModified(Option<Policy>),
    /// `200` with a document that reads.
    Document {
        agents: Agents,
        policy: Policy,
        skipped: Vec<Skipped>,
    },
}

impl Inner {
    /// Asks the registry (conditionally when there is a copy to ask about) and records what it
    /// said in `state`: a new copy, a renewed one, or none at all.
    async fn fetch(&self, state: &mut State) -> Outcome {
        let was_failing = matches!(state.last, Some(Err(_)));
        let requested_at = Instant::now();
        match self.request(state.copy.as_ref()).await {
            Ok(Fetched::NotModified(policy)) => {
                let Some(copy) = state.copy.as_mut() else {
                    // `request` answers a 304 only to a request that asked about a copy.
                    return Err(Arc::new(Failure::new(UNREACHABLE, "unexpected 304")));
                };
                if let Some(policy) = policy {
                    copy.lifetime = policy.lifetime;
                    if policy.validator.is_some() {
                        copy.validator = policy.validator;
                    }
                }
                copy.fresh_until = requested_at + copy.lifetime;
                tracing::debug!("the agent registry is unchanged (304)");
                Ok(Arc::clone(&copy.agents))
            }
            Ok(Fetched::Document {
                agents,
                policy,
                skipped,
            }) => {
                if skipped != state.reported_skips {
                    if !skipped.is_empty() {
                        tracing::warn!(
                            skipped = ?skipped
                                .iter()
                                .map(|s| format!("item {}: {}", s.index, s.reason))
                                .collect::<Vec<_>>(),
                            "the agent registry lists items this build skipped"
                        );
                    }
                    state.reported_skips = skipped;
                }
                if was_failing {
                    tracing::info!("the agent registry can be read again");
                }
                state.copy = Some(Copy {
                    agents: Arc::clone(&agents),
                    fresh_until: requested_at + policy.lifetime,
                    lifetime: policy.lifetime,
                    validator: policy.validator,
                });
                Ok(agents)
            }
            Err(failure) => {
                // Fail closed: nothing of the old copy is served, and the next read asks again
                // with no validator.
                state.copy = None;
                tracing::warn!(
                    cause = %failure.cause,
                    "the agent registry could not be read; its agents are not listed: {}",
                    failure.detail
                );
                Err(Arc::new(failure))
            }
        }
    }

    async fn request(&self, copy: Option<&Copy>) -> Result<Fetched, Failure> {
        let mut call = self
            .http
            .get(self.url.clone())
            .header(ACCEPT, HeaderValue::from_static(MEDIA_TYPE));
        if let Some(authorization) = &self.authorization {
            call = call.header(AUTHORIZATION, authorization.clone());
        }
        if let Some(copy) = copy {
            match &copy.validator {
                Some(Validator::ETag(etag)) => {
                    if let Ok(value) = HeaderValue::from_str(etag) {
                        call = call.header(IF_NONE_MATCH, value);
                    }
                }
                Some(Validator::LastModified(date)) => {
                    if let Ok(value) = HeaderValue::from_str(date) {
                        call = call.header(IF_MODIFIED_SINCE, value);
                    }
                }
                None => {}
            }
        }
        let mut response = call.send().await.map_err(Failure::transport)?;
        let status = response.status();
        match status {
            StatusCode::NOT_MODIFIED if copy.is_some() => {
                let policy = states_freshness(response.headers())
                    .then(|| policy_of(response.headers(), self.max_age));
                return Ok(Fetched::NotModified(policy));
            }
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(Failure::new(
                    REFUSED,
                    format!("the registry answered {status}"),
                ));
            }
            other => {
                return Err(Failure::new(
                    UNREACHABLE,
                    format!("the registry answered {other}"),
                ));
            }
        }
        if !is_linkset_json(response.headers()) {
            return Err(Failure::new(
                UNREADABLE,
                "the registry did not answer application/linkset+json",
            ));
        }
        let declared = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<usize>().ok());
        if declared.is_some_and(|n| n > MAX_BODY_BYTES) {
            return Err(Failure::new(UNREADABLE, "the document is too large"));
        }
        let policy = policy_of(response.headers(), self.max_age);
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Failure::transport)? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(Failure::new(UNREADABLE, "the document is too large"));
            }
            body.extend_from_slice(&chunk);
        }
        let Document { items, skipped } =
            linkset::parse(&body).map_err(|e| Failure::new(UNREADABLE, e.to_string()))?;
        let agents = items
            .into_iter()
            .map(|item| RegistryEntry {
                endpoint: AgentEndpoint::a2a(
                    AgentId::new(item.service),
                    item.href,
                    self.agent_token.clone(),
                ),
                name: item.title,
                tags: item.tags,
                origin: AgentSource::Registry,
            })
            .collect();
        Ok(Fetched::Document {
            agents,
            policy,
            skipped,
        })
    }
}

/// The header text of every field named `name`, joined by commas.
fn joined(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    let values: Vec<&str> = headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

fn states_freshness(headers: &HeaderMap) -> bool {
    let cache_control = joined(headers, CACHE_CONTROL);
    freshness::states_freshness(&CacheHeaders {
        cache_control: cache_control.as_deref(),
        ..CacheHeaders::default()
    })
}

fn policy_of(headers: &HeaderMap, cap: Duration) -> Policy {
    let cache_control = joined(headers, CACHE_CONTROL);
    let age = joined(headers, AGE);
    let etag = joined(headers, ETAG);
    let last_modified = joined(headers, LAST_MODIFIED);
    freshness::policy(
        &CacheHeaders {
            cache_control: cache_control.as_deref(),
            age: age.as_deref(),
            etag: etag.as_deref(),
            last_modified: last_modified.as_deref(),
        },
        cap,
    )
}

/// Whether `Content-Type` is `application/linkset+json`, whatever its parameters (the version of
/// the contract is the document's own `profile` link, not the media type's).
fn is_linkset_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case(MEDIA_TYPE))
}

impl AgentRegistry for PlatformRegistry {
    async fn list(&self) -> AgentListing {
        match self.read().await {
            Ok(agents) => AgentListing {
                entries: agents.to_vec(),
                sources: vec![SourceStatus::ok(SOURCE)],
            },
            Err(failure) => AgentListing {
                entries: Vec::new(),
                sources: vec![SourceStatus::unavailable(SOURCE, failure.detail)],
            },
        }
    }

    async fn get(&self, id: &AgentId) -> Result<Option<RegistryEntry>, RegistryError> {
        match self.read().await {
            Ok(agents) => Ok(agents.iter().find(|e| e.id() == id).cloned()),
            Err(failure) => Err(RegistryError::unavailable(SOURCE, failure.detail)
                .with_source(failure.cause.clone())),
        }
    }
}
