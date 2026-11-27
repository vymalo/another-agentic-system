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
//! * What the orchestrator presents is one of two modes ([`PlatformAuth`]): a bearer token from a
//!   [`PlatformCredentials`], or a **client certificate** on the TLS connection from a
//!   [`PlatformClientCertificates`] (ANR-809). A mode is chosen when the provisioner is built and
//!   never falls back to the other: in certificate mode a certificate that cannot be had sends
//!   nothing, and no `Authorization` header is sent.

mod oidc;

pub use oidc::{OidcClientConfig, OidcClientCredentials};

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use orch_core::BoxError;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, BlockedReason, PlatformClientCertificate,
    PlatformClientCertificates, PlatformCredentials, PlatformCredentialsError, PlatformToken,
    PlatformTokenFuture, ProvisionError, ProvisionedAgent,
};
use reqwest::header::{AUTHORIZATION, ETAG, HeaderValue, IF_MATCH, IF_NONE_MATCH};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use tokio::time::Instant;
use url::Url;
use zeroize::Zeroizing;

/// Times a write is tried again after the platform says the agent changed meanwhile.
const MAX_RACES: u32 = 8;

/// What the orchestrator presents to the Platform API. One mode per provisioner, chosen at build
/// time: the two never fall back to one another.
#[derive(Clone)]
pub enum PlatformAuth {
    /// A bearer token, asked of the port before every request and never kept.
    Bearer(Arc<dyn PlatformCredentials>),
    /// A client certificate presented on the TLS connection, asked of the port before every request
    /// (so a renewal is the port's business); no `Authorization` header is sent.
    ClientCertificate(Arc<dyn PlatformClientCertificates>),
}

impl fmt::Debug for PlatformAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformAuth::Bearer(_) => f.write_str("PlatformAuth::Bearer(<redacted>)"),
            PlatformAuth::ClientCertificate(_) => {
                f.write_str("PlatformAuth::ClientCertificate(<redacted>)")
            }
        }
    }
}

/// How to reach the Platform API.
#[derive(Clone)]
pub struct PlatformConfig {
    /// The API's base URL (`https://platform.example.com`), `http` or `https`, with a host and no
    /// user name or password. `/v1/...` is appended. A client certificate needs `https`.
    pub url: String,
    /// What the orchestrator presents (it must hold `platform:agents.own`). Asked before every
    /// request, so a token or a certificate that expires or rotates is the port's business.
    pub auth: PlatformAuth,
    /// PEM certificates the listener's certificate is also checked against, besides the system's
    /// roots (a private authority). `None`: the system's roots alone.
    pub server_ca_pem: Option<String>,
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
    /// A configuration for `url` with a fixed `token`: no forms, 5 s per request, 120 s to be
    /// ready, polled every 2 s, the environment's proxy honoured.
    pub fn new(url: impl Into<String>, token: SecretString) -> Self {
        Self::with_credentials(url, Arc::new(StaticCredentials::new(token)))
    }

    /// The same, presenting what `credentials` gives at each request.
    pub fn with_credentials(
        url: impl Into<String>,
        credentials: Arc<dyn PlatformCredentials>,
    ) -> Self {
        Self::with_auth(url, PlatformAuth::Bearer(credentials))
    }

    /// The same, presenting the client certificate `certificates` gives at each request, on the
    /// TLS connection and with no `Authorization` header. `url` must be `https`.
    pub fn with_client_certificate(
        url: impl Into<String>,
        certificates: Arc<dyn PlatformClientCertificates>,
    ) -> Self {
        Self::with_auth(url, PlatformAuth::ClientCertificate(certificates))
    }

