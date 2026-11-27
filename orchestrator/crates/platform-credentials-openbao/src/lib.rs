//! [`OpenBaoClientCertificates`]: the [`PlatformClientCertificates`] of a deployment whose
//! orchestrator authenticates to the Platform API with its **workload certificate** (ANR-809).
//!
//! The certificate is a client-authentication leaf issued by an OpenBao PKI secrets engine, for
//! this one use (it is not the certificate that signs prompts, ANR-718):
//!
//! 1. the service-account token of the pod is read **at each login** (it rotates on disk) and
//!    posted to the Kubernetes auth method of OpenBao, which answers a short-lived client token;
//! 2. a fresh ECDSA P-256 key is made **in memory** and a CSR with no subject and no names is
//!    signed by the PKI role (`POST /v1/<pki>/sign/<role>`), which puts the configured SPIFFE id
//!    in the leaf as its one URI name;
//! 3. the leaf is **checked before it is used** (it holds our key, is for client authentication
//!    and not for servers, names exactly the configured id and is valid now), and anything else is
//!    refused as unusable;
//! 4. it is renewed once the renewal margin before its end of validity is reached, with a new key,
//!    by one caller while the others wait (single flight). Until its end of validity the old leaf
//!    is still served when a renewal fails; after it, there is no identity and the call fails.
//!
//! No key, CSR, certificate, token or token file content is ever put in a log, an error or a
//! `Debug`. Nothing of OpenBao's own crates is a dependency (ADR 0007): the adapter speaks HTTP
//! and JSON.

mod leaf;
mod pki;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_ports::{
    Clock, PlatformCertificateFuture, PlatformClientCertificate, PlatformClientCertificates,
    PlatformCredentialsError, SystemClock,
};
use tokio::sync::Mutex;
use url::Url;

/// The default Kubernetes auth mount.
pub const DEFAULT_KUBERNETES_MOUNT: &str = "kubernetes";
/// Where a pod's service-account token is projected.
pub const DEFAULT_SERVICE_ACCOUNT_TOKEN_FILE: &str =
    "/var/run/secrets/kubernetes.io/serviceaccount/token";
/// The default life asked of a leaf: one day. No revocation list exists yet, so the life of a
/// leaf is also its revocation window.
pub const DEFAULT_TTL_SECS: u64 = 86_400;
/// The default margin before the end of validity at which a leaf is renewed: eight hours, so a
/// renewal has three tries a day before the leaf lapses.
pub const DEFAULT_RENEWAL_MARGIN_SECS: u64 = 28_800;
/// How long after a failed issue (an OpenBao that is down, a leaf that is refused) nothing is tried
/// again, whether or not a leaf is still held: the callers that come meanwhile are answered the
/// same error, or the old leaf, and OpenBao is left alone when it is down. The pause ends early
/// when the held leaf lapses, and is not waited for when the clock steps back.
const RETRY_AFTER: SignedDuration = SignedDuration::from_secs(30);
/// The same after a **refusal** (a 400, 401 or 403): a fault of the deployment's setup that a retry
/// does not mend, so it is tried less often than a fault that passes by itself.
const RETRY_AFTER_REFUSED: SignedDuration = SignedDuration::from_secs(60);
/// The shortest life a leaf may be asked for.
const MIN_TTL: Duration = Duration::from_secs(60);

/// How to reach OpenBao and what to ask of it.
#[derive(Clone)]
pub struct OpenBaoPkiConfig {
    /// OpenBao's address (`https://bao.internal:8200`), with a host, no user name, password,
    /// query or fragment.
    pub address: String,
    /// PEM certificates OpenBao's own certificate is also checked against, besides the system's
    /// roots (a private authority). `None`: the system's roots alone.
    pub ca_pem: Option<String>,
    /// The Kubernetes auth mount (`kubernetes`).
    pub kubernetes_mount: String,
    /// The role of that auth method the pod's service account logs in as.
    pub auth_role: String,
    /// The file the pod's service-account token is read from, at each login.
    pub service_account_token_file: PathBuf,
    /// The PKI secrets engine's mount.
    pub pki_mount: String,
    /// The PKI role that signs this certificate (its own, for this one use).
    pub pki_role: String,
    /// The SPIFFE id the leaf must carry as its one URI name: `spiffe://<domain>/<path>`.
    pub spiffe_id: String,
    /// The life asked of each leaf (at least a minute; the role's `max_ttl` may cut it).
    pub ttl: Duration,
    /// A leaf is renewed once it has this long left. Must be less than `ttl`.
    pub renewal_margin: Duration,
    /// How long one request to OpenBao may take.
    pub timeout: Duration,
}

