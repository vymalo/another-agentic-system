//! The definitions of a person's own MCP servers, and the port that keeps them (ADR 0046,
//! decision 2, ADR 0009).
//!
//! An agent's own MCP servers are fixed; a person adds theirs on top, and the orchestrator relays
//! their tools to the agent of the thread they attach them to (`thread-tools/v1`). What is kept
//! here is the **definition** of such a server, per person: its id, name, URL, the kind of
//! authentication it wants ([`UserAuth`]), its [state](UserToolServerState) and the
//! [`CredentialRef`] of its credential. **A definition holds no secret.** The credential is the
//! broker's ([`CredentialBroker`](crate::CredentialBroker)), sealed in a
//! [`SecretStore`](crate::SecretStore). No event of the log records a definition: it is the
//! person's, not the thread's.
//!
//! # The URL of a definition is one the guard accepted
//!
//! [`UserToolServer::new`] takes an [`EgressTarget`], which only [`EgressPolicy::check_url`] makes,
//! so a definition that exists holds a URL the guard read and wrote in its normal form: an unchecked
//! string cannot be stored. A row an adapter reads back goes through
//! [`UserToolServer::from_stored`], which checks the URL again (the rules of the URL itself, not the
//! deployment's lists: a person must be able to list and delete a server that today's lists refuse,
//! and the connection applies the lists every time). The URL never appears in a `Debug`: only its
//! scheme, host and port do, because a path can carry a secret.
//!
//! # The port, and why it is not an upsert
//!
//! The port is [`UserToolServerStore`]: [`insert`](UserToolServerStore::insert),
//! [`get`](UserToolServerStore::get), [`list`](UserToolServerStore::list),
//! [`replace`](UserToolServerStore::replace), [`set_state`](UserToolServerStore::set_state) and
//! [`delete`](UserToolServerStore::delete) by `(owner, id)`. Everything is the owner's: another
//! person's definition is simply not there, for every call. At most [`MAX_USER_TOOL_SERVERS`] per
//! owner, **enforced by the store** with [`UserToolServerStoreError::TooMany`], so every adapter
//! refuses the same way.
//!
//! Several writers touch one definition: the API (an edit, a delete), the broker (a state, a
//! credential reference) and the relay (a state after a 401). A write that is an unconditional
//! upsert would let an edit that read the definition revert what the broker wrote since, and would
//! bring a deleted server back. So every write that changes an existing definition is a
//! **compare and swap** on a [`ServerRevision`] the store keeps and a read returns
//! ([`StoredServer`]): a stale one is [`Conflict`](UserToolServerStoreError::Conflict) (read again
//! and decide again), an absent one is [`NotFound`](UserToolServerStoreError::NotFound) (a deleted
//! definition stays deleted), and only [`insert`](UserToolServerStore::insert) creates.

use std::fmt;
use std::future::Future;
use std::str::FromStr;

use orch_core::{BoxError, Classify, ErrorClass, UserId};
use serde::{Deserialize, Serialize};

use crate::credentials::CredentialRef;
use crate::egress::{EgressTarget, MAX_URL_BYTES};
use crate::tools::is_reserved_header;

/// The most servers one person holds: 8 (ADR 0046). The existing 16 attached to one thread stand.
pub const MAX_USER_TOOL_SERVERS: usize = 8;

/// The most characters of a server's name.
pub const MAX_NAME_CHARS: usize = 80;

/// The most scopes of an `oauth2` server.
pub const MAX_SCOPES: usize = 32;

/// The most bytes of one scope.
pub const MAX_SCOPE_BYTES: usize = 128;

/// The most bytes of an `oauth2` client id.
pub const MAX_CLIENT_ID_BYTES: usize = 256;

/// The most bytes of the name of an `api_key` header.
pub const MAX_HEADER_NAME_BYTES: usize = 64;

// ------------------------------------------------------------------------------------- errors

