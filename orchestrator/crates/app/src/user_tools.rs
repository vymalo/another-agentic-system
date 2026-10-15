//! People's own MCP servers: what the application does with their definitions and with the credential
//! broker (ADR 0046, slice 4b). The HTTP routes call these methods and nothing else; no handler is
//! given a port.
//!
//! # Who may
//!
//! - **Add**, **edit** and **authorize** need `tools.user.add`, granted to no role by default
//!   ([`Permission::ToolsUserAdd`]). The OAuth **callback** needs no person at all: the browser that
//!   comes back to it carries no session, and the person is found by the `state` alone (see below).
//! - **List** one's own needs no permission of its own. A person whose permission was taken away sees
//!   their servers as `unavailable` with the reason `not_permitted`.
//! - **Delete** needs no permission either, so a person whose permission was taken away can still
//!   remove what they added and free the places they hold.
//! - Every call is the owner's alone: another person's server is not there (`NotFound`), whatever the
//!   caller's roles (ADR 0039).
//! - Every call first asks the deployment's mode (`UserToolPorts::mode`): `Off` is
//!   [`UserToolRefusal::Off`], never a store error, because the refusing store of an `Off` deployment
//!   fails even a read (ADR 0046, departures 67 and 78).
//!
//! # The order of writes around the broker
//!
//! The broker reads the definition it works for from the store when it **keeps** a secret, so the
//! definition is written first and the credential after it; the definition names the credential only
//! once the broker has kept it (slice 2's open point 2). `revoke` reads no definition (departure 21).
//!
//! ```text
//! add    none           insert(connected)
//!        oauth2         insert(added)                          the person authorizes (below)
//!        bearer/api_key insert(added), store, replace(connected, credential)
//!                       a failed store takes the definition back (after a best-effort revoke, since the
//!                       broker may have kept the key); a failed replace revokes what was kept
//! edit   name, header   replace
//!        re-key         (read the revision again), store, replace(connected, credential)
//!                       one write when nothing else changed; a replace that loses to another writer
//!                       forgets the key when the definition names no credential now
//!        url, kind,     replace(added, no credential), revoke, then store and replace(connected)
//!        scopes, client when a secret came with it
//! delete                revoke (Full, a kind with a credential), delete
//!                       in WithoutCredentials the definition is deleted and revoke is not called
//! authorize             begin_oauth, then the state is kept (the pending authorizations)
//! callback              take the state (once), complete_oauth, replace(connected, credential)
//!                       a callback that cannot finish leaves the server as it was
//! ```
//!
//! **A write that can lose a race comes before a write that cannot be taken back.** An edit that moves
//! a server writes the definition (a compare-and-swap on the revision it read) *before* it revokes, so
//! a store that fails, a writer that got in first or a request that is dropped never leaves a server
//! that says `connected` with a credential the broker no longer has. What the compare-and-swap leaves
//! behind when the revoke then fails is a server that says `added` and names no credential, with a
//! sealed record that is bound to the old URL or grant, is not lent and is forgotten by the next key
//! or the delete. A delete is the other way round, revoke first: a failed revoke leaves the definition
//! to try again.
//!
//! # Authorizing an `oauth2` server
//!
//! ```text
//! person ── authorize ──▶ begin_oauth(owner, server, redirect) ─▶ authorize_url, state, expires_at
//!                         pending.put(state ↦ owner, server, expires_at)      the person is sent to the URL
//! browser ── callback (code, state, iss | error) ──▶ pending.take(state)      once; unknown, used or ended: nothing
//!                         complete_oauth(owner, server, state, code, iss)     the broker checks `iss` (RFC 9207)
//!                         replace(connected, credential) on the revision it read
//! ```
//!
//! **The callback is not a person's request.** The browser that reaches it is the system browser on
//! the desktop and the in-app tab on mobile; it carries neither the app's bearer token nor the web's
//! cookie (ADR 0047). Who the person is comes from the `state`, kept at `authorize` as `state ↦ (owner,
//! server, until)` and **taken once** (the PKCE verifier stays in the broker). So
//! [`App::finish_user_tool_authorization`] takes no [`Requester`], answers with an
//! [`AuthorizationOutcome`] (a page, never a problem), and does not look at `tools.user.add` again:
//! the permission was checked when the flow began, the flow lives for minutes, and a person who has
//! lost it sees their servers `unavailable` (`not_permitted`) at the next listing.
//!
//! **The state is spent by the first callback that finds it**, whatever comes of it: a callback with
//! `error=` (the person declined), with no `code`, whose exchange fails or whose `iss` the broker
//! refuses is over, and the person begins again. What the authorization server wrote in `error` or
//! `error_description`, and the `code`, the `state` and the `iss`, are never read into a message, a
//! log line or a page.
//!
//! # What is checked, and when
//!
//! Everything a person typed is checked **before any write**: the name, the URL (the egress guard's
//! `check_url`, so a refusal is the guard's `public_reason` and never the precise rule, ADR 0046
//! departure 17), the header, the scopes, and the secret (printable ASCII, no space at either end,
//! at most [`MAX_USER_SECRET_BYTES`]). The broker has no "invalid input" class (departure 33), so
//! nothing it cannot take is sent to it. The check of the URL is the **shape** of it and the lists;
//! what a name resolves to is judged by the broker at each connection, and the DNS is not asked here.
//!
//! # What is never in an answer, an error or a log line
//!
//! The secret. It is a [`ToolSecret`] from the moment the route has it: its `Debug` prints
//! `<redacted>`, it is wiped when dropped, and none of the types here prints it or the URL (a path can
//! carry a secret). The errors name no URL; the cause of a broker failure is logged by its class and
//! its chain, which the port promises holds no secret.

use std::collections::BTreeSet;
use std::fmt;

use orch_core::{Classify, report};
use orch_ports::{
    CredentialBroker, CredentialError, EgressPolicy, EgressTarget, IdGen,
    MIN_SCRUBBABLE_SECRET_BYTES, Ports, ServerRevision, StoreError, StoredServer, ToolSecret,
    ToolServerStatus, UnavailableReason, UserAuth, UserToolPorts, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserToolsMode,
};

use crate::{App, AppError, Permission, Requester, Resource};

mod authorize;

pub(crate) use authorize::AuthorizeLocks;
pub use authorize::{
    AuthorizationOutcome, MAX_CALLBACK_CODE_BYTES, OAuthCallback, StartedAuthorization,
};

