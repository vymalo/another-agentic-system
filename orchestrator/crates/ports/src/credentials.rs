//! The credential broker port: how the orchestrator stores, obtains and drops the credentials of a
//! person's MCP servers (ADR 0046, the amendment of 2026-10-11, ADR 0009).
//!
//! A person's server ([`UserToolServer`](crate::UserToolServer)) holds no secret. The credential
//! (a bearer token, an API key, or the tokens of an OAuth authorization) is the **broker's**: the
//! orchestrator's own, one implementation written once (`orch-credential-broker`),
//! which does the OAuth work (discovery, PKCE `S256`, the `resource` parameter, client
//! registration, refresh) and keeps what it must in a [`SecretStore`](crate::SecretStore). This
//! module is the port the API and the relay are written against:
//!
//! | Call | Does |
//! |---|---|
//! | [`store`](CredentialBroker::store) | Keeps a token, a key or a client secret the person typed; returns its [`CredentialRef`]. |
//! | [`begin_oauth`](CredentialBroker::begin_oauth) | Starts an authorization: the URL to send the person to, the `state` that comes back, and when the flow ends. |
//! | [`complete_oauth`](CredentialBroker::complete_oauth) | Finishes it with the `code` the redirect brought. |
//! | [`lease`](CredentialBroker::lease) | A **fresh** credential for one call to the server, short-lived; refreshed if need be. |
//! | [`revoke`](CredentialBroker::revoke) | Forgets the credential, and revokes it upstream where the server can. |
//!
//! The port carries the key of a server, `(owner, server id)`, and not its URL, kind, scopes or
//! client id: an implementation reads those from the definition in the
//! [`UserToolServerStore`](crate::UserToolServerStore) it is composed with, which stays the one
//! place that says what a server is. Every URL it then fetches (the server's, its resource
//! metadata, the authorization server's metadata and token endpoint) goes through the
//! [`EgressPolicy`](crate::EgressPolicy).
//!
//! # A credential belongs to the URL it was issued for
//!
//! A bearer token is a bearer of nothing but the server it was issued for (the OAuth `resource`,
//! RFC 8707; the MCP specification forbids sending a token to another server). So the broker keeps
//! the server's **normal-form URL** ([`UserToolServer::url`](crate::UserToolServer::url)) inside the
//! sealed material, whenever it stores a credential (`store`, `complete_oauth`), and
//! [`lease`](CredentialBroker::lease) compares it with the URL of the definition as it is now: **a
//! mismatch is [`CredentialError::NeedsReauth`]**, the credential is not lent, and the person gives a
//! key or authorizes again for the new URL. Without this an edit that moves a server to another URL,
//! or a definition replaced under the same id, would send the old token to a host the person never
//! gave it to. (The API also revokes the credential when it edits the URL; this holds when it does
//! not, or when two replicas race.) The conformance macro of the broker
//! (`credential_broker_conformance!`) has cases for it.
//!
//! # The authorization server that answers is the one that was asked (RFC 9207)
//!
//! A person's server names its authorization server in its own metadata, so a person who adds
//! several servers talks to several authorization servers, some of them a stranger's. That is the
//! situation of the **mix-up attack** (RFC 9700, section 4.4): the browser is sent to one
//! authorization server and comes back with a `code` another one issued, and a client that sends
//! that `code` to the first one's token endpoint has handed the second one's grant to a stranger.
//! RFC 9700 (section 2.1) makes a defence required for a client that can talk to more than one
//! authorization server, and the one it names is the `iss` parameter of RFC 9207.
//!
//! **The broker checks it, not the API.** [`begin_oauth`](CredentialBroker::begin_oauth) keeps the
//! issuer identifier of the authorization server it sent the person to, and whether that server
//! advertises `authorization_response_iss_parameter_supported`, inside the sealed pending flow;
//! [`OAuthStart::issuer`] says which one, for the API to show. The callback hands the `iss` it
//! received, as it received it, to [`complete_oauth`](CredentialBroker::complete_oauth), which
//! applies RFC 9207, section 2.4, before it touches anything:
//!
//! | The server advertised support | The callback has `iss` | Result |
//! |---|---|---|
//! | yes | equal to the issuer (simple string comparison) | accepted |
//! | yes | not equal | refused |
//! | yes | absent | refused (a MUST) |
//! | no | present | refused (a SHOULD of the RFC, which local policy may relax; this broker does not) |
//! | no | absent | accepted (the server takes no part in RFC 9207; the defence is then the `state` alone) |
//!
//! A refused callback is [`CredentialError::Unknown`] (the callback is not one of this flow), and it
//! **spends nothing**: the `code` is not sent anywhere, and the flow stays as it was, so the
//! person's own callback still completes it.
//!
//! # A secret appears only
//!
//! in the argument of [`store`](CredentialBroker::store) and [`complete_oauth`](CredentialBroker::complete_oauth)
//! (a [`ToolSecret`], which prints `<redacted>` and is wiped when dropped), in a
//! [`CredentialLease`] on its way to one request to the server, and sealed in the secret store. It is
//! in no event, export, frame, log line, step input or output, error or agent file; no error here
//! holds one.