/// Why a value is not part of a definition. Every one is the sender's mistake; none holds the
/// value (a rejected header name or scope is not echoed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DefinitionError {
    /// Not `u-` and 10 lowercase letters or digits.
    #[error("not a user tool server id: `u-` and 10 lowercase letters or digits")]
    BadId,
    /// A name is 1 to 80 characters with no control character.
    #[error("a name is 1 to {MAX_NAME_CHARS} characters with no control character")]
    BadName,
    /// A stored URL is not the normal form of a URL the guard reads (see
    /// [`UserToolServer::from_stored`]): `https` (or `http`), a host, no user information, query or
    /// fragment, and at most 2 KiB. (Whether it may be fetched today is the egress guard's.)
    #[error("not the normal form of a URL the guard reads: at most {MAX_URL_BYTES} bytes")]
    BadUrl,
    /// A header name is 1 to 64 letters, digits and `-`.
    #[error("a header name is 1 to {MAX_HEADER_NAME_BYTES} letters, digits and -")]
    BadHeader,
    /// The header is one the orchestrator or the HTTP client sets itself (`Host`, `Accept`,
    /// `Content-Type`, `Last-Event-ID`, `Mcp-*`, the framing and hop-by-hop headers, `Cookie`,
    /// `Proxy-*`) or the bearer's (`Authorization`).
    #[error("this header is not for a person to set")]
    ReservedHeader,
    /// A scope is 1 to 128 bytes of visible ASCII with no space, quote or backslash (RFC 6749).
    #[error("a scope is 1 to {MAX_SCOPE_BYTES} characters with no space, quote or backslash")]
    BadScope,
    /// More than 32 scopes.
    #[error("at most {MAX_SCOPES} scopes")]
    TooManyScopes,
    /// A client id is 1 to 256 bytes of visible ASCII.
    #[error("a client id is 1 to {MAX_CLIENT_ID_BYTES} characters of visible ASCII")]
    BadClientId,
    /// A credential reference is 1 to 128 bytes of visible ASCII.
    #[error("not a credential reference")]
    BadCredentialRef,
    /// Not one of `added`, `connected`, `needs_reauth`, `revoked`.
    #[error("not a server state: added, connected, needs_reauth or revoked")]
    BadState,
}

impl Classify for DefinitionError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

// ----------------------------------------------------------------------------------------- id

/// The id of a person's server: `u-` and 10 lowercase letters or digits, made here (ADR 0046).
///
/// The `u-` is what tells it from a deployment's `toolServers[].id`, which the configuration
/// refuses to start with `u-` when the feature is on, so the two never collide in the relayed name
/// `<id>__<tool>`. Ten characters of 36 are about 51 bits: a person holds at most eight, so a
/// collision among one person's ids is a matter of luck on the order of 10^-14, and the store is
/// keyed by `(owner, id)`, so another person's id never matters.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UserToolServerId(String);

impl UserToolServerId {
    /// The prefix of every id.
    pub const PREFIX: &'static str = "u-";
    /// The characters after the prefix.
    pub const SUFFIX_LEN: usize = 10;

    /// Reads the text of an id.
    ///
    /// # Errors
    /// [`DefinitionError::BadId`] for anything else.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        match text.strip_prefix(Self::PREFIX) {
            Some(suffix)
                if suffix.len() == Self::SUFFIX_LEN
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) =>
            {
                Ok(UserToolServerId(text.to_owned()))
            }
            _ => Err(DefinitionError::BadId),
        }
    }

    /// An id made of 16 bytes of entropy: the same bytes make the same id. Callers with an
    /// [`IdGen`](crate::IdGen) pass `ids.new_token_bytes()`; [`generate`](Self::generate) asks the
    /// operating system.
    ///
    /// The 128 bits are read as a number and cut to 36^10 values: the bias is 2^-76, far below
    /// anything an attacker could use.
    pub fn from_entropy(entropy: [u8; 16]) -> Self {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut n = u128::from_be_bytes(entropy) % 36u128.pow(10);
        let mut suffix = [b'0'; UserToolServerId::SUFFIX_LEN];
        for slot in suffix.iter_mut().rev() {
            *slot = DIGITS[usize::try_from(n % 36).unwrap_or(0)];
            n /= 36;
        }
        let suffix: String = suffix.iter().map(|b| char::from(*b)).collect();
        UserToolServerId(format!("{}{suffix}", Self::PREFIX))
    }

    /// A new id from the operating system's random source.
    pub fn generate() -> Self {
        let mut entropy = [0_u8; 16];
        // The operating system's source failing is not something to go on from.
        #[allow(clippy::expect_used)]
        getrandom::fill(&mut entropy).expect("the operating system's random source");
        Self::from_entropy(entropy)
    }

    /// The text of the id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserToolServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for UserToolServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UserToolServerId({})", self.0)
    }
}