/// The longest secret a person may type, in bytes. The broker seals it with the server's URL in a
/// record of at most 16 KiB, so this leaves the room.
pub const MAX_USER_SECRET_BYTES: usize = 4 * 1024;

/// How many times an edit that did not name a revision reads the definition again after another
/// writer changed it between its read and its write.
const EDIT_ATTEMPTS: usize = 3;

/// How many ids are tried when a generated one is taken (51 bits: it is a matter of luck).
const ID_ATTEMPTS: usize = 3;

// ------------------------------------------------------------------------------- refusals

/// Why a request about people's own MCP servers is refused (ADR 0046). The API gives each a status
/// and, where a client acts on it, a code. None holds a URL, a secret or another person's data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UserToolRefusal {
    /// `userToolServers.enabled` is false or absent, or this process serves no routes: the feature
    /// does not exist here (404).
    #[error("people's own MCP servers are not enabled in this deployment")]
    Off,
    /// No such server for this person: never added, deleted, or another person's (404).
    #[error("no such server")]
    NotFound,
    /// The person holds the most they may (409, `too_many_servers`).
    #[error("a person holds at most {limit} servers of their own; delete one first")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// The server changed since the revision the request was based on was read (409,
    /// `stale_revision`).
    #[error("the server changed since it was read; read it again")]
    StaleRevision,
    /// The deployment keeps no credentials, and the request needs one kept (422,
    /// `credentials_not_kept`).
    #[error(
        "this deployment keeps no credentials: only a server that wants no authentication can be \
         added or changed to"
    )]
    NoCredentials,
    /// The egress guard refuses the URL (400, `url_refused`). `reason` is what a person is shown
    /// (`EgressRefusal::public_reason`): never the precise rule, never an address.
    #[error("{reason}")]
    UrlRefused {
        /// The reason a person is shown.
        reason: String,
    },
    /// The credential broker could not keep or remove a credential now (503, `broker_down`). The
    /// cause is in the log.
    #[error("the credential could not be kept or removed now; try again")]
    CredentialsUnavailable,
    /// The egress guard refused a URL the broker was about to fetch for the authorization (the
    /// server's metadata, its authorization server's, the token endpoint): the deployment's lists, or
    /// an address a connection may not be made to (422, `egress_refused`). `reason` is
    /// `EgressRefusal::public_reason`, the one sentence for every rule, never an address.
    #[error("{reason}")]
    EgressRefused {
        /// The reason a person is shown.
        reason: String,
    },
    /// This deployment has no address for the OAuth callback (`userToolServers.oauth.callbackUrl`, or
    /// `server.publicUrl`): an `oauth2` server cannot be authorized here (422, `oauth_not_configured`).
    #[error(
        "this deployment has no address for the OAuth callback, so a server cannot be authorized"
    )]
    OAuthNotConfigured,
    /// The authorization could not be started now: the server publishes no usable metadata, its
    /// authorization server does not answer or does not support PKCE, or the broker cannot reach its
    /// store (503, `authorization_unavailable`). The cause is in the log, by class and chain.
    #[error("the authorization could not be started now; try again")]
    AuthorizationUnavailable,
    /// A server of the person's own that cannot be attached to a thread now, because it is not
    /// `ready`: `reason` says why (422, `tool_server_needs_auth` for a call to action,
    /// `tool_server_unavailable` for a cause that clears by itself). A server that is not the
    /// person's, or does not exist, is not this: it is the same 422 an unknown id has, so that the
    /// answer does not tell whether another person has a server by that id.
    #[error("the server `{id}` cannot be attached now: {}", reason.sentence())]
    NotReady {
        /// The id the person sent: theirs, not a secret.
        id: String,
        /// Why it is not ready.
        reason: OwnReason,
    },
    /// Other requests kept changing the server under this one: an edit that did not name a revision
    /// read it again three times and lost each time, or an add found its own definition changed
    /// before it could connect it (503, `Retry-After: 1`, no code). The same as a thread's
    /// [`AppError::Contended`], but the sentence is about the server and not a thread.
    #[error("the server is being changed by another request; try again")]
    Busy,
}

impl From<UserToolRefusal> for AppError {
    fn from(refusal: UserToolRefusal) -> Self {
        AppError::UserTool(refusal)
    }
}

impl UserToolRefusal {
    /// The stable name a client acts on, where there is one.
    pub const fn code(&self) -> Option<&'static str> {
        match self {
            UserToolRefusal::Off | UserToolRefusal::NotFound => None,
            UserToolRefusal::TooMany { .. } => Some("too_many_servers"),
            UserToolRefusal::StaleRevision => Some("stale_revision"),
            UserToolRefusal::NoCredentials => Some("credentials_not_kept"),
            UserToolRefusal::UrlRefused { .. } => Some("url_refused"),
            UserToolRefusal::CredentialsUnavailable => Some("broker_down"),
            UserToolRefusal::EgressRefused { .. } => Some("egress_refused"),
            UserToolRefusal::OAuthNotConfigured => Some("oauth_not_configured"),
            UserToolRefusal::AuthorizationUnavailable => Some("authorization_unavailable"),
            UserToolRefusal::NotReady { reason, .. } => Some(reason.attach_code()),
            // a client waits and tries again; there is nothing else to act on
            UserToolRefusal::Busy => None,
        }
    }
}

// ------------------------------------------------------------------------------- requests

/// What a person types to add a server (`POST /api/user-tool-servers`). `Debug` shows no URL and no
/// secret.
pub struct AddUserToolServer {
    /// The name the picker shows: 1 to 80 characters, one line.
    pub name: String,
    /// The server's MCP endpoint, as typed; the egress guard reads it.
    pub url: String,
    /// The kind of authentication.
    pub auth: UserAuth,
    /// The token (`bearer`) or the key (`api_key`): required for those two, refused for the others.
    pub secret: Option<ToolSecret>,
}

impl fmt::Debug for AddUserToolServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddUserToolServer")
            .field("name", &self.name)
            .field("auth", &self.auth)
            .field("secret", &self.secret)
            .finish_non_exhaustive()
    }
}

