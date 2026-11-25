//! [`PlatformProvisioner`]: the [`AgentProvisioner`] over the Platform API's owner routes
//! (`/v1/owners/{owner}/agents/{name}`, permission `platform:agents.own`).
//!
//! The contract is the platform's OpenAPI document,
//! `docs/api/platform-api-v1.openapi.json` in another-agentic-platform. Nothing of the platform's
//! crates is a dependency (ADR 0007): the adapter speaks HTTP and JSON.
//!
//! * A **create** is `PUT` with `If-None-Match: *`: of two first messages at once the platform
//!   makes one agent and answers the other `409` `already-exists`. The other `409`s
//!   (`managed-by-gitops`, `deleting`, `conflict`) are told apart by the problem's `type` and are
//!   not "made".
//! * A **resume** is `PUT` with `If-Match` and `suspend: false`; a `412` re-reads and decides again.
//! * **Ready** is the agent's `state`, polled until [`PlatformConfig::ready_timeout`].
//! * The form of each kind is the deployment's ([`PlatformConfig::with_form`]): the adapter adds
//!   `name`, `owner` and `suspend` and nothing else, so it holds no model, image or secret.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use orch_core::BoxError;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
};
use reqwest::header::{AUTHORIZATION, ETAG, HeaderValue, IF_MATCH, IF_NONE_MATCH};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tokio::time::Instant;
use url::Url;

/// Times a write is tried again after the platform says the agent changed meanwhile.
const MAX_RACES: u32 = 8;

/// How to reach the Platform API.
#[derive(Clone)]
pub struct PlatformConfig {
    /// The API's base URL (`https://platform.example.com`), `http` or `https`, with a host and no
    /// user name or password. `/v1/...` is appended.
    pub url: String,
    /// The orchestrator's bearer token (it holds `platform:agents.own`).
    pub token: SecretString,
    /// The agent form of each kind, without `name`, `owner` and `suspend`.
    pub forms: HashMap<AgentKind, Value>,
    /// How long one request may take.
    pub timeout: Duration,
    /// How long [`ensure_ready`](AgentProvisioner::ensure_ready) waits for `ready`.
    pub ready_timeout: Duration,
    /// How often the state is read while waiting.
    pub poll_interval: Duration,
    /// Whether to honour `HTTP(S)_PROXY` from the environment.
    pub use_system_proxy: bool,
}

impl PlatformConfig {
    /// A configuration for `url` with `token`: no forms, 5 s per request, 120 s to be ready,
    /// polled every 2 s, the environment's proxy honoured.
    pub fn new(url: impl Into<String>, token: SecretString) -> Self {
        PlatformConfig {
            url: url.into(),
            token,
            forms: HashMap::new(),
            timeout: Duration::from_secs(5),
            ready_timeout: Duration::from_secs(120),
            poll_interval: Duration::from_secs(2),
            use_system_proxy: true,
        }
    }

    /// Sets the form of `kind` (a JSON object).
    #[must_use]
    pub fn with_form(mut self, kind: AgentKind, form: Value) -> Self {
        self.forms.insert(kind, form);
        self
    }

    /// Sets how long to wait for `ready`.
    #[must_use]
    pub fn with_ready_timeout(mut self, timeout: Duration) -> Self {
        self.ready_timeout = timeout;
        self
    }

    /// Sets how often the state is read while waiting.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Reaches the API directly, whatever the environment's proxy says.
    #[must_use]
    pub fn without_system_proxy(mut self) -> Self {
        self.use_system_proxy = false;
        self
    }
}

impl fmt::Debug for PlatformConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformConfig")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("kinds", &self.forms.keys().collect::<Vec<_>>())
            .field("ready_timeout", &self.ready_timeout)
            .finish_non_exhaustive()
    }
}

/// The provisioner could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The URL is not an absolute `http(s)` URL with a host and no user name or password.
    #[error(
        "the platform URL must be an absolute http:// or https:// URL with a host and no user name or password"
    )]
    BadUrl,
    /// The token cannot be sent in a header.
    #[error("the platform token cannot be sent as a header")]
    BadToken,
    /// A form is not a JSON object, or sets a field the adapter owns.
    #[error("the form of {0:?} must be a JSON object without name, owner or suspend")]
    BadForm(AgentKind),
    /// The HTTP client could not be built.
    #[error("cannot build the platform HTTP client")]
    Http(#[source] BoxError),
}

/// What a read of one agent said.
struct Seen {
    state: AgentState,
    etag: Option<HeaderValue>,
}

/// The [`AgentProvisioner`] over the Platform API.
#[derive(Clone)]
pub struct PlatformProvisioner {
    http: reqwest::Client,
    base: Url,
    authorization: HeaderValue,
    forms: HashMap<AgentKind, Value>,
    ready_timeout: Duration,
    poll_interval: Duration,
}

