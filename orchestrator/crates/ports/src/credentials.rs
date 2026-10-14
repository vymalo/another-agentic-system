//! The credential broker port: how the orchestrator stores, obtains and drops the credentials of a
//! person's MCP servers (ADR 0046, the amendment of 2026-10-11, ADR 0009).
//!
//! A person's server ([`UserToolServer`](crate::UserToolServer)) holds no secret. The credential
//! (a bearer token, an API key, or the tokens of an OAuth authorization) is the **broker's**: the
//! orchestrator's own, one implementation written once (`orch-credential-broker`, a later slice),
//! which does the OAuth work (discovery, PKCE `S256`, the `resource` parameter, client
//! registration, refresh) and keeps what it must in a [`SecretStore`](crate::SecretStore). This
//! module is the port the API and the relay are written against:
//!
//! | Call | Does |
//! |---|---|
//! | [`store`](CredentialBroker::store) | Keeps a token, a key or a client secret the person typed; returns its [`CredentialRef`]. |
//! | [`begin_oauth`](CredentialBroker::begin_oauth) | Starts an authorization: the URL to send the person to and the `state` that comes back. |
//! | [`complete_oauth`](CredentialBroker::complete_oauth) | Finishes it with the `code` the redirect brought. |
//! | [`lease`](CredentialBroker::lease) | A **fresh** credential for one call to the server, short-lived; refreshed if need be. |
//! | [`revoke`](CredentialBroker::revoke) | Forgets the credential, and revokes it upstream where the server can. |
//!
//! The port carries the key of a server, `(owner, server id)`, and not its URL, scopes or client
//! id: an implementation reads those from the definition in the
//! [`UserToolServerStore`](crate::UserToolServerStore) it is composed with, which stays the one
//! place that says what a server is. Every URL it then fetches (the server's, its resource
//! metadata, the authorization server's metadata and token endpoint) goes through the
//! [`EgressPolicy`](crate::EgressPolicy).
//!
//! # A secret appears only
//!
//! in the argument of [`store`](CredentialBroker::store) and [`complete_oauth`](CredentialBroker::complete_oauth)
//! (a [`ToolSecret`], which prints `<redacted>`), in a [`CredentialLease`] on its way to one request
//! to the server, and sealed in the secret store. It is in no event, export, frame, log line, step
//! input or output, error or agent file; no error here holds one.

use std::fmt;
use std::future::Future;

use jiff::Timestamp;
use orch_core::{BoxError, Classify, ErrorClass, UserId};

use crate::tools::ToolSecret;
use crate::user_tools::{DefinitionError, UserAuth, UserToolServerId};

/// The longest [`CredentialRef`], in bytes.
pub const MAX_CREDENTIAL_REF_BYTES: usize = 128;

/// An opaque id of a stored credential, as the broker made it: 1 to 128 bytes of visible ASCII.
///
/// It is not a secret (it names a credential, it does not open one) and it is not the key of the
/// credential: the broker keys what it stores by owner and server. Nothing outside the broker reads
/// anything into its text.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef(String);

impl CredentialRef {
    /// Wraps the id a broker gave.
    ///
    /// # Errors
    /// [`DefinitionError::BadCredentialRef`] when `text` is empty, longer than
    /// [`MAX_CREDENTIAL_REF_BYTES`] or not visible ASCII (what a stored row that was altered would
    /// look like).
    pub fn new(text: &str) -> Result<Self, DefinitionError> {
        if text.is_empty()
            || text.len() > MAX_CREDENTIAL_REF_BYTES
            || !text.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(DefinitionError::BadCredentialRef);
        }
        Ok(CredentialRef(text.to_owned()))
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialRef({})", self.0)
    }
}

impl fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ------------------------------------------------------------------------------------- the lease