/// What a person changes (`PATCH /api/user-tool-servers/{id}`): every member is optional, at least
/// one is needed. `Debug` shows no URL and no secret.
#[derive(Default)]
pub struct EditUserToolServer {
    /// A new name. Keeps the credential.
    pub name: Option<String>,
    /// A new URL. The credential was issued for the old one, so it is revoked and the server is
    /// `added` again.
    pub url: Option<String>,
    /// A new kind or new parts of it. A change of kind, scopes or client id revokes the credential;
    /// a new header name keeps it.
    pub auth: Option<UserAuth>,
    /// A new token or key (re-keying), for a `bearer` or `api_key` server.
    pub secret: Option<ToolSecret>,
    /// The revision the edit is based on, from the listing: a server that changed since is
    /// [`UserToolRefusal::StaleRevision`]. Without it the edit applies to what the server is now.
    pub revision: Option<ServerRevision>,
}

impl fmt::Debug for EditUserToolServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditUserToolServer")
            .field("name", &self.name)
            .field("auth", &self.auth)
            .field("secret", &self.secret)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------------- the view

/// Why a server is not `ready`: a closed list, so what the web shows is never built from a URL, an
/// address or an error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnReason {
    /// `needs_auth`: the definition is saved and holds no credential yet (a key not given, an
    /// authorization not done).
    NotConnected,
    /// `needs_auth`: the credential ended (a refresh was refused, the server answered 401); the
    /// person gives a key or authorizes again.
    Reauthorize,
    /// `needs_auth`: the person disconnected it, or the authorization server revoked the grant.
    Disconnected,
    /// `unavailable`: the person's `tools.user.add` was taken away.
    NotPermitted,
    /// `unavailable`: today's egress lists refuse the server's address.
    EgressRefused,
    /// `unavailable`: the server wants a credential and this deployment keeps none.
    BrokerDown,
}

impl OwnReason {
    /// A sentence for a person that says why, for an attach that is refused. It holds no URL and no
    /// address.
    pub const fn sentence(self) -> &'static str {
        match self {
            OwnReason::NotConnected => "it has no key or authorization yet",
            OwnReason::Reauthorize => "its key or authorization ended; give it again",
            OwnReason::Disconnected => "it was disconnected; connect it again",
            OwnReason::NotPermitted => "you may not use servers of your own",
            OwnReason::EgressRefused => "this deployment does not allow connections to it",
            OwnReason::BrokerDown => "this deployment keeps no credentials",
        }
    }

    /// The `code` of the 422 that refuses an attach for this reason: `tool_server_needs_auth` when
    /// the person has something to do (give a key, authorize), `tool_server_unavailable` when a
    /// cause outside them has to clear.
    pub const fn attach_code(self) -> &'static str {
        match self {
            OwnReason::NotConnected | OwnReason::Reauthorize | OwnReason::Disconnected => {
                "tool_server_needs_auth"
            }
            OwnReason::NotPermitted | OwnReason::EgressRefused | OwnReason::BrokerDown => {
                "tool_server_unavailable"
            }
        }
    }

    /// The name the API writes.
    pub const fn as_str(self) -> &'static str {
        match self {
            OwnReason::NotConnected => "not_connected",
            OwnReason::Reauthorize => "reauthorize",
            OwnReason::Disconnected => "disconnected",
            OwnReason::NotPermitted => "not_permitted",
            OwnReason::EgressRefused => "egress_refused",
            OwnReason::BrokerDown => "broker_down",
        }
    }
}

/// One of the person's own servers as they are told of it: the definition, the revision to send back
/// with an edit, and what it is to the person now. **No secret** is in it, and `Debug` shows no path.
#[derive(Debug, Clone)]
pub struct OwnToolServer {
    server: UserToolServer,
    revision: ServerRevision,
    status: ToolServerStatus,
    reason: Option<OwnReason>,
}

impl OwnToolServer {
    /// The id, `u-` and ten characters.
    pub fn id(&self) -> &UserToolServerId {
        self.server.id()
    }

    /// The name the person gave.
    pub fn name(&self) -> &str {
        self.server.name()
    }

    /// The URL in its normal form: the person's own, shown back to them. It may hold a secret in its
    /// path; the answer carries `Cache-Control: no-store` and nothing logs it.
    pub fn url(&self) -> &str {
        self.server.url()
    }

    /// The kind of authentication: the header name, the scopes and the client id, never a secret.
    pub fn auth(&self) -> &UserAuth {
        self.server.auth()
    }

    /// Where the server is in the lifecycle of its credential.
    pub fn state(&self) -> UserToolServerState {
        self.server.state()
    }

    /// Whether a credential is kept for it (a key given, an authorization done). Not the credential.
    pub fn has_credential(&self) -> bool {
        self.server.credential().is_some()
    }

    /// The revision, to send back with an edit. Opaque: compare for equality only.
    pub fn revision(&self) -> ServerRevision {
        self.revision
    }

    /// `ready`, `needs_auth` or `unavailable`.
    pub fn status(&self) -> ToolServerStatus {
        self.status
    }

    /// Why it is not `ready`; `None` when it is.
    pub fn reason(&self) -> Option<OwnReason> {
        self.reason
    }
}

/// What a person's own servers read as, for a listing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum OwnServers {
    /// The feature is off here: there is nothing to list.
    Off,
    /// The person's servers, sorted by name and then id.
    Listed(Vec<OwnToolServer>),
    /// A definition of the person's does not read back, so none can be listed: the store fails a
    /// listing as a whole for one such row (`Corrupt`), and the port names no id. The other servers
    /// are not lost; each can still be deleted by its id (`delete` reads nothing).
    Unreadable,
}

// ------------------------------------------------------------------------------- the methods