impl FromStr for UserToolServerId {
    type Err = DefinitionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl TryFrom<String> for UserToolServerId {
    type Error = DefinitionError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<UserToolServerId> for String {
    fn from(id: UserToolServerId) -> String {
        id.0
    }
}

// ------------------------------------------------------------------------------------- the kind

/// How a person's server wants to be authenticated to (ADR 0046, decision 2): a **closed** enum, so
/// a kind added later fails to compile everywhere that matches on it.
///
/// It holds the shape of the authentication, never a secret: the token, the key or the client
/// secret go to [`CredentialBroker::store`](crate::CredentialBroker::store), and the tokens of an
/// OAuth flow are the broker's.
///
/// Reading one from storage does not check it (it is data): [`UserToolServer::new`] does, as does
/// [`validate`](Self::validate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserAuth {
    /// The server wants nothing.
    None,
    /// A bearer token, sent as `Authorization: Bearer <token>`.
    Bearer,
    /// A key in a header the person names (LibreChat's "custom header"). The name is checked: see
    /// [`UserAuth::api_key`].
    ApiKey {
        /// The header's name.
        header: String,
    },
    /// OAuth 2.0 authorization code with PKCE, which the broker runs.
    #[serde(rename = "oauth2")]
    OAuth2 {
        /// The scopes to ask for; empty asks for what the server's metadata says.
        scopes: Vec<String>,
        /// The client id of a client the person registered with the authorization server; `None`
        /// leaves registration to the broker (a client metadata document, or dynamic
        /// registration).
        client_id: Option<String>,
    },
}

impl UserAuth {
    /// An `api_key` kind whose key goes in `header`.
    ///
    /// The name is a header name (1 to 64 letters, digits and `-`) that the orchestrator does not
    /// own: `Host`, `Accept`, `Content-Type`, `Last-Event-ID` and every `Mcp-*` are refused in any
    /// case, as the configuration's `toolServers[].headers` refuses them, and so is `Authorization`,
    /// which is the `bearer` kind's. So are the headers that frame or route the request or belong to
    /// the connection, which an HTTP client sets and which a person's value could turn into request
    /// smuggling or a changed route: `Content-Length`, `Transfer-Encoding`, `Connection`, `Upgrade`,
    /// `TE`, `Expect`, `Keep-Alive`, `Trailer`, `Cookie` and every `Proxy-*`.
    ///
    /// # Errors
    /// [`DefinitionError::BadHeader`], [`DefinitionError::ReservedHeader`].
    pub fn api_key(header: &str) -> Result<Self, DefinitionError> {
        let auth = UserAuth::ApiKey {
            header: header.to_owned(),
        };
        auth.validate()?;
        Ok(auth)
    }

    /// An `oauth2` kind.
    ///
    /// # Errors
    /// [`DefinitionError::BadScope`], [`DefinitionError::TooManyScopes`],
    /// [`DefinitionError::BadClientId`].
    pub fn oauth2(scopes: Vec<String>, client_id: Option<String>) -> Result<Self, DefinitionError> {
        let auth = UserAuth::OAuth2 { scopes, client_id };
        auth.validate()?;
        Ok(auth)
    }

    /// Checks what [`api_key`](Self::api_key) and [`oauth2`](Self::oauth2) check, for a kind that
    /// was built from its parts or read from storage.
    ///
    /// # Errors
    /// The error of the constructor of that kind.
    pub fn validate(&self) -> Result<(), DefinitionError> {
        match self {
            UserAuth::None | UserAuth::Bearer => Ok(()),
            UserAuth::ApiKey { header } => check_header(header),
            UserAuth::OAuth2 { scopes, client_id } => {
                if scopes.len() > MAX_SCOPES {
                    return Err(DefinitionError::TooManyScopes);
                }
                if !scopes.iter().all(|scope| is_scope(scope)) {
                    return Err(DefinitionError::BadScope);
                }
                match client_id {
                    Some(id) if !is_client_id(id) => Err(DefinitionError::BadClientId),
                    _ => Ok(()),
                }
            }
        }
    }

    /// The name of the kind as the API writes it: `none`, `bearer`, `api_key`, `oauth2`.
    pub const fn kind(&self) -> &'static str {
        match self {
            UserAuth::None => "none",
            UserAuth::Bearer => "bearer",
            UserAuth::ApiKey { .. } => "api_key",
            UserAuth::OAuth2 { .. } => "oauth2",
        }
    }

    /// Whether the kind has a credential to store: every kind but `none`.
    pub const fn needs_credential(&self) -> bool {
        !matches!(self, UserAuth::None)
    }