impl fmt::Debug for PlatformProvisioner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformProvisioner")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl PlatformProvisioner {
    /// Builds the provisioner, checking the URL, the token and the forms.
    pub fn new(cfg: PlatformConfig) -> Result<Self, BuildError> {
        let base = Url::parse(cfg.url.trim()).map_err(|_| BuildError::BadUrl)?;
        let ok = matches!(base.scheme(), "http" | "https")
            && base.host_str().is_some()
            && base.username().is_empty()
            && base.password().is_none();
        if !ok {
            return Err(BuildError::BadUrl);
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {}", cfg.token.expose_secret()))
                .map_err(|_| BuildError::BadToken)?;
        authorization.set_sensitive(true);
        for (kind, form) in &cfg.forms {
            let object = form.as_object().ok_or(BuildError::BadForm(*kind))?;
            if ["name", "owner", "suspend"]
                .iter()
                .any(|k| object.contains_key(*k))
            {
                return Err(BuildError::BadForm(*kind));
            }
        }
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
        Ok(PlatformProvisioner {
            http,
            base,
            authorization,
            forms: cfg.forms,
            ready_timeout: cfg.ready_timeout,
            poll_interval: cfg.poll_interval,
        })
    }

    fn url(&self, owner: &AgentOwner, name: &str) -> Result<Url, ProvisionError> {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .map_err(|()| ProvisionError::Invalid)?
            .pop_if_empty()
            .extend(["v1", "owners", owner.as_str(), "agents", name]);
        Ok(url)
    }

    fn form(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
        name: &str,
        suspend: bool,
    ) -> Result<Value, ProvisionError> {
        let mut form = self
            .forms
            .get(&kind)
            .cloned()
            .ok_or(ProvisionError::Invalid)?;
        if let Some(object) = form.as_object_mut() {
            object.insert("name".into(), json!(name));
            object.insert("owner".into(), json!(owner.as_str()));
            object.insert("suspend".into(), json!(suspend));
        }
        Ok(form)
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        headers: &[(reqwest::header::HeaderName, HeaderValue)],
        body: Option<&Value>,
    ) -> Result<reqwest::Response, ProvisionError> {
        let mut request = self
            .http
            .request(method, url)
            .header(AUTHORIZATION, self.authorization.clone());
        for (name, value) in headers {
            request = request.header(name, value.clone());
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request
            .send()
            .await
            .map_err(|e| ProvisionError::unavailable(e.without_url()))
    }

    /// Maps a status that is none of the ones the caller expects.
    fn unexpected(status: StatusCode) -> ProvisionError {
        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProvisionError::Refused,
            StatusCode::BAD_REQUEST
            | StatusCode::UNPROCESSABLE_ENTITY
            | StatusCode::PAYLOAD_TOO_LARGE
            | StatusCode::UNSUPPORTED_MEDIA_TYPE => ProvisionError::Invalid,
            other => ProvisionError::unavailable(format!("the platform answered {other}")),
        }
    }

    async fn get(&self, owner: &AgentOwner, name: &str) -> Result<Option<Seen>, ProvisionError> {
        let response = self
            .send(Method::GET, self.url(owner, name)?, &[], None)
            .await?;
        match response.status() {
            StatusCode::OK => {
                let etag = response.headers().get(ETAG).cloned();
                let body: Value = response
                    .json()
                    .await
                    .map_err(|e| ProvisionError::unavailable(e.without_url()))?;
                let state = parse_state(body.get("state").and_then(Value::as_str))?;
                Ok(Some(Seen { state, etag }))
            }
            StatusCode::NOT_FOUND => Ok(None),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Creates the agent. `true`: made or already there; `false` never (an error says why).
    async fn create(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
        name: &str,
    ) -> Result<(), ProvisionError> {
        let form = self.form(owner, kind, name, false)?;
        let response = self
            .send(
                Method::PUT,
                self.url(owner, name)?,
                &[(IF_NONE_MATCH, HeaderValue::from_static("*"))],
                Some(&form),
            )
            .await?;
        match response.status() {
            StatusCode::CREATED | StatusCode::OK => Ok(()),
            // Only `already-exists` means somebody made it first, which is what we wanted. The
            // platform's other 409s are different facts and must not read as "made".
            StatusCode::CONFLICT => match problem_slug(response).await.as_deref() {
                Some("already-exists") => Ok(()),
                // An agent managed by GitOps is not ours to write: retrying cannot help.
                Some("managed-by-gitops") => Err(ProvisionError::Invalid),
                Some("deleting") => Err(ProvisionError::Deleting),
                // `conflict`, a slug this build does not know, or no readable problem.
                _ => Err(ProvisionError::unavailable(
                    "the platform answered 409 for a reason this build cannot treat as made",
                )),
            },
            other => Err(Self::unexpected(other)),
        }
    }

    /// Replaces the agent with the owner's form and `suspend`. `false`: the agent changed or went
    /// away, read again.
    async fn replace(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
        name: &str,
        etag: &HeaderValue,
        suspend: bool,
    ) -> Result<bool, ProvisionError> {
        let form = self.form(owner, kind, name, suspend)?;
        let response = self
            .send(
                Method::PUT,
                self.url(owner, name)?,
                &[(IF_MATCH, etag.clone())],
                Some(&form),
            )
            .await?;
        match response.status() {
            StatusCode::OK | StatusCode::CREATED => Ok(true),
            StatusCode::PRECONDITION_FAILED | StatusCode::NOT_FOUND | StatusCode::CONFLICT => {
                Ok(false)
            }
            other => Err(Self::unexpected(other)),
        }
    }
}

/// The slug of a problem answer (`urn:vymalo:agents:problem:<slug>`), if the body is one.
async fn problem_slug(response: reqwest::Response) -> Option<String> {
    let body: Value = response.json().await.ok()?;
    let kind = body.get("type")?.as_str()?;
    kind.strip_prefix("urn:vymalo:agents:problem:")
        .map(str::to_owned)
}

fn parse_state(text: Option<&str>) -> Result<AgentState, ProvisionError> {
    Ok(match text {
        Some("saving") => AgentState::Saving,
        Some("blocked") => AgentState::Blocked,
        Some("degraded") => AgentState::Degraded,
        Some("ready") => AgentState::Ready,
        Some("suspended") => AgentState::Suspended,
        Some("deleting") => AgentState::Deleting,
        _ => {
            return Err(ProvisionError::unavailable(
                "an agent state this build cannot read",
            ));
        }
    })
}

fn raced() -> ProvisionError {
    ProvisionError::unavailable("the agent kept changing while it was being read")
}

impl AgentProvisioner for PlatformProvisioner {
    async fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<ProvisionedAgent, ProvisionError> {
        let name = kind.agent_name(owner);
        let deadline = Instant::now() + self.ready_timeout;
        let mut races = 0;
        let mut last = AgentState::Saving;
        loop {
            match self.get(owner, &name).await? {
                None => {
                    races += 1;
                    if races > MAX_RACES {
                        // Made, and still not ours to read: the name is held by another owner.
                        return Err(ProvisionError::Invalid);
                    }
                    self.create(owner, kind, &name).await?;
                }
                Some(seen) => {
                    last = seen.state;
                    match seen.state {
                        AgentState::Ready => {
                            return Ok(ProvisionedAgent {
                                name,
                                state: AgentState::Ready,
                            });
                        }
                        AgentState::Deleting => return Err(ProvisionError::Deleting),
                        AgentState::Suspended => {
                            races += 1;
                            if races > MAX_RACES {
                                return Err(raced());
                            }
                            let etag = seen.etag.ok_or_else(raced)?;
                            self.replace(owner, kind, &name, &etag, false).await?;
                        }
                        AgentState::Saving | AgentState::Blocked | AgentState::Degraded => {
                            if Instant::now() >= deadline {
                                return Err(ProvisionError::NotReady { last });
                            }
                            tokio::time::sleep(self.poll_interval).await;
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(ProvisionError::NotReady { last });
            }
        }
    }

    async fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> Result<Option<AgentState>, ProvisionError> {
        Ok(self
            .get(owner, &kind.agent_name(owner))
            .await?
            .map(|s| s.state))
    }

    async fn suspend(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        let name = kind.agent_name(owner);
        for _ in 0..MAX_RACES {
            let Some(seen) = self.get(owner, &name).await? else {
                return Ok(());
            };
            if matches!(seen.state, AgentState::Suspended | AgentState::Deleting) {
                return Ok(());
            }
            let etag = seen.etag.ok_or_else(raced)?;
            if self.replace(owner, kind, &name, &etag, true).await? {
                return Ok(());
            }
        }
        Err(raced())
    }

    async fn delete(&self, owner: &AgentOwner, kind: AgentKind) -> Result<(), ProvisionError> {
        let name = kind.agent_name(owner);
        for _ in 0..MAX_RACES {
            let Some(seen) = self.get(owner, &name).await? else {
                return Ok(());
            };
            let etag = seen.etag.ok_or_else(raced)?;
            let response = self
                .send(
                    Method::DELETE,
                    self.url(owner, &name)?,
                    &[(IF_MATCH, etag)],
                    None,
                )
                .await?;
            match response.status() {
                StatusCode::OK
                | StatusCode::ACCEPTED
                | StatusCode::NO_CONTENT
                | StatusCode::NOT_FOUND => return Ok(()),
                StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => {}
                other => return Err(Self::unexpected(other)),
            }
        }
        Err(raced())
    }
}