impl<P: Ports> App<P> {
    /// The person's own servers, with what each is to them now (ADR 0046, decisions 2 and 4). It
    /// needs no permission; the route adds its own (`thread.write` for the picker).
    ///
    /// The status is computed from the definition and the deployment and **asks nobody**: no lease
    /// (a lease refreshes a token and would put a network call to an authorization server in every
    /// listing, for up to eight servers). The broker marks `needs_reauth` itself when it learns a
    /// grant ended, and the relay does when the broker says a credential ended or the server answers 401 or
    /// 403 (slice 5a); this is the state it reads.
    ///
    /// | Condition, first that holds | `status` | `reason` |
    /// |---|---|---|
    /// | the person lacks `tools.user.add` | `unavailable` | `not_permitted` |
    /// | today's egress lists refuse the URL | `unavailable` | `egress_refused` |
    /// | a credential kind, and no credentials are kept here | `unavailable` | `broker_down` |
    /// | state `connected` | `ready` | |
    /// | state `added` | `needs_auth` | `not_connected` |
    /// | state `needs_reauth` | `needs_auth` | `reauthorize` |
    /// | state `revoked` | `needs_auth` | `disconnected` |
    ///
    /// # Errors
    /// [`AppError::Store`] when the definitions cannot be read now.
    pub async fn list_own_tool_servers(
        &self,
        who: &impl Requester,
    ) -> Result<OwnServers, AppError> {
        let ports = self.user_tools();
        let mode = ports.mode();
        if !mode.is_enabled() {
            return Ok(OwnServers::Off);
        }
        let permitted = self.may_add_own(who);
        match ports.servers().list(who.user()).await {
            Ok(rows) => {
                let mut servers: Vec<OwnToolServer> = rows
                    .into_iter()
                    .map(|stored| describe(permitted, mode, ports.egress(), stored))
                    .collect();
                servers.sort_by_cached_key(|s| (s.name().to_lowercase(), s.id().clone()));
                Ok(OwnServers::Listed(servers))
            }
            Err(UserToolServerStoreError::Corrupt(_)) => {
                tracing::warn!(
                    "a definition of a person's own MCP server does not read back; none of theirs \
                     is listed until it is deleted by its id"
                );
                Ok(OwnServers::Unreadable)
            }
            Err(e) => Err(store_error(e)),
        }
    }

    /// Whether the server `id` may be attached to a thread of `owner`, whose request is `who`:
    /// the thread's **owner** has a server of that id and it is `ready` for them now (ADR 0046,
    /// decision 4). Asks nobody but the definitions: no lease, which would refresh a token for an
    /// attach.
    ///
    /// A server that is not the owner's, one that does not exist, a malformed id and a deployment
    /// that has the feature off are all [`AppError::Unprocessable`] with the sentence an unknown id
    /// has, so the answer does not tell whether somebody else has a server by that id. One of the
    /// owner's that is not `ready` is [`UserToolRefusal::NotReady`] with the reason the picker
    /// shows.
    pub(crate) async fn check_own_attach(
        &self,
        who: &impl Requester,
        owner: &orch_core::UserId,
        id: &str,
    ) -> Result<(), AppError> {
        let unknown =
            || AppError::Unprocessable(format!("no server `{id}` is offered for attaching"));
        let ports = self.user_tools();
        let mode = ports.mode();
        if !mode.is_enabled() {
            return Err(unknown());
        }
        let Ok(server) = UserToolServerId::parse(id) else {
            return Err(unknown());
        };
        let Some(stored) = ports
            .servers()
            .get(owner, &server)
            .await
            .map_err(store_error)?
        else {
            return Err(unknown());
        };
        let own = describe(self.may_add_own(who), mode, ports.egress(), stored);
        match own.reason() {
            None => Ok(()),
            Some(reason) => Err(UserToolRefusal::NotReady {
                id: id.to_owned(),
                reason,
            }
            .into()),
        }
    }

    /// Whether `owner` has a server of the id `id` (ADR 0046, decision 4: a fork carries the servers
    /// its new owner owns). Ownership is having the definition, not being `ready`: a server that
    /// needs a new key is still the person's, and is carried. A feature that is off, a malformed id
    /// and a server that is gone are `false`.
    ///
    /// # Errors
    /// [`AppError::Store`] when the definitions cannot be read now: a fork that could not be sure
    /// fails and is made again, rather than carrying a server it may not own or dropping one it does.
    pub(crate) async fn owns_user_tool_server(
        &self,
        owner: &orch_core::UserId,
        id: &str,
    ) -> Result<bool, AppError> {
        let ports = self.user_tools();
        if !ports.mode().is_enabled() {
            return Ok(false);
        }
        let Ok(server) = UserToolServerId::parse(id) else {
            return Ok(false);
        };
        Ok(ports
            .servers()
            .get(owner, &server)
            .await
            .map_err(store_error)?
            .is_some())
    }