use std::fmt;
use std::future::Future;

use jiff::Timestamp;
use orch_core::{BoxError, Classify, ErrorClass, UserId};
use subtle::ConstantTimeEq;
use url::{Host, Url};

use crate::egress::MAX_URL_BYTES;
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

/// What the relay is lent for one call to a person's server: the credential, where it goes, and when
/// the loan ends. A closed enum, so a kind of lease added later fails to compile wherever one is
/// matched.
///
/// A lease is **fresh** (the broker refreshed the token if it was about to run out) and **short**:
/// the relay asks for one per `tools/list` and per `tools/call` and keeps none, so a token is never
/// older than the broker says.
///
/// - [`Bearer`](CredentialLease::Bearer): a bearer token for `Authorization`, **without** the
///   `Bearer ` scheme; the relay hands it to
///   [`ToolServerEndpoint::with_bearer`](crate::ToolServerEndpoint::with_bearer), which sends
///   `Authorization: Bearer <token>`. For the `bearer` and `oauth2` kinds.
/// - [`Header`](CredentialLease::Header): the value to send as it is in the person's own header
///   ([`ToolServerEndpoint::with_header`](crate::ToolServerEndpoint::with_header)). For `api_key`.
///   The name is checked as an `api_key` header is, by [`CredentialLease::header`].
///
/// **No `PartialEq`, no `Eq`, no `Clone`**: nothing compares credentials with `==` (it is not a
/// constant-time comparison), and a lease is used once, so it is moved, never copied. `Debug` shows
/// the kind, the header and the time and never the value.
#[derive(Debug)]
pub enum CredentialLease {
    /// A bearer token for `Authorization`.
    #[non_exhaustive]
    Bearer {
        /// The token, without the `Bearer ` scheme.
        token: ToolSecret,
        /// When the lease ends. The relay does not use the credential after it.
        expires_at: Timestamp,
    },
    /// A value in a header the person named.
    #[non_exhaustive]
    Header {
        /// The header's name (checked: never `Authorization`, `Host`, `Content-Length`, ...).
        name: String,
        /// The value, to send as it is.
        value: ToolSecret,
        /// When the lease ends. The relay does not use the credential after it.
        expires_at: Timestamp,
    },
}

impl CredentialLease {
    /// The header of a bearer token.
    pub const AUTHORIZATION: &'static str = "Authorization";

    /// A lease of a bearer token (`token` is the token, without the scheme).
    pub fn bearer(token: ToolSecret, expires_at: Timestamp) -> Self {
        CredentialLease::Bearer { token, expires_at }
    }

    /// A lease of `value` in the header `name`, which must be a header name a person may use for an
    /// `api_key` ([`UserAuth::api_key`]).
    ///
    /// # Errors
    /// [`DefinitionError::BadHeader`], [`DefinitionError::ReservedHeader`].
    pub fn header(
        name: &str,
        value: ToolSecret,
        expires_at: Timestamp,
    ) -> Result<Self, DefinitionError> {
        UserAuth::api_key(name)?;
        Ok(CredentialLease::Header {
            name: name.to_owned(),
            value,
            expires_at,
        })
    }

    /// The header the credential travels in: `Authorization` for a bearer token, the person's
    /// header for a key.
    pub fn header_name(&self) -> &str {
        match self {
            CredentialLease::Bearer { .. } => Self::AUTHORIZATION,
            CredentialLease::Header { name, .. } => name,
        }
    }

    /// The credential itself: the bare token of a bearer lease, the value of a header lease.
    pub fn secret(&self) -> &ToolSecret {
        match self {
            CredentialLease::Bearer { token, .. } => token,
            CredentialLease::Header { value, .. } => value,
        }
    }

    /// When the lease ends.
    pub fn expires_at(&self) -> Timestamp {
        match self {
            CredentialLease::Bearer { expires_at, .. }
            | CredentialLease::Header { expires_at, .. } => *expires_at,
        }
    }