    /// The header of an `api_key`; `None` for the other kinds.
    pub fn header_name(&self) -> Option<&str> {
        match self {
            UserAuth::ApiKey { header } => Some(header),
            UserAuth::None | UserAuth::Bearer | UserAuth::OAuth2 { .. } => None,
        }
    }

    /// The scopes of an `oauth2`; empty for the other kinds.
    pub fn scopes(&self) -> &[String] {
        match self {
            UserAuth::OAuth2 { scopes, .. } => scopes,
            UserAuth::None | UserAuth::Bearer | UserAuth::ApiKey { .. } => &[],
        }
    }

    /// The pre-registered client id of an `oauth2`.
    pub fn client_id(&self) -> Option<&str> {
        match self {
            UserAuth::OAuth2 { client_id, .. } => client_id.as_deref(),
            UserAuth::None | UserAuth::Bearer | UserAuth::ApiKey { .. } => None,
        }
    }
}

/// Headers an HTTP client frames the request with, or that belong to one connection, and `Cookie`: a
/// person's `api_key` value in one of them would change how the request is read, not authenticate it.
const FRAMING_HEADERS: &[&str] = &[
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "expect",
    "keep-alive",
    "trailer",
    "cookie",
];

/// A header name that is a token of letters, digits and `-`, and not one the orchestrator owns.
fn check_header(name: &str) -> Result<(), DefinitionError> {
    if name.is_empty()
        || name.len() > MAX_HEADER_NAME_BYTES
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(DefinitionError::BadHeader);
    }
    let starts_with = |prefix: &[u8]| {
        name.as_bytes()
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
    };
    if starts_with(b"mcp-")
        || starts_with(b"proxy-")
        || is_reserved_header(name)
        || FRAMING_HEADERS
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
    {
        return Err(DefinitionError::ReservedHeader);
    }
    Ok(())
}

/// An OAuth scope token (RFC 6749, section 3.3): visible ASCII without `"` and `\`.
fn is_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= MAX_SCOPE_BYTES
        && scope
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

fn is_client_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_CLIENT_ID_BYTES && id.bytes().all(|b| b.is_ascii_graphic())
}

// ------------------------------------------------------------------------------------ the state

/// Where a server is in the lifecycle of its credential (ADR 0046, the state diagram): what the
/// store keeps. It is the definition's, so it is the same for every thread of the person.
///
/// `unavailable` is **not** here. It is a reason shown while a cause lasts (the server is down, the
/// egress guard refuses it, the person's permission is off) and clears with the cause; the API says
/// it with [`ToolServerStatus::Unavailable`] and stores nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserToolServerState {
    /// The definition is saved and there is no credential yet.
    Added,
    /// Consent is done, the key is stored, or the kind is `none`: the server is offered.
    Connected,
    /// The refresh failed, the server answered 401, or a scope step-up is wanted: the person must
    /// authorize again.
    NeedsReauth,
    /// The person disconnected, or the authorization server revoked the grant.
    Revoked,
}

impl UserToolServerState {
    /// The name as it is stored and written: `added`, `connected`, `needs_reauth`, `revoked`.
    pub const fn as_str(self) -> &'static str {
        match self {
            UserToolServerState::Added => "added",
            UserToolServerState::Connected => "connected",
            UserToolServerState::NeedsReauth => "needs_reauth",
            UserToolServerState::Revoked => "revoked",
        }
    }

    /// Whether the server is offered to a person (the picker, an attach, the relay's listing): only
    /// a connected one.
    pub const fn is_offered(self) -> bool {
        matches!(self, UserToolServerState::Connected)
    }

    /// The status a person sees for this state: `ready` for a connected server, `needs_auth` for
    /// the others. (`unavailable` is the API's, from a cause it knows.)
    pub const fn status(self) -> ToolServerStatus {
        match self {
            UserToolServerState::Connected => ToolServerStatus::Ready,
            UserToolServerState::Added
            | UserToolServerState::NeedsReauth
            | UserToolServerState::Revoked => ToolServerStatus::NeedsAuth,
        }
    }
}

impl FromStr for UserToolServerState {
    type Err = DefinitionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "added" => Ok(UserToolServerState::Added),
            "connected" => Ok(UserToolServerState::Connected),
            "needs_reauth" => Ok(UserToolServerState::NeedsReauth),
            "revoked" => Ok(UserToolServerState::Revoked),
            _ => Err(DefinitionError::BadState),
        }
    }
}