    fn with_auth(url: impl Into<String>, auth: PlatformAuth) -> Self {
        PlatformConfig {
            url: url.into(),
            auth,
            server_ca_pem: None,
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

    /// Also trusts the PEM certificates in `pem` for the listener (a private authority).
    #[must_use]
    pub fn with_server_ca_pem(mut self, pem: impl Into<String>) -> Self {
        self.server_ca_pem = Some(pem.into());
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
            .field("auth", &self.auth)
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
    /// A client certificate is presented on a TLS connection, so the URL must be `https`.
    #[error("a client certificate needs an https:// platform URL")]
    CertificateNeedsHttps,
    /// The PEM certificates given for the listener cannot be read.
    #[error("the platform's server CA is not a PEM certificate bundle")]
    BadServerCa,
    /// A form is not a JSON object, or sets a field the adapter owns.
    #[error("the form of {0:?} must be a JSON object without name, owner or suspend")]
    BadForm(AgentKind),
    /// The HTTP client could not be built.
    #[error("cannot build the platform HTTP client")]
    Http(#[source] BoxError),
}

/// What every client of the provisioner shares.
struct ClientSettings {
    timeout: Duration,
    use_system_proxy: bool,
    roots: Vec<reqwest::Certificate>,
}

impl ClientSettings {
    /// A client with these settings, presenting `identity` on the TLS connection when there is one.
    fn build(&self, identity: Option<reqwest::Identity>) -> Result<reqwest::Client, BuildError> {
        // Err means a provider is already installed, which is what we want.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut builder = reqwest::Client::builder()
            .timeout(self.timeout)
            .connect_timeout(self.timeout)
            // What is presented goes to the configured URL and nowhere else.
            .redirect(reqwest::redirect::Policy::none());
        if !self.use_system_proxy {
            builder = builder.no_proxy();
        }
        for root in &self.roots {
            builder = builder.add_root_certificate(root.clone());
        }
        if let Some(identity) = identity {
            builder = builder.identity(identity);
        }
        builder
            .build()
            .map_err(|e| BuildError::Http(e.without_url().into()))
    }
}

/// A port's refusal is the platform's refusal; anything else is "try later".
fn credentials_error(e: PlatformCredentialsError) -> ProvisionError {
    match e {
        PlatformCredentialsError::Refused => ProvisionError::Refused,
        other => ProvisionError::unavailable(other),
    }
}

/// The TLS identity of a certificate: its chain and its key, as the one PEM buffer reqwest reads.
/// The buffer holds the key, so it is wiped when it is dropped.
fn identity_of(
    certificate: &PlatformClientCertificate,
) -> Result<reqwest::Identity, ProvisionError> {
    let mut pem = Zeroizing::new(String::new());
    for der in certificate.chain_der() {
        push_pem(&mut pem, "CERTIFICATE", der);
    }
    push_pem(&mut pem, "PRIVATE KEY", certificate.expose_key_pkcs8());
    reqwest::Identity::from_pem(pem.as_bytes())
        .map_err(|_| ProvisionError::unavailable("the platform client certificate is unusable"))
}

fn push_pem(out: &mut String, label: &str, der: &[u8]) {
    let mut body = Zeroizing::new(String::new());
    B64.encode_string(der, &mut body);
    out.push_str("-----BEGIN ");
    out.push_str(label);
    out.push_str("-----\n");
    for line in body.as_bytes().chunks(64) {
        out.extend(line.iter().map(|b| char::from(*b)));
        out.push('\n');
    }
    out.push_str("-----END ");
    out.push_str(label);
    out.push_str("-----\n");
}

/// What a read of one agent said.
struct Seen {
    state: AgentState,
    /// For a `Blocked` agent: the reason of a condition that waiting cannot clear, if there is one.
    terminal: Option<BlockedReason>,
    etag: Option<HeaderValue>,
}

/// A token that never changes: for a deployment that hands the orchestrator one, and for tests.
pub struct StaticCredentials(SecretString);

impl StaticCredentials {
    /// Presents `token` at every request.
    pub fn new(token: SecretString) -> Self {
        Self(token)
    }
}

impl fmt::Debug for StaticCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StaticCredentials(<redacted>)")
    }
}

impl PlatformCredentials for StaticCredentials {
    fn token(&self) -> PlatformTokenFuture<'_> {
        let token = PlatformToken::new(self.0.expose_secret());
        Box::pin(async move { Ok(token) })
    }
}

/// The [`AgentProvisioner`] over the Platform API.
#[derive(Clone)]
pub struct PlatformProvisioner {
    /// The client of the bearer mode (and a plain one, never used, in the certificate mode).
    http: reqwest::Client,
    /// How the clients are built, for the certificate mode which builds one per certificate.
    settings: Arc<ClientSettings>,
    /// The certificate mode's client and the serial of the certificate it presents: rebuilt exactly
    /// when the serial changes.
    mtls: Arc<Mutex<Option<(String, reqwest::Client)>>>,
    base: Url,
    auth: PlatformAuth,
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
        for (kind, form) in &cfg.forms {
            let object = form.as_object().ok_or(BuildError::BadForm(*kind))?;
            if ["name", "owner", "suspend"]
                .iter()
                .any(|k| object.contains_key(*k))
            {
                return Err(BuildError::BadForm(*kind));
            }
        }
        let mut roots = Vec::new();
        if let Some(pem) = &cfg.server_ca_pem {
            roots = reqwest::Certificate::from_pem_bundle(pem.as_bytes())
                .map_err(|_| BuildError::BadServerCa)?;
            if roots.is_empty() {
                return Err(BuildError::BadServerCa);
            }
        }
        if matches!(cfg.auth, PlatformAuth::ClientCertificate(_)) && base.scheme() != "https" {
            return Err(BuildError::CertificateNeedsHttps);
        }
        let settings = Arc::new(ClientSettings {
            timeout: cfg.timeout,
            use_system_proxy: cfg.use_system_proxy,
            roots,
        });
        let http = settings.build(None)?;
        Ok(PlatformProvisioner {
            http,
            settings,
            mtls: Arc::new(Mutex::new(None)),
            base,
            auth: cfg.auth,
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

    /// The client to send with and, in the bearer mode, the `Authorization` header.
    async fn identity(&self) -> Result<(reqwest::Client, Option<HeaderValue>), ProvisionError> {
        match &self.auth {
            PlatformAuth::Bearer(credentials) => {
                let token = credentials.token().await.map_err(credentials_error)?;
                let mut authorization =
                    HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                        .map_err(|_| ProvisionError::Refused)?;
                authorization.set_sensitive(true);
                Ok((self.http.clone(), Some(authorization)))
            }
            PlatformAuth::ClientCertificate(certificates) => {
                let certificate = certificates.current().await.map_err(credentials_error)?;
                // The port answers a certificate that is valid now; the platform would refuse one
                // that is not, but it is not the only check: nothing is sent with a lapsed one.
                if certificate.not_after() <= jiff::Timestamp::now() {
                    return Err(ProvisionError::unavailable(
                        "the platform client certificate is over",
                    ));
                }
                Ok((self.certificate_client(&certificate)?, None))
            }
        }
    }

    /// The client that presents `certificate`: the cached one while the serial is the same, a new
    /// one (a new connection pool, so a new TLS handshake with the new identity) when it changed.
    fn certificate_client(
        &self,
        certificate: &PlatformClientCertificate,
    ) -> Result<reqwest::Client, ProvisionError> {
        let mut cache = self.mtls.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, client)) = cache
            .as_ref()
            .filter(|(serial, _)| serial.as_str() == certificate.serial())
        {
            return Ok(client.clone());
        }
        let identity = identity_of(certificate)?;
        let client = self
            .settings
            .build(Some(identity))
            .map_err(ProvisionError::unavailable)?;
        *cache = Some((certificate.serial().to_owned(), client.clone()));
        Ok(client)
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        headers: &[(reqwest::header::HeaderName, HeaderValue)],
        body: Option<&Value>,
    ) -> Result<reqwest::Response, ProvisionError> {
        // Asked at every request, and never kept: the port owns expiry and rotation. In the
        // certificate mode an error ends here: nothing is sent without an identity.
        let (client, authorization) = self.identity().await?;
        let mut request = client.request(method, url);
        if let Some(authorization) = authorization {
            request = request.header(AUTHORIZATION, authorization);
        }
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
                let terminal = if state == AgentState::Blocked {
                    terminal_reason(&body)
                } else {
                    None
                };
                Ok(Some(Seen {
                    state,
                    terminal,
                    etag,
                }))
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

/// The conditions of a `Blocked` agent that waiting cannot clear, as the platform writes them
/// (`conditions` of the agent's detail: objects with `type`, `status` and `reason`; the platform's
/// `crates/api/src/status.rs` names the reasons): a `RuntimeReady` that is `False` for `NameConflict`
/// (an object the runtime needs exists and is not the platform's) and a `StoreReady` that is
/// `False` for `CNPGNotInstalled` (the database operator is absent). Every other blocked reason
/// (`ClusterNotReady`, `ConfigNotResolved`, `ConfigInvalid`, which also covers a referent not made
/// yet) passes by itself and is waited for; a missing or unreadable `conditions` is waited for too.
fn terminal_reason(body: &Value) -> Option<BlockedReason> {
    body.get("conditions")?.as_array()?.iter().find_map(|c| {
        let field = |name: &str| c.get(name).and_then(Value::as_str);
        if field("status") != Some("False") {
            return None;
        }
        match (field("type")?, field("reason")?) {
            ("RuntimeReady", "NameConflict") => Some(BlockedReason::NameConflict),
            ("StoreReady", "CNPGNotInstalled") => Some(BlockedReason::StoreOperatorMissing),
            _ => None,
        }
    })
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
                    // Blocked is waited for like Saving, because a fresh store or a referent not
                    // made yet blocks an agent that then goes `ready`; only a reason that waiting
                    // cannot clear (`terminal_reason`) ends the wait at once.
                    if let Some(reason) = seen.terminal {
                        return Err(ProvisionError::Blocked { reason });
                    }
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
                        // Saving and Degraded (applied, not ready: a rollout or a crash loop that
                        // goes back to Ready by itself) are waited for, up to the bound.
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