    /// Whether the credential travels in `Authorization`, so the relay sends it as a bearer token.
    pub fn is_bearer(&self) -> bool {
        matches!(self, CredentialLease::Bearer { .. })
    }

    /// Whether the lease has ended at `now`: `now` is its `expires_at` or later.
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        now >= self.expires_at()
    }
}

// ----------------------------------------------------------------------------------- the OAuth

/// The shortest [`OAuthState`], in bytes: 16 characters, 96 bits when they are base64url of random
/// bytes. A `state` is what binds a callback to the person who began, so it is not guessable.
pub const MIN_OAUTH_STATE_BYTES: usize = 16;

/// The longest [`OAuthState`], in bytes.
pub const MAX_OAUTH_STATE_BYTES: usize = 128;

/// Not an [`OAuthState`]: 16 to 128 characters of letters, digits, `-`, `.`, `_` and `~`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "not an OAuth state: {MIN_OAUTH_STATE_BYTES} to {MAX_OAUTH_STATE_BYTES} letters, digits, -, ., _ or ~"
)]
pub struct OAuthStateError;

impl Classify for OAuthStateError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// The `state` of an authorization that has begun: the one-time value that comes back with the
/// `code` on the redirect, by which the callback finds the person and the server.
///
/// It is checked when it is made, **whether the broker made it or a callback carried it** (a
/// callback's query is a stranger's text): 16 to 128 characters of the unreserved set of RFC 3986
/// (letters, digits, `-`, `.`, `_`, `~`), so it travels in a URL as it is and a short or odd one is
/// refused before it is looked up. Two states are compared in **constant time**. `Debug` shows
/// `<redacted>` (it is short-lived and used once, but it is what binds a callback to a person).
#[derive(Clone)]
pub struct OAuthState(String);

impl OAuthState {
    /// Wraps the state a broker made, or one a callback carried.
    ///
    /// # Errors
    /// [`OAuthStateError`] for a text that is too short or too long or has another character.
    pub fn new(state: &str) -> Result<Self, OAuthStateError> {
        let unreserved =
            |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if !(MIN_OAUTH_STATE_BYTES..=MAX_OAUTH_STATE_BYTES).contains(&state.len())
            || !state.bytes().all(unreserved)
        {
            return Err(OAuthStateError);
        }
        Ok(OAuthState(state.to_owned()))
    }

    /// The text, for the URL it travels in and for the table that maps it to a person.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl PartialEq for OAuthState {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

impl Eq for OAuthState {}

impl fmt::Debug for OAuthState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Not a redirect URI: `https`, or `http` on the loopback address `127.0.0.1` or `[::1]`, with a
/// host, a port that is not 0, no user information and no fragment, in at most 2 KiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a redirect URI: https, or http on 127.0.0.1 or [::1], with no fragment")]
pub struct RedirectUriError;

impl Classify for RedirectUriError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// The `redirect_uri` of an OAuth authorization: where the authorization server sends the person
/// back with the `code` and the `state`.
///
/// The rules are those of the MCP authorization specification and OAuth for native apps (RFC 8252,
/// section 7.3 and 8.3): **`https`**, or **`http` on a loopback IP literal** (`127.0.0.1` or `[::1]`,
/// with the port the app listens on; `localhost` is refused, the name can be pointed elsewhere), and
/// **no fragment** (RFC 6749, section 3.1.2). No user information. Nothing else: a custom scheme is
/// not accepted here.
///
/// It is the orchestrator's own URL (the callback endpoint) or the desktop app's loopback listener,
/// never one a stranger names, and nothing here fetches it. The broker sends it in the
/// authorization request and again, byte for byte, in the token request: [`as_str`](Self::as_str) is
/// the normal form the URL parser writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectUri(String);

impl RedirectUri {
    /// Reads and checks `text`.
    ///
    /// # Errors
    /// [`RedirectUriError`].
    pub fn parse(text: &str) -> Result<Self, RedirectUriError> {
        if text.len() > MAX_URL_BYTES {
            return Err(RedirectUriError);
        }
        let url = Url::parse(text).map_err(|_| RedirectUriError)?;
        if url.as_str().len() > MAX_URL_BYTES
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port() == Some(0)
        {
            return Err(RedirectUriError);
        }
        match (url.scheme(), url.host()) {
            ("https", Some(_)) => {}
            ("http", Some(Host::Ipv4(ip))) if ip == std::net::Ipv4Addr::LOCALHOST => {}
            ("http", Some(Host::Ipv6(ip))) if ip == std::net::Ipv6Addr::LOCALHOST => {}
            _ => return Err(RedirectUriError),
        }
        Ok(RedirectUri(url.as_str().to_owned()))
    }