/// What a person is told about a server (`GET /api/tool-servers`, ADR 0046): `ready`, `needs_auth`
/// or `unavailable` with a reason. Derived, never stored.
///
/// No port signature uses it. It is here, and not in the API's crate, because two surfaces write it
/// (the API's listing, the relay's refusal to offer a server) and `orch-ports` is the crate both
/// depend on, and because [`UserToolServerState::status`] is where a stored state becomes what a
/// person sees: one table for both. It moves out to the crate that owns the endpoints if that
/// crate comes to be the only one that needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolServerStatus {
    /// Connected and offered.
    Ready,
    /// The person has to authorize or give a key: a call to action in the web, not an error of the
    /// agent.
    NeedsAuth,
    /// Not offered for now, for a reason that clears with its cause.
    Unavailable(UnavailableReason),
}

impl ToolServerStatus {
    /// The name the API writes: `ready`, `needs_auth`, `unavailable`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ToolServerStatus::Ready => "ready",
            ToolServerStatus::NeedsAuth => "needs_auth",
            ToolServerStatus::Unavailable(_) => "unavailable",
        }
    }

    /// The reason, for `unavailable`.
    pub const fn reason(self) -> Option<UnavailableReason> {
        match self {
            ToolServerStatus::Unavailable(reason) => Some(reason),
            ToolServerStatus::Ready | ToolServerStatus::NeedsAuth => None,
        }
    }
}

/// Why a server is `unavailable`: a closed list, so what the web shows is never built from a URL, an
/// address or an error message. (Kept beside [`ToolServerStatus`], for the same reason.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnavailableReason {
    /// The person's permission to use their own servers (`tools.user.add`) is off.
    NotPermitted,
    /// The egress guard refuses the server's address.
    EgressRefused,
    /// The server did not answer.
    ServerDown,
    /// The credential broker, or the secret store behind it, is down.
    BrokerDown,
}

impl UnavailableReason {
    /// The name the API writes: `not_permitted`, `egress_refused`, `server_down`, `broker_down`.
    pub const fn as_str(self) -> &'static str {
        match self {
            UnavailableReason::NotPermitted => "not_permitted",
            UnavailableReason::EgressRefused => "egress_refused",
            UnavailableReason::ServerDown => "server_down",
            UnavailableReason::BrokerDown => "broker_down",
        }
    }
}

// --------------------------------------------------------------------------------- definition

/// One of a person's MCP servers, as the store keeps it: **no secret**.
///
/// Built by [`new`](UserToolServer::new), from a URL the guard accepted
/// ([`EgressPolicy::check_url`](crate::EgressPolicy::check_url) made the [`EgressTarget`]), or by
/// [`from_stored`](UserToolServer::from_stored) for a row read back, which checks the URL again. So
/// a definition that exists is a valid one, and its [`url`](UserToolServer::url) is the normal form
/// the broker keeps with the credential it issues for it.
///
/// `Debug` shows the URL's scheme, host and port and never its path.
#[derive(Clone, PartialEq, Eq)]
pub struct UserToolServer {
    id: UserToolServerId,
    name: String,
    target: EgressTarget,
    auth: UserAuth,
    state: UserToolServerState,
    credential: Option<CredentialRef>,
}

impl UserToolServer {
    /// A definition that has just been added: state [`Added`](UserToolServerState::Added), no
    /// credential, at the URL `target` is the normal form of.
    ///
    /// The name is trimmed.
    ///
    /// # Errors
    /// [`DefinitionError`] for the first part that is not valid.
    pub fn new(
        id: UserToolServerId,
        name: impl AsRef<str>,
        target: &EgressTarget,
        auth: UserAuth,
    ) -> Result<Self, DefinitionError> {
        Self::build(
            id,
            name.as_ref(),
            target.clone(),
            auth,
            UserToolServerState::Added,
            None,
        )
    }