impl OpenBaoPkiConfig {
    /// A configuration with the defaults: the `kubernetes` mount, the pod's own token file, a
    /// life of a day, renewal eight hours before the end, 5 s per request.
    pub fn new(
        address: impl Into<String>,
        auth_role: impl Into<String>,
        pki_mount: impl Into<String>,
        pki_role: impl Into<String>,
        spiffe_id: impl Into<String>,
    ) -> Self {
        Self {
            address: address.into(),
            ca_pem: None,
            kubernetes_mount: DEFAULT_KUBERNETES_MOUNT.to_owned(),
            auth_role: auth_role.into(),
            service_account_token_file: PathBuf::from(DEFAULT_SERVICE_ACCOUNT_TOKEN_FILE),
            pki_mount: pki_mount.into(),
            pki_role: pki_role.into(),
            spiffe_id: spiffe_id.into(),
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            renewal_margin: Duration::from_secs(DEFAULT_RENEWAL_MARGIN_SECS),
            timeout: Duration::from_secs(5),
        }
    }
}

impl fmt::Debug for OpenBaoPkiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoPkiConfig")
            .field("address", &self.address)
            .field("custom_ca", &self.ca_pem.is_some())
            .field("kubernetes_mount", &self.kubernetes_mount)
            .field("auth_role", &self.auth_role)
            .field("pki_mount", &self.pki_mount)
            .field("pki_role", &self.pki_role)
            .field("spiffe_id", &self.spiffe_id)
            .field("ttl", &self.ttl)
            .field("renewal_margin", &self.renewal_margin)
            .finish_non_exhaustive()
    }
}

/// The adapter could not be built. Each case names the setting, never a value.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The address is not an absolute `http(s)` URL with a host and no user name, password, query
    /// or fragment.
    #[error(
        "the OpenBao address must be an absolute URL with a host, and no user name, password, query or fragment"
    )]
    BadAddress,
    /// A mount or a role is empty, too long, or has a character a URL path cannot carry safely.
    #[error(
        "{0} must be 1 to 128 characters of A-Z a-z 0-9 . _ - (and / inside a mount), with no empty or dot part"
    )]
    BadName(&'static str),
    /// The SPIFFE id is not `spiffe://<trust domain>/<path>` in its plain form.
    #[error(
        "the SPIFFE id must be spiffe://<trust domain>/<path>, with no user name, port, query or fragment"
    )]
    BadSpiffeId,
    /// The life or the renewal margin is out of range.
    #[error("the life must be at least a minute and the renewal margin must be less than the life")]
    BadLifetime,
    /// The CA given for OpenBao's certificate cannot be read as PEM certificates.
    #[error("the OpenBao CA is not a PEM certificate bundle")]
    BadCa,
    /// The HTTP client could not be built.
    #[error("cannot build the OpenBao HTTP client")]
    Http,
}

/// What the adapter holds between two calls: the leaf, and the last failed issue with the moment
/// before which none is tried again.
#[derive(Default)]
struct State {
    held: Option<PlatformClientCertificate>,
    retry_not_before: Option<Timestamp>,
    /// How long the pause that ends at `retry_not_before` is: a pause that is longer than this
    /// from now means the clock stepped back, and is not waited for.
    pause: SignedDuration,
    /// What the last issue failed with; kept while the pause lasts.
    failure: Option<PlatformCredentialsError>,
}

impl State {
    /// Whether a failed issue is still being waited out at `now`.
    fn backing_off(&self, now: Timestamp) -> bool {
        match self.retry_not_before {
            Some(until) if now < until => until.duration_since(now) <= self.pause,
            _ => false,
        }
    }
}

/// [`PlatformClientCertificates`] from an OpenBao PKI role, over the pod's service-account login.
pub struct OpenBaoClientCertificates {
    issuer: pki::Issuer,
    clock: Arc<dyn Clock>,
    margin: SignedDuration,
    state: Mutex<State>,
}

impl fmt::Debug for OpenBaoClientCertificates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoClientCertificates")
            .finish_non_exhaustive()
    }
}

impl OpenBaoClientCertificates {
    /// Checks the settings and builds the adapter on the system clock. Nothing is asked of
    /// OpenBao until the first certificate is wanted.
    pub fn new(config: OpenBaoPkiConfig) -> Result<Self, BuildError> {
        Self::with_clock(config, Arc::new(SystemClock))
    }