    /// Adds a server of the person's own (`POST /api/user-tool-servers`). Needs `tools.user.add`.
    ///
    /// Everything is checked before the first write; then the order of the module's docs. A `none`
    /// server is `connected` at once; an `oauth2` server is `added` and waits for the person to
    /// authorize (slice 4c); a `bearer` or `api_key` server is `added`, its secret goes to the broker,
    /// and the definition is replaced as `connected` with the reference the broker gave. **A failed
    /// `store` takes the definition back** (after a best-effort `revoke`: a `put` that committed and
    /// whose answer was lost has kept the secret), so a failed add leaves nothing and costs no place
    /// **when the request runs to its end**: the route runs this in a task of its own, so that a
    /// client that hangs up or a timeout does not stop it between two writes. A `replace` that fails
    /// after a successful `store` revokes what was kept and leaves the definition `added`, which its
    /// owner deletes or re-keys.
    ///
    /// # Errors
    /// [`UserToolRefusal::Off`]; [`AppError::Forbidden`] without the permission;
    /// [`AppError::Invalid`] for a name, header, scope, client id or secret that cannot be used, a
    /// secret missing for a `bearer` or `api_key` server or given for another kind;
    /// [`UserToolRefusal::UrlRefused`]; [`UserToolRefusal::NoCredentials`];
    /// [`UserToolRefusal::TooMany`]; [`UserToolRefusal::CredentialsUnavailable`];
    /// [`UserToolRefusal::Busy`]; [`AppError::Store`].
    pub async fn add_user_tool_server(
        &self,
        who: &impl Requester,
        request: AddUserToolServer,
    ) -> Result<OwnToolServer, AppError> {
        let mode = self.enabled_user_tools()?;
        self.require_add_own(who)?;
        let ports = self.user_tools();
        let owner = who.user();
        let AddUserToolServer {
            name,
            url,
            auth,
            secret,
        } = request;

        // everything the person typed, before any write
        let target = check_url(ports.egress(), &url)?;
        auth.validate()
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        match (&secret, takes_secret(&auth)) {
            (Some(secret), true) => check_secret(secret)?,
            (None, true) => {
                return Err(AppError::Invalid(
                    "a bearer or api_key server needs its secret".to_owned(),
                ));
            }
            (Some(_), false) => return Err(secret_not_for_kind()),
            (None, false) => {}
        }
        if auth.needs_credential() && !mode.keeps_credentials() {
            return Err(UserToolRefusal::NoCredentials.into());
        }

        // a `none` server has nothing to wait for
        let first_state = if auth.needs_credential() {
            UserToolServerState::Added
        } else {
            UserToolServerState::Connected
        };
        let mut inserted = None;
        for _ in 0..ID_ATTEMPTS {
            let id = UserToolServerId::from_entropy(self.ports().ids().new_token_bytes());
            let server = UserToolServer::new(id, &name, &target, auth.clone())
                .map_err(|e| AppError::Invalid(e.to_string()))?
                .with_state(first_state);
            match ports.servers().insert(owner, &server).await {
                Ok(revision) => {
                    inserted = Some((server, revision));
                    break;
                }
                Err(UserToolServerStoreError::Exists) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        let Some((server, revision)) = inserted else {
            return Err(AppError::internal(
                "a generated server id was taken three times",
            ));
        };

        let Some(secret) = secret else {
            return Ok(describe(
                true,
                mode,
                ports.egress(),
                StoredServer::new(server, revision),
            ));
        };
        match ports.broker().store(owner, server.id(), &secret).await {
            Ok(credential) => {
                let connected = server
                    .clone()
                    .with_state(UserToolServerState::Connected)
                    .with_credential(credential);
                match ports.servers().replace(owner, &connected, revision).await {
                    Ok(revision) => Ok(describe(
                        true,
                        mode,
                        ports.egress(),
                        StoredServer::new(connected, revision),
                    )),
                    Err(e) => {
                        // the person removed or changed it in the milliseconds since the insert, or
                        // the store failed: the credential has no definition that names it
                        self.forget_unnamed(
                            owner,
                            server.id(),
                            "a credential kept for a server whose add did not finish",
                        )
                        .await;
                        Err(match e {
                            UserToolServerStoreError::Conflict => UserToolRefusal::Busy.into(),
                            other => store_error(other),
                        })
                    }
                }
            }
            Err(e) => {
                // a store that failed may still have kept the secret (a `put` that committed and
                // whose answer was lost): forget it before the definition that would be its only
                // name goes. `Unknown` is "no such server", and kept nothing
                if may_have_kept(&e) {
                    self.forget_unnamed(
                        owner,
                        server.id(),
                        "a credential a failed store may have kept",
                    )
                    .await;
                }
                // take the definition back, so the failed add costs no place and a retry does not
                // leave two
                if let Err(e) = ports.servers().delete(owner, server.id()).await {
                    tracing::warn!(
                        error = %report(&e),
                        "a server whose credential could not be kept could not be taken back; it stays `added` and its owner can delete it"
                    );
                }
                Err(credentials_failure("store", &e))
            }
        }
    }

    /// Changes a server of the person's own (`PATCH /api/user-tool-servers/{id}`). Needs
    /// `tools.user.add`.
    ///
    /// | The edit | Writes |
    /// |---|---|
    /// | a name, or the header of an `api_key` | `replace`; the credential is kept |
    /// | a secret, nothing else that matters | the revision read again, `store`, then `replace` as `connected` |
    /// | a URL, a kind, the scopes or the client id | `replace` as `added` with no credential, then `revoke`, then (with a secret) `store` and `replace` as `connected` |
    ///
    /// **The `replace` of a move comes first**, as a compare-and-swap on the revision that was read,
    /// so that an edit that loses a race or whose write fails has revoked nothing: the server still
    /// says `connected` and its credential is still there. A `revoke` that fails after it leaves the
    /// server `added` with no credential named (the sealed record that is left is not lent and the
    /// next key or the delete forgets it) and the answer is [`UserToolRefusal::CredentialsUnavailable`].
    /// A change to `none` makes the server `connected`.
    ///
    /// With `edit.revision` the edit is refused when the server changed since; without it the edit
    /// applies to the server as it is, read again up to three times when the broker or another request
    /// wrote in between ([`UserToolRefusal::Busy`] after). **Once the edit has written the definition
    /// its revision is spent**: a later conflict reads the server again and finishes the edit, and
    /// is never [`UserToolRefusal::StaleRevision`] over a move that was made.
    ///
    /// A new key replaces the old one in the broker, in place and for good: a key is kept only when
    /// the revision is still the one this pass read. When the compare and swap then finds the
    /// definition changed, the definition is read again: if it still names a credential the key
    /// stays (it may be `connected` and rely on it; the next pass keeps it again), and if it names
    /// none (another request moved the server) or is gone the key is forgotten, best effort.
    ///
    /// # Errors
    /// As [`add_user_tool_server`](Self::add_user_tool_server), and [`UserToolRefusal::NotFound`] for
    /// an id that is not the person's, [`UserToolRefusal::StaleRevision`], and
    /// [`AppError::Invalid`] for an edit that changes nothing.
    pub async fn edit_user_tool_server(
        &self,
        who: &impl Requester,
        id: &str,
        edit: EditUserToolServer,
    ) -> Result<OwnToolServer, AppError> {
        let mode = self.enabled_user_tools()?;
        self.require_add_own(who)?;
        let id = parse_id(id)?;
        let ports = self.user_tools();
        let EditUserToolServer {
            name,
            url,
            auth,
            secret,
            revision,
        } = edit;
        if name.is_none() && url.is_none() && auth.is_none() && secret.is_none() {
            return Err(AppError::Invalid(
                "nothing to change: give a name, a url, an auth or a secret".to_owned(),
            ));
        }

        // everything the person typed, before any read or write
        let target = url
            .as_deref()
            .map(|url| check_url(ports.egress(), url))
            .transpose()?;
        if let Some(auth) = &auth {
            auth.validate()
                .map_err(|e| AppError::Invalid(e.to_string()))?;
            if auth.needs_credential() && !mode.keeps_credentials() {
                return Err(UserToolRefusal::NoCredentials.into());
            }
        }
        if let Some(secret) = &secret {
            check_secret(secret)?;
        }

        let plan = EditPlan {
            mode,
            id,
            name,
            target,
            auth,
            secret,
        };
        let mut revision = revision;
        for _ in 0..EDIT_ATTEMPTS {
            let stored = match ports.servers().get(who.user(), &plan.id).await {
                Ok(Some(stored)) => stored,
                Ok(None) => return Err(UserToolRefusal::NotFound.into()),
                Err(e) => return Err(store_error(e)),
            };
            if revision.is_some_and(|expected| expected != stored.revision()) {
                return Err(UserToolRefusal::StaleRevision.into());
            }
            match self.edit_once(who.user(), &plan, stored).await? {
                Pass::Done(view) => return Ok(*view),
                Pass::Lost { written: true } => {
                    // the edit's own write spent the revision the person named: finish the edit on
                    // the server as it is now
                    revision = None;
                }
                // a revision the person named is the one they read: it is stale now
                Pass::Lost { written: false } if revision.is_some() => {
                    return Err(UserToolRefusal::StaleRevision.into());
                }
                Pass::Lost { written: false } => {}
            }
        }
        Err(UserToolRefusal::Busy.into())
    }

    /// Deletes a server of the person's own (`DELETE /api/user-tool-servers/{id}`). Needs **no**
    /// permission: a person whose `tools.user.add` was taken away can still remove what they added.
    ///
    /// In `Full` it is **revoke, then delete** for a kind that holds a credential, and a revoke that
    /// fails aborts the delete (the definition stays to try again, and one deleted first would leave a
    /// sealed credential that nothing names). In
    /// `WithoutCredentials` the broker refuses everything and the person must still be able to free
    /// the place: the definition is deleted and `revoke` is not called; a sealed credential, if one
    /// is left, is never lent (ADR 0046, departure 77). A definition that does not read back is
    /// deleted by its id as well, and its credential, if any, revoked, since its kind is not known.
    ///
    /// # Errors
    /// [`UserToolRefusal::Off`], [`UserToolRefusal::NotFound`] (also the second time),
    /// [`UserToolRefusal::CredentialsUnavailable`], [`AppError::Store`].
    pub async fn delete_user_tool_server(
        &self,
        who: &impl Requester,
        id: &str,
    ) -> Result<(), AppError> {
        let mode = self.enabled_user_tools()?;
        let id = parse_id(id)?;
        let ports = self.user_tools();
        let owner = who.user();
        let holds_credential = match ports.servers().get(owner, &id).await {
            Ok(Some(stored)) => stored.server().auth().needs_credential(),
            Ok(None) => return Err(UserToolRefusal::NotFound.into()),
            // by its id: the kind is not known, so whatever credential there is goes too
            Err(UserToolServerStoreError::Corrupt(_)) => true,
            Err(e) => return Err(store_error(e)),
        };
        if holds_credential && mode.keeps_credentials() {
            ports
                .broker()
                .revoke(owner, &id)
                .await
                .map_err(|e| credentials_failure("revoke", &e))?;
        }
        ports
            .servers()
            .delete(owner, &id)
            .await
            .map_err(store_error)
    }

    /// Whether this deployment has people's own MCP servers on: the routes are 404 when it has not.
    pub fn user_tool_servers_enabled(&self) -> bool {
        self.user_tools().mode().is_enabled()
    }

    /// The kinds of authentication this deployment takes for a server of one's own, as the API
    /// writes them: none when the feature is off, only `none` when the deployment keeps no
    /// credentials, all four otherwise. A hint for the screen; the checks are the methods'.
    pub fn user_tool_kinds(&self) -> &'static [&'static str] {
        match self.user_tools().mode() {
            UserToolsMode::Off => &[],
            UserToolsMode::WithoutCredentials => &["none"],
            UserToolsMode::Full => &["none", "bearer", "api_key", "oauth2"],
        }
    }

    // -------------------------------------------------------------------------- the helpers

    /// The mode, or [`UserToolRefusal::Off`].
    fn enabled_user_tools(&self) -> Result<UserToolsMode, AppError> {
        let mode = self.user_tools().mode();
        if mode.is_enabled() {
            Ok(mode)
        } else {
            Err(UserToolRefusal::Off.into())
        }
    }

    /// Whether the person holds `tools.user.add`.
    pub(crate) fn may_add_own(&self, who: &impl Requester) -> bool {
        self.access(who)
            .check(Permission::ToolsUserAdd, &Resource::Anything)
            .is_ok()
    }

    fn require_add_own(&self, who: &impl Requester) -> Result<(), AppError> {
        if self.may_add_own(who) {
            Ok(())
        } else {
            Err(AppError::missing_permission(Permission::ToolsUserAdd))
        }
    }

    /// One pass of an edit over the definition as it was just read.
    async fn edit_once(
        &self,
        owner: &orch_core::UserId,
        plan: &EditPlan,
        stored: StoredServer,
    ) -> Result<Pass, AppError> {
        let ports = self.user_tools();
        let old = stored.server();
        let name = plan.name.as_deref().unwrap_or_else(|| old.name());
        let url = plan.target.as_ref().map_or_else(|| old.url(), |t| t.url());
        let auth = plan.auth.clone().unwrap_or_else(|| old.auth().clone());
        let keyed = plan.secret.is_some();
        if keyed && !takes_secret(&auth) {
            return Err(secret_not_for_kind());
        }

        // what the credential was issued for: the URL, the kind, and for OAuth the scopes and the
        // client (the broker lends it for nothing else)
        let affecting = url != old.url() || !same_grant(old.auth(), &auth);
        if affecting && old.auth().needs_credential() && !plan.mode.keeps_credentials() {
            // there may be a credential to revoke and nobody to revoke it: delete the server instead
            return Err(UserToolRefusal::NoCredentials.into());
        }
        let (state, credential) = if !auth.needs_credential() {
            (UserToolServerState::Connected, None)
        } else if affecting {
            (UserToolServerState::Added, None)
        } else {
            (old.state(), old.credential().cloned())
        };
        let candidate =
            UserToolServer::from_stored(plan.id.clone(), name, url, auth, state, credential)
                .map_err(|e| AppError::Invalid(e.to_string()))?;
        if !keyed && candidate == *old {
            return Ok(Pass::Done(Box::new(describe(
                true,
                plan.mode,
                ports.egress(),
                stored,
            ))));
        }

        let finish = |server: UserToolServer, revision| {
            Pass::Done(Box::new(describe(
                true,
                plan.mode,
                ports.egress(),
                StoredServer::new(server, revision),
            )))
        };
        match (&plan.secret, affecting) {
            // a new key for a definition that stays what it is: one write, after the store
            (Some(secret), false) => {
                // The broker keeps a key in place of the old one, and cannot give the old one back
                // when the write below finds the definition changed. So the key is kept only when
                // the definition is still the one this pass read; the write below stays the check
                // for what happens in the milliseconds after.
                match ports.servers().get(owner, &plan.id).await {
                    Ok(Some(now)) if now.revision() == stored.revision() => {}
                    Ok(Some(_)) => return Ok(Pass::Lost { written: false }),
                    Ok(None) => return Err(UserToolRefusal::NotFound.into()),
                    Err(e) => return Err(store_error(e)),
                }
                let credential = ports
                    .broker()
                    .store(owner, &plan.id, secret)
                    .await
                    .map_err(|e| credentials_failure("store", &e))?;
                let connected = candidate
                    .with_state(UserToolServerState::Connected)
                    .with_credential(credential);
                match ports
                    .servers()
                    .replace(owner, &connected, stored.revision())
                    .await
                {
                    Ok(revision) => Ok(finish(connected, revision)),
                    // somebody wrote the definition between the check above and this write
                    Err(UserToolServerStoreError::Conflict) => {
                        self.settle_a_key_that_lost(owner, &plan.id).await;
                        Ok(Pass::Lost { written: false })
                    }
                    Err(UserToolServerStoreError::NotFound) => {
                        self.forget_unnamed(
                            owner,
                            &plan.id,
                            "a credential kept for a server that was deleted under the edit",
                        )
                        .await;
                        Err(UserToolRefusal::NotFound.into())
                    }
                    Err(e) => Err(store_error(e)),
                }
            }
            // the definition changes first (a compare-and-swap, so that a lost race has revoked
            // nothing), and the broker reads the new one when it stores
            (secret, _) => {
                let revision = match ports
                    .servers()
                    .replace(owner, &candidate, stored.revision())
                    .await
                {
                    Ok(revision) => revision,
                    Err(UserToolServerStoreError::Conflict) => {
                        return Ok(Pass::Lost { written: false });
                    }
                    Err(e) => return Err(store_error(e)),
                };
                // the definition no longer names the old credential: forget it. A failure leaves the
                // server `added`, and what the broker still holds is bound to the old URL or grant
                if affecting && old.auth().needs_credential() {
                    ports
                        .broker()
                        .revoke(owner, &plan.id)
                        .await
                        .map_err(|e| credentials_failure("revoke", &e))?;
                }
                let Some(secret) = secret else {
                    return Ok(finish(candidate, revision));
                };
                // the old credential is gone and the new definition is saved: a failure here leaves
                // the server `added`, and the person gives the key again
                let credential = ports
                    .broker()
                    .store(owner, &plan.id, secret)
                    .await
                    .map_err(|e| credentials_failure("store", &e))?;
                let connected = candidate
                    .with_state(UserToolServerState::Connected)
                    .with_credential(credential);
                match ports.servers().replace(owner, &connected, revision).await {
                    Ok(revision) => Ok(finish(connected, revision)),
                    // somebody wrote the definition since (the broker marked it, or another request):
                    // the next pass reads it (new URL, no credential named) and keeps the key again
                    Err(UserToolServerStoreError::Conflict) => Ok(Pass::Lost { written: true }),
                    Err(e) => {
                        // the definition is the `added` one written above and nobody else's, so
                        // nothing names the key that was just kept
                        self.forget_unnamed(
                            owner,
                            &plan.id,
                            "a credential kept for a server whose edit did not finish",
                        )
                        .await;
                        Err(store_error(e))
                    }
                }
            }
        }
    }

    /// A key was kept for an edit whose `replace` then lost to another writer. The broker keeps a key
    /// in place of the old one and cannot give the old one back, so the question is what the
    /// definition is now, and it is asked again (best effort, nothing here is reported):
    ///
    /// - it still **names a credential** (a rename, the broker marking the grant, another key): it
    ///   relies on what the broker holds, which is the key just kept. Revoking it would leave a
    ///   `connected` server with nothing behind it, so it stays, and the next pass keeps it again;
    /// - it **names none** (another request moved the server): nothing relies on the key, and the
    ///   broker, which read the definition when it kept the key, may have bound it to the new URL
    ///   and would lend it there. It is forgotten;
    /// - it is **gone**: nothing names the key, and it is forgotten.
    ///
    /// A forgetting that lands between the other request's `store` and its `replace` takes that
    /// request's key too; the window is the two writes of an edit that moves a server with a key, and
    /// the person is then told `needs_auth` at the next lease (a recorded risk, ADR 0046, departure
    /// 103).
    async fn settle_a_key_that_lost(&self, owner: &orch_core::UserId, id: &UserToolServerId) {
        match self.user_tools().servers().get(owner, id).await {
            Ok(Some(now)) if now.server().credential().is_some() => {}
            Ok(Some(_)) => {
                self.forget_unnamed(
                    owner,
                    id,
                    "a credential kept for a server that was moved under the edit",
                )
                .await;
            }
            Ok(None) => {
                self.forget_unnamed(
                    owner,
                    id,
                    "a credential kept for a server that was deleted under the edit",
                )
                .await;
            }
            // nothing is known of the definition: leave the key, the next read says
            Err(e) => tracing::warn!(
                error = %report(&e),
                "a credential kept for an edit that lost a race could not be settled"
            ),
        }
    }

    /// Forgets a credential that no definition names, best effort: the caller has its own answer to
    /// give, and a failure here is logged and never reported (a sealed record nothing names is not
    /// lent, ADR 0046 departure 77).
    async fn forget_unnamed(
        &self,
        owner: &orch_core::UserId,
        id: &UserToolServerId,
        what: &'static str,
    ) {
        if let Err(e) = self.user_tools().broker().revoke(owner, id).await {
            tracing::warn!(error = %report(&e), "{what} could not be forgotten");
        }
    }
}

/// How one pass of an edit ended.
enum Pass {
    /// The edit is done: the server as it is now.
    Done(Box<OwnToolServer>),
    /// A race was lost and the pass undid nothing it should have kept: read the server and go on.
    /// `written` says whether the pass had already written the definition (the person's revision is
    /// spent then).
    Lost { written: bool },
}

/// What an edit means, once what the person typed has been checked.
struct EditPlan {
    mode: UserToolsMode,
    id: UserToolServerId,
    name: Option<String>,
    target: Option<EgressTarget>,
    auth: Option<UserAuth>,
    secret: Option<ToolSecret>,
}

// --------------------------------------------------------------------------- free functions

/// A server of these kinds takes a secret the person types.
const fn takes_secret(auth: &UserAuth) -> bool {
    matches!(auth, UserAuth::Bearer | UserAuth::ApiKey { .. })
}

fn secret_not_for_kind() -> AppError {
    AppError::Invalid("a secret is only for a bearer or api_key server".to_owned())
}

/// Whether a credential issued for `old` is still the right one for `new` (the URL aside): the same
/// kind, and for OAuth the same scopes (as a set) and client. A new header name for an `api_key`
/// does not matter: the header is the definition's, not part of what the broker keeps.
fn same_grant(old: &UserAuth, new: &UserAuth) -> bool {
    match (old, new) {
        (UserAuth::None, UserAuth::None) | (UserAuth::Bearer, UserAuth::Bearer) => true,
        (UserAuth::ApiKey { .. }, UserAuth::ApiKey { .. }) => true,
        (
            UserAuth::OAuth2 {
                scopes: a,
                client_id: client_a,
            },
            UserAuth::OAuth2 {
                scopes: b,
                client_id: client_b,
            },
        ) => {
            client_a == client_b
                && a.iter().collect::<BTreeSet<_>>() == b.iter().collect::<BTreeSet<_>>()
        }
        (
            UserAuth::None | UserAuth::Bearer | UserAuth::ApiKey { .. } | UserAuth::OAuth2 { .. },
            _,
        ) => false,
    }
}

/// The egress guard's reading of a URL a person typed. A refusal is what a person is shown
/// (`public_reason`), never the precise rule (departure 17).
fn check_url(egress: &EgressPolicy, text: &str) -> Result<EgressTarget, AppError> {
    egress.check_url(text).map_err(|refusal| {
        UserToolRefusal::UrlRefused {
            reason: refusal.public_reason(),
        }
        .into()
    })
}

/// A secret a header can carry: [`MIN_SCRUBBABLE_SECRET_BYTES`] (8) to [`MAX_USER_SECRET_BYTES`]
/// printable ASCII characters (spaces inside are fine), none at either end. Nothing of it is in the
/// message. The minimum is the shortest value the relay can take out of what a stranger's server says
/// back without redacting ordinary words (ADR 0046, slice 5a review): a shorter one would be said
/// back whole and not found, and no real token or key is that short.
fn check_secret(secret: &ToolSecret) -> Result<(), AppError> {
    let text = secret.expose();
    let printable = text.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if text.len() < MIN_SCRUBBABLE_SECRET_BYTES
        || text.len() > MAX_USER_SECRET_BYTES
        || !printable
        || text.starts_with(' ')
        || text.ends_with(' ')
    {
        return Err(AppError::Invalid(format!(
            "the secret must be {MIN_SCRUBBABLE_SECRET_BYTES} to {MAX_USER_SECRET_BYTES} printable \
             ASCII characters with no space at either end"
        )));
    }
    Ok(())
}

/// Whether `id` is shaped like the id of a person's own server (`u-` and ten characters). The
/// deployment's list is asked first by every caller: a server it lists is its own whatever its id
/// looks like (the startup refuses `u-` ids in the list only while people's servers are on).
pub(crate) fn is_own_server_id(id: &str) -> bool {
    id.starts_with(UserToolServerId::PREFIX)
}

/// A malformed id is a server that is not there.
fn parse_id(text: &str) -> Result<UserToolServerId, AppError> {
    UserToolServerId::parse(text).map_err(|_| UserToolRefusal::NotFound.into())
}

/// What the person is told of `stored`: status and reason from the definition and the deployment,
/// asking nobody (see [`App::list_own_tool_servers`]).
fn describe(
    permitted: bool,
    mode: UserToolsMode,
    egress: &EgressPolicy,
    stored: StoredServer,
) -> OwnToolServer {
    let server = stored.server();
    let (status, reason) = if !permitted {
        (
            ToolServerStatus::Unavailable(UnavailableReason::NotPermitted),
            Some(OwnReason::NotPermitted),
        )
    } else if egress.check_url(server.url()).is_err() {
        (
            ToolServerStatus::Unavailable(UnavailableReason::EgressRefused),
            Some(OwnReason::EgressRefused),
        )
    } else if server.auth().needs_credential() && !mode.keeps_credentials() {
        (
            ToolServerStatus::Unavailable(UnavailableReason::BrokerDown),
            Some(OwnReason::BrokerDown),
        )
    } else {
        let reason = match server.state() {
            UserToolServerState::Connected => None,
            UserToolServerState::Added => Some(OwnReason::NotConnected),
            UserToolServerState::NeedsReauth => Some(OwnReason::Reauthorize),
            UserToolServerState::Revoked => Some(OwnReason::Disconnected),
        };
        (server.state().status(), reason)
    };
    OwnToolServer {
        revision: stored.revision(),
        server: stored.into_server(),
        status,
        reason,
    }
}

/// A failure of the definitions' store, as the application says it: the storage classes of the
/// thread store, so the API answers them as it answers those.
fn store_error(error: UserToolServerStoreError) -> AppError {
    match error {
        UserToolServerStoreError::TooMany { limit } => UserToolRefusal::TooMany { limit }.into(),
        UserToolServerStoreError::NotFound => UserToolRefusal::NotFound.into(),
        UserToolServerStoreError::Conflict => UserToolRefusal::StaleRevision.into(),
        UserToolServerStoreError::Unavailable { .. } => {
            AppError::Store(StoreError::unavailable(error))
        }
        UserToolServerStoreError::Corrupt(_) => AppError::Store(StoreError::corrupt_with(
            "a definition of a person's own MCP server does not read back",
            error,
        )),
        UserToolServerStoreError::Exists => {
            AppError::internal("a server id the store reported as taken was not retried")
        }
        // a variant added later is a bug until it is mapped
        other => AppError::Store(StoreError::internal(other)),
    }
}

/// Whether a failed `store` may have kept the secret: anything but "no such server" (and a kind that
/// has no credential), where it did not get so far.
fn may_have_kept(error: &CredentialError) -> bool {
    !matches!(error, CredentialError::Unknown)
}

/// A failure of the broker, as the application says it. The cause is logged here, by class and chain
/// (the port promises neither holds a secret, a token or a URL), and never sent.
fn credentials_failure(step: &'static str, error: &CredentialError) -> AppError {
    match error {
        // the server is gone: someone deleted it under the request
        CredentialError::Unknown => UserToolRefusal::NotFound.into(),
        CredentialError::Unavailable { .. } | CredentialError::NeedsReauth => {
            tracing::warn!(
                step,
                class = ?error.class(),
                error = %report(error),
                "the credential broker could not keep or remove a credential"
            );
            UserToolRefusal::CredentialsUnavailable.into()
        }
        // a variant added later is no reason to guess: say it is down
        _ => UserToolRefusal::CredentialsUnavailable.into(),
    }
}