/// What the relay is lent for one call to a person's server: the header to send, its value, and when
/// the loan ends.
///
/// A lease is **fresh** (the broker refreshed the token if it was about to run out) and **short**:
/// the relay asks for one per `tools/list` and per `tools/call` and keeps none, so a token is never
/// older than the broker says. `Debug` shows the header and the time and never the value.
///
/// `value` is the credential itself. For the header `Authorization` it is the **bearer token
/// without the `Bearer ` scheme** (the relay hands it to
/// [`ToolServerEndpoint::with_bearer`](crate::ToolServerEndpoint::with_bearer), which sends
/// `Authorization: Bearer <value>`); for any other header (an `api_key`) it is the value to send
/// as it is ([`ToolServerEndpoint::with_header`](crate::ToolServerEndpoint::with_header)).
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialLease {
    /// The header the credential travels in: `Authorization` for a bearer token and an OAuth access
    /// token, the person's header for an `api_key`.
    pub header: String,
    /// The credential.
    pub value: ToolSecret,
    /// When the lease ends. The relay does not use the credential after it.
    pub expires_at: Timestamp,
}

impl CredentialLease {
    /// The header of a bearer token.
    pub const AUTHORIZATION: &'static str = "Authorization";

    /// A lease of `value` in `header`.
    pub fn new(header: impl Into<String>, value: ToolSecret, expires_at: Timestamp) -> Self {
        CredentialLease {
            header: header.into(),
            value,
            expires_at,
        }
    }

    /// A lease of a bearer token (`value` is the token, without the scheme).
    pub fn bearer(value: ToolSecret, expires_at: Timestamp) -> Self {
        Self::new(Self::AUTHORIZATION, value, expires_at)
    }

    /// Whether the credential travels in `Authorization` (compared without regard to case), so the
    /// relay sends it as a bearer token.
    pub fn is_authorization(&self) -> bool {
        self.header.eq_ignore_ascii_case(Self::AUTHORIZATION)
    }

    /// Whether the lease has ended at `now`: `now` is its `expires_at` or later.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        now >= self.expires_at
    }
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialLease")
            .field("header", &self.header)
            .field("value", &self.value)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

// ----------------------------------------------------------------------------------- the OAuth

/// The `state` of an authorization that has begun: the one-time value that comes back with the
/// `code` on the redirect, by which the callback finds the person and the server. Opaque; `Debug`
/// shows `<redacted>` (it is short-lived and used once, but it is what binds a callback to a
/// person).
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthState(String);

impl OAuthState {
    /// Wraps the state a broker made, or one a callback carried.
    pub fn new(state: impl Into<String>) -> Self {
        OAuthState(state.into())
    }

    /// The text, for the URL it travels in and for the table that maps it to a person.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OAuthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// An authorization that has begun ([`CredentialBroker::begin_oauth`]).
///
/// `Debug` shows neither field: the URL carries the `state` and the PKCE challenge.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthStart {
    /// Where to send the person: the authorization server's endpoint with the PKCE `S256`
    /// challenge, the `resource` parameter, the scopes, the `redirect_uri` and the `state`.
    pub authorize_url: String,
    /// The `state` in that URL, which the redirect brings back.
    pub state: OAuthState,
}

impl fmt::Debug for OAuthStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthStart")
            .field("authorize_url", &"[redacted]")
            .field("state", &self.state)
            .finish()
    }
}

// ------------------------------------------------------------------------------------- errors

/// Why the broker did not give or keep a credential. None holds a credential, a token or a URL of
/// the person's; a `detail` is plain text about what was attempted.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CredentialError {
    /// The credential cannot be used and the person has to authorize or give a key again: the
    /// refresh failed, the server answered 401, a scope step-up is wanted, the grant was revoked, or
    /// there is no credential yet. The web shows it as a call to action, not as an error of the
    /// agent; the server's state becomes `needs_reauth`.
    #[error("the credential needs the person to sign in again")]
    NeedsReauth,
    /// The broker could not do it now: its secret store is down, the authorization server did not
    /// answer, the server's metadata could not be fetched, or the egress guard refused a URL. The
    /// server is `unavailable` for as long as this lasts.
    #[error("the credential broker is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// There is nothing to do this for: no such server for this owner, no such pending flow (an
    /// unknown or used `state`), or a kind that has no credential (`none`).
    #[error("no such server, flow or credential")]
    Unknown,
}

