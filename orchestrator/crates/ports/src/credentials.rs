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
//! # A flow is for the definition it began with
//!
//! [`begin_oauth`](CredentialBroker::begin_oauth) keeps the definition's URL, scopes and client id
//! inside the pending flow, and [`complete_oauth`](CredentialBroker::complete_oauth) compares them
//! with the definition it reads when the callback comes: a flow whose URL, scopes (as a set) or client
//! id differ is **ended** and is [`CredentialError::Unknown`], with the code sent to nobody. A rename
//! is not a difference. The caller edits a definition by writing it and **then** revoking (which
//! deletes the pending flow), so a callback can come between the two; without this the code obtained
//! for the old URL would be kept as the credential of the new definition. The conformance macro has
//! cases for it (`a_flow_is_for_the_definition_it_began_with` and
//! `a_rename_or_a_reordering_of_the_scopes_does_not_end_a_flow`).
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
use orch_core::{AgentId, BoxError, Classify, ErrorClass, UserId};
use subtle::ConstantTimeEq;
use url::{Host, Url};

use crate::egress::{EgressRefusal, MAX_URL_BYTES};
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

    /// The refusal of the [`EgressPolicy`](crate::EgressPolicy) that made this an
    /// [`Unavailable`](Self::Unavailable), when that is why: the broker keeps the guard's refusal as
    /// the source of the error when the guard stopped a URL it was about to fetch (the server's
    /// metadata, the authorization server's, its token endpoint). A person is shown
    /// [`EgressRefusal::public_reason`], never the precise [`rule`](EgressRefusal::rule) (ADR 0046,
    /// departure 17). `None` for any other error.
    pub fn egress_refusal(&self) -> Option<EgressRefusal> {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(self);
        while let Some(e) = source {
            if let Some(refusal) = e.downcast_ref::<EgressRefusal>() {
                return Some(*refusal);
            }
            source = e.source();
        }
        None
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
    /// It **does not read the definition**: a grant keeps the endpoints its authorization server
    /// named, sealed with the tokens (ADR 0046, departure 21), so a revocation works for a definition
    /// that is already gone or has just been rewritten. The order against the definition is the
    /// caller's. Deleting a server is "revoke, then delete"
    /// ([`UserToolServerStore::delete`](crate::UserToolServerStore::delete)): a definition deleted
    /// first leaves a sealed credential that no definition names, and a revoke that fails leaves the
    /// definition to try again. An edit that moves a server writes the definition first (a
    /// compare-and-swap) and revokes after: a write that fails, a race that is lost or a request that
    /// is dropped has then revoked nothing, where a revoke done first would leave a `connected`
    /// server whose credential is gone. After `revoke`, [`lease`](Self::lease) is
    /// [`CredentialError::NeedsReauth`] while the definition exists.
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

/// A credential broker that **keeps credentials**: a real one, over a secret store that seals. It is
/// implemented by the brokers that do and never by [`NoBroker`]. [`UserTools::full`](crate::UserTools::full)
/// asks for it, so a bundle cannot claim [`UserToolsMode::Full`](crate::UserToolsMode::Full) over a
/// broker that refuses every call: the mode of a bundle is what its constructor built, and not a second
/// statement that may disagree with it.
///
/// It adds nothing to [`CredentialBroker`]: no port signature changed for it.
pub trait KeepsCredentials: CredentialBroker {}

/// The broker of a deployment that keeps no credentials (`secrets` absent from the configuration, or
/// `userToolServers` off): every call is [`CredentialError::Unavailable`] with a sentence that says
/// why and holds no owner, server, state or secret. With it only servers that want no authentication
/// can exist ([`UserAuth::None`]); the caller does not ask it about those.
///
/// It is the broker, and not a real one over [`NoSecrets`](crate::NoSecrets), because a real broker
/// would do its network work first (discovery, registration) and fail only at the store. The model is
/// [`NoArtifacts`](crate::NoArtifacts); it does not pass the conformance suite, which is for a broker
/// that keeps.
///
/// A server that already holds a credential when the section is taken away from the configuration is
/// `unavailable` for as long as it is gone. `revoke` is refused too, because the port is honest: nothing
/// was revoked. **A caller does not call it in that deployment** ([`UserToolsMode::WithoutCredentials`](crate::UserToolsMode::WithoutCredentials)
/// does not keep credentials, see [`UserToolPorts`](crate::UserToolPorts)): it deletes the definition,
/// which a person must always be able to do, and the sealed row, if there is one, stays in the secret
/// store, unreachable without its key and never lent (the broker binds a credential to the
/// definition's URL and to the id made again, and the ids are random).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBroker;

impl NoBroker {
    fn refuse<T>() -> Result<T, CredentialError> {
        Err(CredentialError::unavailable(
            "no credentials are kept (the secrets section is absent from the configuration, or \
             userToolServers is off)",
        ))
    }
}

impl CredentialBroker for NoBroker {
    async fn store(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        Self::refuse()
    }

    async fn begin_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        Self::refuse()
    }

    async fn complete_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _state: &OAuthState,
        _code: &ToolSecret,
        _iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        Self::refuse()
    }

    async fn lease(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        Self::refuse()
    }

    async fn revoke(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        Self::refuse()
    }
}

