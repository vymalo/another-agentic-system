//! The broker's settings: what a deployment may tune, with the defaults ADR 0046 names.

use std::time::Duration;

use url::Url;

use crate::BuildError;

/// How long before an access token runs out the broker refreshes it (ADR 0046: 30 seconds).
pub const DEFAULT_REFRESH_MARGIN: Duration = Duration::from_secs(30);

/// The longest a [`CredentialLease`](orch_ports::CredentialLease) lasts.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(300);

/// How long an authorization that has begun may take before it is forgotten.
pub const DEFAULT_FLOW_TTL: Duration = Duration::from_secs(600);

/// The longest one request to an authorization server may take, from connecting to the last byte.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a connection may take to be made.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The most bytes of any response the broker reads: metadata, a token, a registration.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// The longest a lease may be set to last: an hour. A lease is the broker's answer to "may I send
/// this credential now", and the relay asks for one per call.
pub const MAX_LEASE_TTL: Duration = Duration::from_secs(3600);

/// The longest the margin before an access token runs out may be set to: ten minutes.
pub const MAX_REFRESH_MARGIN: Duration = Duration::from_secs(600);

/// The longest an authorization that has begun may be allowed to take: an hour.
pub const MAX_FLOW_TTL: Duration = Duration::from_secs(3600);

/// The longest a request, and a connection, may be allowed to take: a minute. A refresh holds a
/// claim for as long as a request can take (and a margin), so the bound is also the longest other
/// replicas wait for one.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(60);

/// The most bytes of a response the broker may be set to read: a mebibyte.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// The longest the URL of the Client ID Metadata Document may be, in bytes: the client id the broker
/// would hand to authorization servers is that URL, and it is kept in every pending flow.
const MAX_CLIENT_METADATA_URL_BYTES: usize = 2048;

/// The name a dynamically registered client gives itself (RFC 7591 `client_name`).
pub const DEFAULT_CLIENT_NAME: &str = "another-agentic-system";

/// How long past one request to an authorization server a claim to refresh lasts: the time to keep
/// what the server answered, and the replicas' clocks disagreeing.
pub(crate) const CLAIM_MARGIN: Duration = Duration::from_secs(10);

/// What a deployment may tune. `Default` is what ADR 0046 says; every setter returns the changed
/// copy. Checked when the broker is built ([`BrokerBuilder::build`](crate::BrokerBuilder::build)):
/// every duration is above zero and at most its `MAX_*` bound, and so is the size of a response.
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub(crate) client_name: String,
    pub(crate) client_metadata_url: Option<String>,
    pub(crate) refresh_margin: Duration,
    pub(crate) lease_ttl: Duration,
    pub(crate) flow_ttl: Duration,
    pub(crate) request_timeout: Duration,
    pub(crate) connect_timeout: Duration,
    pub(crate) max_response_bytes: usize,
}

impl Default for BrokerConfig {
    fn default() -> Self {
        BrokerConfig {
            client_name: DEFAULT_CLIENT_NAME.to_owned(),
            client_metadata_url: None,
            refresh_margin: DEFAULT_REFRESH_MARGIN,
            lease_ttl: DEFAULT_LEASE_TTL,
            flow_ttl: DEFAULT_FLOW_TTL,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }
}

impl BrokerConfig {
    /// The name a client registered dynamically (RFC 7591) shows the person on the consent page.
    #[must_use]
    pub fn with_client_name(mut self, name: impl Into<String>) -> Self {
        self.client_name = name.into();
        self
    }

    /// The URL of the orchestrator's **Client ID Metadata Document**: with it, an authorization
    /// server that says it supports such documents is given this URL as the `client_id` and
    /// registers nothing. The orchestrator serves the document at that URL (the broker only names
    /// it, and the authorization server fetches it); it must be an `https` URL with a path.
    /// Without it that step of the registration order is skipped.
    #[must_use]
    pub fn with_client_metadata_url(mut self, url: impl Into<String>) -> Self {
        self.client_metadata_url = Some(url.into());
        self
    }

    /// How long before an access token runs out it is refreshed.
    #[must_use]
    pub fn with_refresh_margin(mut self, margin: Duration) -> Self {
        self.refresh_margin = margin;
        self
    }

    /// The longest a lease lasts (it ends sooner when the access token does).
    #[must_use]
    pub fn with_lease_ttl(mut self, ttl: Duration) -> Self {
        self.lease_ttl = ttl;
        self
    }

    /// How long an authorization that has begun may take.
    #[must_use]
    pub fn with_flow_ttl(mut self, ttl: Duration) -> Self {
        self.flow_ttl = ttl;
        self
    }

    /// The longest one request to an authorization server may take, and the longest a connection
    /// may take to be made.
    #[must_use]
    pub fn with_timeouts(mut self, request: Duration, connect: Duration) -> Self {
        self.request_timeout = request;
        self.connect_timeout = connect;
        self
    }

    /// The most bytes of any response that are read; a longer one is refused.
    #[must_use]
    pub fn with_max_response_bytes(mut self, max: usize) -> Self {
        self.max_response_bytes = max;
        self
    }

    /// How long a replica's claim to refresh holds: the longest the request can take, and a margin.
    /// A replica that dies mid-refresh leaves its claim for this long, and then another takes over.
    pub(crate) fn claim_ttl(&self) -> Duration {
        self.request_timeout.saturating_add(CLAIM_MARGIN)
    }

    pub(crate) fn validate(&self) -> Result<(), BuildError> {
        let within = |d: Duration, max: Duration| !d.is_zero() && d <= max;
        let sane = within(self.refresh_margin, MAX_REFRESH_MARGIN)
            && within(self.lease_ttl, MAX_LEASE_TTL)
            && within(self.flow_ttl, MAX_FLOW_TTL)
            && within(self.request_timeout, MAX_TIMEOUT)
            && within(self.connect_timeout, MAX_TIMEOUT)
            && (1..=MAX_RESPONSE_BYTES).contains(&self.max_response_bytes);
        if !sane || self.client_name.trim().is_empty() {
            return Err(BuildError::Config);
        }
        if let Some(text) = &self.client_metadata_url {
            let url = Url::parse(text).map_err(|_| BuildError::ClientMetadataUrl)?;
            let ok = text.len() <= MAX_CLIENT_METADATA_URL_BYTES
                && url.scheme() == "https"
                && url.host().is_some()
                && url.path() != "/"
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none();
            if !ok {
                return Err(BuildError::ClientMetadataUrl);
            }
        }
        Ok(())
    }
}