impl CredentialError {
    /// The broker could not do it now.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        CredentialError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` error; any other variant is returned
    /// unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            CredentialError::Unavailable { detail, .. } => CredentialError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            other @ (CredentialError::NeedsReauth | CredentialError::Unknown) => other,
        }
    }
}

impl Classify for CredentialError {
    fn class(&self) -> ErrorClass {
        match self {
            CredentialError::NeedsReauth => ErrorClass::Unauthenticated,
            CredentialError::Unavailable { .. } => ErrorClass::Transient,
            CredentialError::Unknown => ErrorClass::NotFound,
        }
    }
}

// ------------------------------------------------------------------------------------- the port

/// The credentials of people's MCP servers, as the API and the relay ask for them.
///
/// An implementation holds nothing between calls that a replica could not rebuild from the secret
/// store, hands out **no credential older than its lease**, keeps no secret in an error or a log
/// line, and applies the egress guard to every URL it fetches. Refresh is lazy and inside
/// [`lease`](Self::lease): one refresh at a time per `(owner, server)` in a process, and across
/// replicas [`SecretStore::rotate`](crate::SecretStore::rotate) settles it. Every call is the
/// owner's alone: another person's server is [`CredentialError::Unknown`].
pub trait CredentialBroker: Send + Sync + 'static {
    /// Keeps the credential the person typed for `server`: the bearer token, the API key, or an
    /// OAuth client secret of a client they registered themselves. `kind` says which. Replaces what
    /// was kept. Returns the reference the definition keeps.
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for no such server or for a kind with no credential
    /// ([`UserAuth::None`]); [`CredentialError::Unavailable`] when the secret store fails.
    fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        kind: &UserAuth,
        material: &ToolSecret,
    ) -> impl Future<Output = Result<CredentialRef, CredentialError>> + Send;

    /// Begins an OAuth authorization of `server` for `owner`: discovers the server's authorization
    /// server, registers a client if need be, makes the PKCE verifier (which stays with the broker)
    /// and returns the URL to send the person to and the `state` that comes back to `redirect`.
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for no such server or a kind that is not `oauth2`;
    /// [`CredentialError::Unavailable`] when discovery or the secret store fails or a URL is
    /// refused by the egress guard.
    fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &str,
    ) -> impl Future<Output = Result<OAuthStart, CredentialError>> + Send;

    /// Finishes the authorization that `state` belongs to, with the `code` the redirect brought:
    /// exchanges it (with the verifier and the `resource`) and keeps the tokens. `state` is **used
    /// once**. Returns the reference the definition keeps.
    ///
    /// `server` is in the call because the pending flow is kept under `(owner, server)`: the
    /// callback learns it from the table that maps `state` to the person who began (ADR 0046, the
    /// amendment of 2026-10-06).
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for a `state` that is not a pending flow of this owner and
    /// server, or was used; [`CredentialError::NeedsReauth`] when the authorization server refuses
    /// the code; [`CredentialError::Unavailable`] when it cannot be asked.
    fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
    ) -> impl Future<Output = Result<CredentialRef, CredentialError>> + Send;

    /// A fresh [`CredentialLease`] for one call to `server`, refreshed first if the access token is
    /// about to run out.
    ///
    /// # Errors
    /// [`CredentialError::NeedsReauth`] when there is no usable credential and the person must
    /// authorize again; [`CredentialError::Unknown`] for no such server;
    /// [`CredentialError::Unavailable`] when the secret store or the authorization server fails.
    fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> impl Future<Output = Result<CredentialLease, CredentialError>> + Send;

    /// Forgets the credential of `server` (and any pending flow), and revokes it upstream when the
    /// authorization server has a revocation endpoint. **Idempotent**: a server with nothing kept
    /// succeeds.
    ///
    /// # Errors
    /// [`CredentialError::Unavailable`] when the secret store fails.
    fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> impl Future<Output = Result<(), CredentialError>> + Send;
}