    /// The URI in its normal form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RedirectUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An authorization that has begun ([`CredentialBroker::begin_oauth`]).
///
/// `Debug` shows neither the URL nor the state: the URL carries the `state` and the PKCE challenge.
/// It does show the issuer, which is a public identifier.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthStart {
    /// Where to send the person: the authorization server's endpoint with the PKCE `S256`
    /// challenge, the `resource` parameter, the scopes, the `redirect_uri` and the `state`.
    pub authorize_url: String,
    /// The `state` in that URL, which the redirect brings back.
    pub state: OAuthState,
    /// When the pending flow ends: [`complete_oauth`](CredentialBroker::complete_oauth) with this
    /// `state` after it is [`Unknown`](CredentialError::Unknown). The API keeps its own table of
    /// `state` to person (ADR 0046, the amendment of 2026-10-06) and expires its rows at this time.
    pub expires_at: Timestamp,
    /// The issuer identifier (RFC 8414) of the authorization server the person is sent to: an
    /// `https` URL with no query or fragment, as the server's own metadata gave it. The API shows
    /// its host next to the consent button, so that a person can see which server a stranger's
    /// MCP server chose. The callback is held to it (see the module's docs).
    pub issuer: String,
}

impl fmt::Debug for OAuthStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthStart")
            .field("authorize_url", &"[redacted]")
            .field("state", &self.state)
            .field("expires_at", &self.expires_at)
            .field("issuer", &self.issuer)
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
    /// refresh failed, the server answered 401, a scope step-up is wanted, the grant was revoked
    /// (the credential was removed, or never given), or it was issued for another URL than the
    /// server's now (see the module's docs). The web shows it as a call to action, not as an error
    /// of the agent; the server's state becomes `needs_reauth`.
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
    /// unknown, used or expired `state`), or a kind that has no credential (`none`). A server that
    /// has a kind with a credential, and has no credential now, is not this: it is
    /// [`NeedsReauth`](Self::NeedsReauth).
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
/// line, and applies the egress guard to every URL it fetches.
///
/// **Refresh is lazy and inside [`lease`](Self::lease), and a refresh token is presented once.**
/// An authorization server that rotates refresh tokens treats one presented a second time as
/// stolen and revokes the grant (RFC 9700, sections 2.2.2 and 4.14.2), so two replicas must not
/// both ask. In a process one caller per `(owner, server)` asks and the others are told how it came
/// out, whether it kept its tokens or failed. Across replicas the one that asks first **claims**
/// the refresh in the store, with [`SecretStore::rotate`](crate::SecretStore::rotate), before it
/// asks, for as long as one request can take; a replica that finds the claim lends the access
/// token while it has not expired, and otherwise waits for the tokens to be written, taking over
/// only when the claim has run out. A refresh that fails ends its claim. Every call is the
/// owner's alone: another person's server is [`CredentialError::Unknown`]. It reads the
/// definition (kind, URL, scopes, client id) from the [`UserToolServerStore`](crate::UserToolServerStore)
/// it is composed with, and keeps the credential bound to the URL it was issued for (see the
/// module's docs).
pub trait CredentialBroker: Send + Sync + 'static {
    /// Keeps the credential the person typed for `server`: the bearer token, the API key, or an
    /// OAuth client secret of a client they registered themselves. The definition's kind says
    /// which. Replaces what was kept. Returns the reference the definition keeps.
    ///
    /// The broker keeps the definition's normal-form URL
    /// ([`UserToolServer::url`](crate::UserToolServer::url)) with the credential, inside the sealed
    /// material, so that [`lease`](Self::lease) can tell a credential issued for another URL.
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for no such server or for a kind with no credential
    /// ([`UserAuth::None`]); [`CredentialError::Unavailable`] when the secret store fails.
    fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        material: &ToolSecret,
    ) -> impl Future<Output = Result<CredentialRef, CredentialError>> + Send;

    /// Begins an OAuth authorization of `server` for `owner`: discovers the server's authorization
    /// server, registers a client if need be, makes the PKCE verifier (which stays with the broker)
    /// and returns the URL to send the person to, the `state` that comes back to `redirect`, and
    /// when the flow ends ([`OAuthStart::expires_at`]). A flow already pending for the server is
    /// replaced: its `state` is no longer one.
    ///
    /// **A flow that is abandoned, or that comes back with `error=`, needs no call.** The pending
    /// flow (the verifier, kept under [`SecretServer::pending_flow`](crate::SecretServer::pending_flow))
    /// simply expires at `expires_at`: the broker forgets it the next time it meets it (a
    /// `complete_oauth` after the time, the next `begin_oauth`, a `revoke`), and until then it can
    /// only be used once. The callback of an `error=` shows the person what the authorization server
    /// said and calls nothing; the server stays `added` or `needs_reauth` and the person begins again.
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for no such server or a kind that is not `oauth2`;
    /// [`CredentialError::Unavailable`] when discovery or the secret store fails or a URL is
    /// refused by the egress guard.
    fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &RedirectUri,
    ) -> impl Future<Output = Result<OAuthStart, CredentialError>> + Send;

    /// Finishes the authorization that `state` belongs to, with the `code` the redirect brought:
    /// exchanges it (with the verifier and the `resource`) and keeps the tokens, bound to the URL,
    /// the scopes and the client id the flow began with. `state` is **used once**. Returns the
    /// reference the definition keeps.
    ///
    /// `server` is in the call because the pending flow is kept under `(owner, server)`: the
    /// callback learns it from the table that maps `state` to the person who began (ADR 0046, the
    /// amendment of 2026-10-06).
    ///
    /// `iss` is the `iss` parameter of the redirect **as it came**, decoded, and `None` when the
    /// redirect had none. The broker checks it against the issuer of the authorization server the
    /// person was sent to, by RFC 9207, section 2.4 (the table in the module's docs): a mismatch, an
    /// absent `iss` from a server that advertised it and a present one from a server that did not
    /// are all refused. The API passes it on and does not interpret it. A callback that carries
    /// `error=` is not a call (see [`begin_oauth`](Self::begin_oauth)), and what it says is shown as
    /// unverified text: without `iss` checked, "the error does not come from the intended
    /// authorization server" is possible (RFC 9207, section 2.4).
    ///
    /// # Errors
    /// [`CredentialError::Unknown`] for a `state` that is not a pending flow of this owner and
    /// server, or was used, or has expired, **and for a callback whose `iss` is refused**: in both
    /// cases nothing is sent to anyone and the flow is left as it was;
    /// [`CredentialError::NeedsReauth`] when the authorization server refuses the code;
    /// [`CredentialError::Unavailable`] when it cannot be asked.
    fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
        iss: Option<&str>,
    ) -> impl Future<Output = Result<CredentialRef, CredentialError>> + Send;

    /// A fresh [`CredentialLease`] for one call to `server`, refreshed first if the access token is
    /// about to run out.
    ///
    /// It is lent only if the credential was issued for the URL the definition has **now**; if the
    /// URL kept with the credential is not the definition's, the credential is not lent and the
    /// answer is `NeedsReauth` (see the module's docs).
    ///
    /// For a server whose kind has no credential ([`UserAuth::None`]) there is nothing to lend: the
    /// answer is [`CredentialError::Unknown`], as for `store`. The relay does not ask for a lease
    /// for a server that does not [need one](UserAuth::needs_credential); it connects with none.
    ///
    /// # Errors
    /// [`CredentialError::NeedsReauth`] when the server needs a credential and has no usable one:
    /// it was **revoked** (or removed), never given, expired and could not be refreshed, or was
    /// issued for another URL; the person must authorize or give a key again.
    /// [`CredentialError::Unknown`] for no such server (or one of kind `none`) and for nothing
    /// else; [`CredentialError::Unavailable`] when the secret store or the authorization server
    /// fails.
    fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> impl Future<Output = Result<CredentialLease, CredentialError>> + Send;

    /// Forgets the credential of `server` (and any pending flow), and revokes it upstream when the
    /// authorization server has a revocation endpoint. **Idempotent**: a server with nothing kept
    /// succeeds.
    ///
    /// It reads the definition to find where to revoke (the server's URL leads to its authorization
    /// server), so it runs **before** the definition is deleted: deleting a server is "revoke, then
    /// delete" ([`UserToolServerStore::delete`](crate::UserToolServerStore::delete)). A definition
    /// deleted first leaves a sealed credential that no definition names. After `revoke`,
    /// [`lease`](Self::lease) is [`CredentialError::NeedsReauth`] while the definition exists.
    ///
    /// # Errors
    /// [`CredentialError::Unavailable`] when the secret store fails; the caller does not delete the
    /// definition then.
    fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> impl Future<Output = Result<(), CredentialError>> + Send;
}