    /// A definition read back from storage, by an adapter: every part is checked again, the URL
    /// included. The URL must be the **normal form** of a URL that passes the rules of the URL itself
    /// ([`EgressPolicy::check_url`](crate::EgressPolicy::check_url): length, scheme, no user
    /// information, a host, no query, no fragment, a port), so a row that was altered, or written by
    /// something else, is not a definition.
    ///
    /// The deployment's lists and the address are **not** applied here: the policy may have been
    /// tightened since the row was written, and the person must still be able to list the server,
    /// see that it is `unavailable` (`egress_refused`) and delete it. The connection applies them
    /// every time (`check_url`, then `check_resolved`), and an `http` URL is read as the policy that
    /// stored it admitted it.
    ///
    /// # Errors
    /// [`DefinitionError`] for the first part that is not valid; [`DefinitionError::BadUrl`] for
    /// the URL, which an adapter treats as a corrupt row
    /// ([`UserToolServerStoreError::Corrupt`]).
    pub fn from_stored(
        id: UserToolServerId,
        name: &str,
        url: &str,
        auth: UserAuth,
        state: UserToolServerState,
        credential: Option<CredentialRef>,
    ) -> Result<Self, DefinitionError> {
        let target = EgressTarget::from_stored(url).map_err(|_| DefinitionError::BadUrl)?;
        Self::build(id, name, target, auth, state, credential)
    }

    fn build(
        id: UserToolServerId,
        name: &str,
        target: EgressTarget,
        auth: UserAuth,
        state: UserToolServerState,
        credential: Option<CredentialRef>,
    ) -> Result<Self, DefinitionError> {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(DefinitionError::BadName);
        }
        auth.validate()?;
        Ok(UserToolServer {
            id,
            name: name.to_owned(),
            target,
            auth,
            state,
            credential,
        })
    }

    /// The same definition in `state`.
    #[must_use]
    pub fn with_state(mut self, state: UserToolServerState) -> Self {
        self.state = state;
        self
    }

    /// The same definition that keeps `credential` as the reference to its credential.
    #[must_use]
    pub fn with_credential(mut self, credential: CredentialRef) -> Self {
        self.credential = Some(credential);
        self
    }

    /// The id.
    pub fn id(&self) -> &UserToolServerId {
        &self.id
    }

    /// The name the person gave it, trimmed.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The server's URL in its normal form: what the broker keeps with the credential it issues for
    /// it (the OAuth `resource`), and what it compares with the current one at `lease`. It may hold a
    /// secret in its path: never print it.
    pub fn url(&self) -> &str {
        self.target.url()
    }

    /// The kind of authentication, with its header, scopes and client id.
    pub fn auth(&self) -> &UserAuth {
        &self.auth
    }

    /// Where it is in the lifecycle.
    pub fn state(&self) -> UserToolServerState {
        self.state
    }

    /// The reference to its credential, once the broker has stored one.
    pub fn credential(&self) -> Option<&CredentialRef> {
        self.credential.as_ref()
    }
}

impl fmt::Debug for UserToolServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserToolServer")
            .field("id", &self.id)
            .field("name", &self.name)
            // scheme, host and port: the path can carry a secret
            .field("target", &self.target)
            .field("auth", &self.auth)
            .field("state", &self.state)
            .field("credential", &self.credential)
            .finish()
    }
}

// ------------------------------------------------------------------------------- the revision

/// The revision of a stored definition: what the store gives to every write of it and a read returns,
/// so that a write based on a read can say which one it is based on.
///
/// Opaque: a caller compares for **equality only** and passes it back. An adapter gives a new one to
/// every write of a definition (an `insert`, a `replace` or a `set_state`), **never gives one twice
/// for a key, not even across a `delete`** (a revision read before a definition was deleted and
/// added again, with the same id, must not match the new one; a counter kept for the whole store, or
/// a database sequence, does this) and numbers the writes of a key in the order they land, so the
/// revision a definition holds is the greatest it has been given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerRevision(u64);