    /// The same on `clock`: what "now" and "expired" mean for the leaf (tests move it).
    pub fn with_clock(config: OpenBaoPkiConfig, clock: Arc<dyn Clock>) -> Result<Self, BuildError> {
        let address = Url::parse(config.address.trim()).map_err(|_| BuildError::BadAddress)?;
        let address_ok = matches!(address.scheme(), "http" | "https")
            && address.host_str().is_some()
            && address.username().is_empty()
            && address.password().is_none()
            && address.query().is_none()
            && address.fragment().is_none();
        if !address_ok {
            return Err(BuildError::BadAddress);
        }
        for (name, value, slashes) in [
            ("the Kubernetes auth mount", &config.kubernetes_mount, true),
            ("the auth role", &config.auth_role, false),
            ("the PKI mount", &config.pki_mount, true),
            ("the PKI role", &config.pki_role, false),
        ] {
            if !is_name(value, slashes) {
                return Err(BuildError::BadName(name));
            }
        }
        if !is_spiffe_id(&config.spiffe_id) {
            return Err(BuildError::BadSpiffeId);
        }
        if config.ttl < MIN_TTL || config.renewal_margin >= config.ttl {
            return Err(BuildError::BadLifetime);
        }
        let margin =
            SignedDuration::try_from(config.renewal_margin).map_err(|_| BuildError::BadLifetime)?;
        let issuer = pki::Issuer::new(&address, &config)?;
        Ok(Self {
            issuer,
            clock,
            margin,
            state: Mutex::new(State::default()),
        })
    }

    async fn certificate(&self) -> Result<PlatformClientCertificate, PlatformCredentialsError> {
        // One caller at a time decides and renews: the others wait for the answer and then find a
        // fresh leaf, so a burst of requests makes one login and one signing.
        let mut state = self.state.lock().await;
        let now = self.clock.now();
        // A leaf that has lapsed is dropped with its key, and a pause begun while it was good ends
        // with it: the next caller may try again at once.
        if state
            .held
            .as_ref()
            .is_some_and(|held| now >= held.not_after())
        {
            state.held = None;
            state.retry_not_before = None;
            state.failure = None;
        }
        let valid = state.held.clone();
        let backing_off = state.backing_off(now);
        if let Some(held) = &valid {
            if !renewal_due(held.not_after(), self.margin, now) || backing_off {
                return Ok(held.clone());
            }
        } else if let (true, Some(error)) = (backing_off, state.failure) {
            // No identity, and the last try failed a moment ago: the callers that queued behind it
            // are answered what it was answered, and OpenBao is not asked again until the pause is over.
            return Err(error);
        }
        match self.issuer.issue(self.clock.as_ref(), self.margin).await {
            Ok(fresh) => {
                tracing::info!(
                    serial = fresh.serial(),
                    not_after = %fresh.not_after(),
                    "the platform client certificate is issued"
                );
                state.held = Some(fresh.clone());
                state.retry_not_before = None;
                state.failure = None;
                Ok(fresh)
            }
            Err(failure) => {
                tracing::warn!(
                    reason = %failure,
                    still_valid = valid.is_some(),
                    "the platform client certificate could not be renewed"
                );
                let error = PlatformCredentialsError::from(failure);
                let pause = if error == PlatformCredentialsError::Refused {
                    RETRY_AFTER_REFUSED
                } else {
                    RETRY_AFTER
                };
                state.retry_not_before = now.checked_add(pause).ok();
                state.pause = pause;
                state.failure = Some(error);
                match valid {
                    // Inside the margin, and the old leaf was good before the issue began: it is
                    // judged again now, for the issue can take two requests of the timeout each.
                    Some(held) if self.clock.now() < held.not_after() => Ok(held),
                    Some(_) => {
                        state.held = None;
                        Err(PlatformCredentialsError::Unavailable)
                    }
                    // None: no identity.
                    None => Err(error),
                }
            }
        }
    }
}

impl PlatformClientCertificates for OpenBaoClientCertificates {
    fn current(&self) -> PlatformCertificateFuture<'_> {
        Box::pin(self.certificate())
    }
}

/// Whether `now` is within `margin` of `not_after` (or past it).
fn renewal_due(not_after: Timestamp, margin: SignedDuration, now: Timestamp) -> bool {
    match not_after.checked_sub(margin) {
        Ok(renew_at) => now >= renew_at,
        Err(_) => true,
    }
}

/// A role (`slashes: false`) or a mount (`slashes: true`): parts of `A-Z a-z 0-9 . _ -` joined by
/// single `/`, 1 to 128 characters, neither `.` nor `..` as a part.
fn is_name(raw: &str, slashes: bool) -> bool {
    !raw.is_empty()
        && raw.len() <= 128
        && (slashes || !raw.contains('/'))
        && raw.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

/// `spiffe://<trust domain>/<path>` in the form a parser writes back unchanged.
fn is_spiffe_id(raw: &str) -> bool {
    raw.len() <= 2048
        && Url::parse(raw).is_ok_and(|u| {
            u.scheme() == "spiffe"
                && u.host_str()
                    .is_some_and(|h| !h.is_empty() && h == h.to_ascii_lowercase())
                && u.port().is_none()
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && u.fragment().is_none()
                && u.path().len() > 1
                && u.as_str() == raw
        })
}