// ---------------------------------------------------------------- the run lease (B1, ADR 0084)

/// A run's id: the root run's — the A2A task id the orchestrator binds (ADR 0067 decision 1), the
/// same value `X-Run-Id` carries. Opaque to the broker: no format is assumed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(String);

impl RunId {
    /// The id as the orchestrator holds it.
    pub fn new(id: impl Into<String>) -> Self {
        RunId(id.into())
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who calls the run-lease route. The orchestrator binds and unbinds a run; the agent the run is
/// bound to leases its credential. A lookup from any other identity is refused (ADR 0084).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCaller {
    /// The orchestrator itself: it binds a run, unbinds it, and may read the run's lease.
    Orchestrator,
    /// The agent the run is bound to. It is named **by** the binding and is **never** enough for a
    /// lease: an agent is refused the lease of its own run, so a reachable agent holds no key
    /// (ADR 0084 decision 2, amended 2026-11-24).
    Agent(AgentId),
    /// The injector: the process that adds the run's model credential to a request. It reads the
    /// lease and does nothing else — it binds nothing and unbinds nothing (ADR 0084 decision 2,
    /// amended 2026-11-24).
    Injector,
}

/// What the orchestrator binds at run creation: the run, the agent it belongs to, the person it
/// spends for, the model endpoint it may use (when the binding names one), and its deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBinding {
    /// The root run's id.
    pub run: RunId,
    /// The agent the run is bound to.
    pub agent: AgentId,
    /// The person the run spends for.
    pub person: UserId,
    /// The model endpoint label the lease is for, when the binding names one: the `endpoint` of the
    /// choice the live card maps the run's pick to (ADR 0084, the status note of 2026-11-24). `None`
    /// when the card names none, and the lookup then answers `unavailable` (fail closed).
    pub model: Option<String>,
    /// When the binding ends; the broker caps it.
    pub expires_at: Timestamp,
}

/// What the injector gets: the model endpoint's bearer token, the ledger account whose call it is
/// (the run's person), and the moment the token stops being valid. At most 30 s, never past the
/// binding's deadline, `no-store`, and never logged. No `Clone`: a lease is used once and moved.
pub struct RunLease {
    token: ToolSecret,
    account: UserId,
    expires_at: Timestamp,
}

/// `Debug` shows the expiry and nothing of the token or the account.
impl std::fmt::Debug for RunLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunLease")
            .field("token", &"<redacted>")
            .field("account", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl RunLease {
    /// A lease of `token` until `expires_at`, for the account `account`: the run's person, a
    /// `UserId`, which is the key the usage ledger is kept by (`orch_core::UsageRow.owner`,
    /// `crates/core/src/usage_record.rs`; the column `usage_tasks.owner` of migration `0023`; ADR
    /// 0065). That person id and not a provider account or an account number of the ledger service:
    /// the orchestrator holds no other identity for the person. The account is returned to the
    /// injector beside the key and goes in no log line, no error and no message to an agent.
    pub fn bearer(token: ToolSecret, account: UserId, expires_at: Timestamp) -> Self {
        RunLease {
            token,
            account,
            expires_at,
        }
    }

    /// The ledger account the run spends for: the run's person (`UserId`), the key the ledger keeps
    /// the rows by.
    pub fn account(&self) -> &UserId {
        &self.account
    }

    /// The token, without the `Bearer ` scheme.
    pub fn token(&self) -> &ToolSecret {
        &self.token
    }

    /// When the token stops being valid.
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }
}

/// Why a binding or a lease was refused. The slugs are the contract's (`run_not_bound`,
/// `lease_expired`, `unavailable`); `Conflict` is a different binding for a run already bound.
#[derive(Debug)]
pub enum RunLeaseError {
    /// No live binding for the run (unknown, unbound, or revoked).
    NotBound,
    /// The binding exists and has ended.
    Expired,
    /// The run is already bound to something else; a run is bound once.
    Conflict,
    /// The store or the credential source failed; the caller fails closed.
    Unavailable(String),
}