impl ServerRevision {
    /// The revision `number`. For adapters, and for a test.
    pub const fn new(number: u64) -> Self {
        ServerRevision(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ServerRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A definition as a read returns it: with the revision a write based on it passes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredServer {
    server: UserToolServer,
    revision: ServerRevision,
}

impl StoredServer {
    /// A definition at a revision. For adapters.
    pub fn new(server: UserToolServer, revision: ServerRevision) -> Self {
        StoredServer { server, revision }
    }

    /// The definition.
    pub fn server(&self) -> &UserToolServer {
        &self.server
    }

    /// The revision to pass to [`replace`](UserToolServerStore::replace) and
    /// [`set_state`](UserToolServerStore::set_state).
    pub fn revision(&self) -> ServerRevision {
        self.revision
    }

    /// The definition, owned.
    pub fn into_server(self) -> UserToolServer {
        self.server
    }
}

// ------------------------------------------------------------------------------------- the port

/// Why a store did not do what it was asked.
///
/// None holds a definition, a URL or a credential.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UserToolServerStoreError {
    /// The owner already holds [`MAX_USER_TOOL_SERVERS`] and the definition is not one of them.
    /// Nothing was written.
    #[error("a person holds at most {limit} servers of their own")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// [`insert`](UserToolServerStore::insert) of an id the owner already holds. Nothing was
    /// written.
    #[error("the person already has a server with that id")]
    Exists,
    /// There is no such server for this owner: it was never added, it was removed, or it is
    /// another person's (which is the same to the caller). A write of a removed definition is this,
    /// and writes nothing.
    #[error("no such user tool server")]
    NotFound,
    /// The definition changed since the revision the write was based on was read. Nothing was
    /// written. Read it again and decide again.
    #[error("the server changed since it was read")]
    Conflict,
    /// The store could not be reached or failed.
    #[error("the user tool server store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// What the store holds is not a definition: a row that cannot be read back through
    /// [`UserToolServer::from_stored`]. Alert.
    #[error("the user tool server store holds corrupt data: {0}")]
    Corrupt(String),
}

impl UserToolServerStoreError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        UserToolServerStoreError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unavailable` error; any other variant is returned
    /// unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            UserToolServerStoreError::Unavailable { detail, .. } => {
                UserToolServerStoreError::Unavailable {
                    detail,
                    source: Some(source.into()),
                }
            }
            other @ (UserToolServerStoreError::TooMany { .. }
            | UserToolServerStoreError::Exists
            | UserToolServerStoreError::NotFound
            | UserToolServerStoreError::Conflict
            | UserToolServerStoreError::Corrupt(_)) => other,
        }
    }
}

impl Classify for UserToolServerStoreError {
    fn class(&self) -> ErrorClass {
        match self {
            UserToolServerStoreError::TooMany { .. } | UserToolServerStoreError::Exists => {
                ErrorClass::Rejected
            }
            UserToolServerStoreError::NotFound => ErrorClass::NotFound,
            UserToolServerStoreError::Conflict => ErrorClass::Conflict,
            UserToolServerStoreError::Unavailable { .. } => ErrorClass::Transient,
            UserToolServerStoreError::Corrupt(_) => ErrorClass::Corrupt,
        }
    }
}

/// Where the definitions of people's MCP servers are kept, by `(owner, id)`.
///
/// An implementation keeps them durably, holds no secret (a definition has none), and **never lets
/// one owner reach another's**: for every call, another person's server is simply not there. It is
/// not asked who may add a server (the API checks the permission) and it does not judge a URL (the
/// egress guard does, and [`UserToolServer::from_stored`] checks a row again).
///
/// # What an implementation owes
///
/// - **The limit is the store's, and atomic.** [`insert`](Self::insert) refuses an owner's ninth
///   ([`MAX_USER_TOOL_SERVERS`] is 8) with [`TooMany`](UserToolServerStoreError::TooMany), and
///   writers that race for the last places never pass it. A count read and then an insert is not
///   enough: two writers both count seven. A SQL adapter serializes the inserts of one owner, with a
///   transaction-scoped advisory lock keyed by the owner (`pg_advisory_xact_lock(hashtext(owner))`)
///   or `SELECT … FOR UPDATE` on a row of the owner, then counts, then inserts. The testkit races
///   sixteen adds for the last places, twenty times over.
/// - **Revisions are compare and swap.** Every write of a definition gives it a new
///   [`ServerRevision`] (never given twice for a key, even across a `delete`: see its docs), and
///   [`replace`](Self::replace) and [`set_state`](Self::set_state) write only when the stored
///   revision is the one passed, in one atomic step with the check. The writers of one key are
///   serialized.
/// - **A write never creates, and never brings back.** Only `insert` creates a definition. A
///   `replace` or a `set_state` of a definition that is not there (never added, or deleted since it
///   was read) is `NotFound` and writes nothing.
pub trait UserToolServerStore: Send + Sync + 'static {
    /// Adds `server` for `owner`, state and credential reference as it holds them, and returns its
    /// first revision.
    ///
    /// # Errors
    /// [`UserToolServerStoreError::Exists`] when the owner holds a definition with that id (use
    /// [`replace`](Self::replace)); [`UserToolServerStoreError::TooMany`] when this would be the
    /// owner's ninth; [`UserToolServerStoreError::Unavailable`] or
    /// [`UserToolServerStoreError::Corrupt`] when the store fails.
    fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> impl Future<Output = Result<ServerRevision, UserToolServerStoreError>> + Send;

    /// The owner's definition `id` with its revision; `None` when there is none.
    ///
    /// # Errors
    /// [`UserToolServerStoreError`] when the store fails.
    fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> impl Future<Output = Result<Option<StoredServer>, UserToolServerStoreError>> + Send;

    /// Every definition of the owner with its revision, ordered by id.
    ///
    /// # Errors
    /// [`UserToolServerStoreError`] when the store fails.
    fn list(
        &self,
        owner: &UserId,
    ) -> impl Future<Output = Result<Vec<StoredServer>, UserToolServerStoreError>> + Send;

    /// **Compare and swap**: replaces the owner's definition with `server`'s id **whole** (an edit:
    /// name, URL, kind, state and credential reference) when its stored revision is `expected`, and
    /// returns the new revision. This is also how a credential reference is set: the broker stored a
    /// credential, so the API replaces the definition it read with one that names it.
    ///
    /// An edit that changes the URL or the kind of authentication leaves the credential that was
    /// issued for the old ones behind: the caller revokes it first
    /// ([`CredentialBroker::revoke`](crate::CredentialBroker::revoke)) and writes the new definition
    /// as [`Added`](UserToolServerState::Added) with no reference.
    ///
    /// # Errors
    /// [`UserToolServerStoreError::NotFound`] when the owner has no such definition (it was deleted
    /// since it was read: nothing is brought back); [`UserToolServerStoreError::Conflict`] when its
    /// revision is not `expected`: nothing is written.
    fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> impl Future<Output = Result<ServerRevision, UserToolServerStoreError>> + Send;

    /// **Compare and swap** of the state alone: sets the state of the owner's definition `id` when
    /// its stored revision is `expected`, changes nothing else of it, and returns the new revision.
    /// It is the narrow write of the broker and the relay (`needs_reauth` after a refresh failed or a
    /// 401). A writer whose read is stale re-reads: if the URL or the kind changed since, what it
    /// observed was about another server, and it writes nothing.
    ///
    /// # Errors
    /// As [`replace`](Self::replace).
    fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> impl Future<Output = Result<ServerRevision, UserToolServerStoreError>> + Send;

    /// Removes the owner's definition `id`, whatever its revision. **Idempotent**: removing what is
    /// not there succeeds. It does not touch the credential.
    ///
    /// **Deleting a server is "revoke, then delete"**: the caller first asks the broker to
    /// [`revoke`](crate::CredentialBroker::revoke) the credential, which reads this definition (its
    /// URL tells the broker where the authorization server is, to revoke upstream), and deletes the
    /// definition only after the broker answered. A definition deleted first leaves a sealed
    /// credential that no definition names and nobody can revoke. A `revoke` that fails with
    /// [`Unavailable`](crate::CredentialError::Unavailable) aborts the delete; the person tries again.
    ///
    /// # Errors
    /// [`UserToolServerStoreError`] when the store fails.
    fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> impl Future<Output = Result<(), UserToolServerStoreError>> + Send;
}

/// The definitions store of a deployment where people's own MCP servers are off
/// (`userToolServers.enabled` false or absent): every call is
/// [`UserToolServerStoreError::Unavailable`] with a sentence that says why and holds no owner or
/// definition. A feature that is off keeps and reads nothing, so a route that forgets to check the
/// switch cannot write a definition. The model is [`NoArtifacts`](crate::NoArtifacts); it does not
/// pass the conformance suite, which is for the stores that keep.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoUserToolServers;

impl NoUserToolServers {
    fn refuse<T>() -> Result<T, UserToolServerStoreError> {
        Err(UserToolServerStoreError::unavailable(
            "people's own MCP servers are off in this deployment (userToolServers.enabled is false)",
        ))
    }
}

impl UserToolServerStore for NoUserToolServers {
    async fn insert(
        &self,
        _owner: &UserId,
        _server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        Self::refuse()
    }

    async fn get(
        &self,
        _owner: &UserId,
        _id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        Self::refuse()
    }

    async fn list(&self, _owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        Self::refuse()
    }

    async fn replace(
        &self,
        _owner: &UserId,
        _server: &UserToolServer,
        _expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        Self::refuse()
    }

    async fn set_state(
        &self,
        _owner: &UserId,
        _id: &UserToolServerId,
        _state: UserToolServerState,
        _expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        Self::refuse()
    }

    async fn delete(
        &self,
        _owner: &UserId,
        _id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        Self::refuse()
    }
}