impl std::fmt::Display for RunLeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunLeaseError::NotBound => f.write_str("run_not_bound"),
            RunLeaseError::Expired => f.write_str("lease_expired"),
            RunLeaseError::Conflict => f.write_str("run_already_bound"),
            RunLeaseError::Unavailable(reason) => write!(f, "unavailable: {reason}"),
        }
    }
}

impl std::error::Error for RunLeaseError {}

/// The broker's first part (B1, ADR 0084): the binding the orchestrator makes at run creation, and
/// the lease the injector asks for, answered to the orchestrator and the injector and never to an
/// agent (ADR 0084 decision 2, amended 2026-11-24).
pub trait RunLeases: Send + Sync + 'static {
    /// Binds `binding`. Idempotent for the same binding; [`RunLeaseError::Conflict`] for a different
    /// one on the same run.
    fn bind(&self, binding: RunBinding) -> impl Future<Output = Result<(), RunLeaseError>> + Send;

    /// Removes the run's binding; idempotent, and a no-op for a run that was never bound.
    fn unbind(&self, run: &RunId) -> impl Future<Output = Result<(), RunLeaseError>> + Send;

    /// Leases the run's model credential to `caller`, which must be the bound agent (or, for the
    /// orchestrator, refused: the orchestrator binds and unbinds, it does not lease).
    fn lease(
        &self,
        run: &RunId,
        caller: &RunCaller,
    ) -> impl Future<Output = Result<RunLease, RunLeaseError>> + Send;
}

/// The binding store the broker keeps: one row per run, and the same binding written again is
/// idempotent (ADR 0084 decision 1).
pub trait RunBindingStore: Send + Sync + 'static {
    /// Writes `binding`; [`RunLeaseError::Conflict`] when the run is bound to something else.
    fn upsert(
        &self,
        binding: &RunBinding,
    ) -> impl Future<Output = Result<(), RunLeaseError>> + Send;

    /// Removes the run's binding; idempotent.
    fn delete(&self, run: &RunId) -> impl Future<Output = Result<(), RunLeaseError>> + Send;

    /// The run's binding, if it has one.
    fn get(
        &self,
        run: &RunId,
    ) -> impl Future<Output = Result<Option<RunBinding>, RunLeaseError>> + Send;
}

/// The deployment's model credentials by model name: the bearer token the injector adds upstream
/// (ADR 0084: the deployment's provider key, never the person's).
pub trait ModelCredentials: Send + Sync + 'static {
    /// The token for `model`, if the deployment has one.
    fn bearer(
        &self,
        model: &str,
    ) -> impl Future<Output = Result<Option<ToolSecret>, RunLeaseError>> + Send;
}

/// The orchestrator's side of ADR 0084 decisions 1 and 6: the binding it makes when a run starts
/// and removes when the run ends. The broker's side of the same wire is [`RunLeases`]; a deployment
/// with no broker behind it uses [`NoRunBindings`], which accepts every call and does nothing.
///
/// A binding is not a secret: it names the run, the agent, the person and the model endpoint, and
/// carries a deadline. The value the lease returns is never part of it.
pub trait RunBindings: Send + Sync + 'static {
    /// Binds `binding`; the broker treats the same binding sent again as idempotent and a different
    /// one for the same run as [`RunBindingError::Conflict`]. The broker caps the deadline.
    fn bind(&self, binding: RunBinding)
    -> impl Future<Output = Result<(), RunBindingError>> + Send;

    /// Removes the run's binding; a run that was never bound is not an error.
    fn unbind(&self, run: &RunId) -> impl Future<Output = Result<(), RunBindingError>> + Send;
}

/// Why a bind or an unbind did not happen. It holds no URL, no token and no body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunBindingError {
    /// The broker could not be reached, or did not answer in time. A run that cannot be bound goes
    /// on today (the failure is logged; S9 makes it park the run).
    Unreachable,
    /// The run is already bound to another agent (`409 run_already_bound`): a run is bound once.
    Conflict,
    /// The broker answered with a status that is neither a success nor a conflict.
    Refused(u16),
}

impl std::fmt::Display for RunBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunBindingError::Unreachable => f.write_str("the broker is unreachable"),
            RunBindingError::Conflict => f.write_str("the run is already bound to another agent"),
            RunBindingError::Refused(status) => write!(f, "the broker refused the call ({status})"),
        }
    }
}

impl std::error::Error for RunBindingError {}

/// A deployment with no broker behind it: every bind and unbind succeeds and nothing is stored.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRunBindings;

impl RunBindings for NoRunBindings {
    async fn bind(&self, _binding: RunBinding) -> Result<(), RunBindingError> {
        Ok(())
    }

    async fn unbind(&self, _run: &RunId) -> Result<(), RunBindingError> {
        Ok(())
    }
}
