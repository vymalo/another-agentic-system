//! The pending authorizations port: what the orchestrator keeps between the moment a person is sent
//! to an authorization server and the moment the browser comes back with the `code` (ADR 0046, the
//! amendment of 2026-10-06 and slice 4c, ADR 0009).
//!
//! The browser that reaches the OAuth callback is **not the app's session**: on the desktop and on
//! mobile it carries neither the app's bearer token nor the web's cookie (ADR 0047), so the callback
//! cannot know who the person is from the request. It knows from the `state` alone. When the signed-in
//! person starts the flow ([`begin_oauth`](crate::CredentialBroker::begin_oauth)), the orchestrator
//! writes `state → (who, which server, until when)` here; the callback reads it back **once**.
//! Everything else of the flow, the PKCE verifier above all, stays sealed in the broker.
//!
//! | Call | Does |
//! |---|---|
//! | [`put`](PendingAuthorizations::put) | Keeps the state for `(owner, server)` until it ends; replaces what that server had pending. |
//! | [`take`](PendingAuthorizations::take) | Removes the state and gives back who and which server, **at most once**; an unknown, used or ended state is `None`. |
//!
//! # What an implementation owes
//!
//! - **At most once.** Of any number of callers that [`take`](PendingAuthorizations::take) one state,
//!   at the same time or one after the other, exactly one gets the owner and the server. The removal
//!   and the read are one atomic step (`DELETE … RETURNING`), never a read and then a delete.
//! - **An ended state is nothing.** A state is valid while `now < expires_at`; at `expires_at` and
//!   after, [`take`](PendingAuthorizations::take) gives `None`, and the state is forgotten.
//! - **The state is a bearer value, so it is not kept.** The adapter keeps its SHA-256
//!   ([`StateDigest`]), never the text: a copy of the table does not hold working states. A digest of
//!   a state of 96 random bits or more cannot be turned back into one.
//! - **One pending authorization per server.** [`put`](PendingAuthorizations::put) forgets the state a
//!   server (of this owner) had pending: the broker keeps one pending flow per `(owner, server)` too,
//!   and the state of the flow it replaced is no longer one. This also bounds the table: it holds at
//!   most one row per server of each person, whatever the number of times they authorize. A state that
//!   is put twice belongs to the later put.
//! - **Growth is bounded by the caller's clock.** [`put`](PendingAuthorizations::put) is given
//!   `now` and forgets every state that ended at or before it, so abandoned flows (a person who closed
//!   the tab) do not accumulate.
//! - **Nothing in an error, a log line or a `Debug` names a state, an owner or a server.**
//!
//! What a `take` that finds a state does with the rest of the flow is the caller's: the broker's
//! pending flow expires on its own, and an `error=` callback needs no call (see
//! [`begin_oauth`](crate::CredentialBroker::begin_oauth)).

use std::fmt;
use std::future::Future;

use jiff::Timestamp;
use orch_core::{BoxError, Classify, ErrorClass, UserId};
use sha2::{Digest, Sha256};

use crate::credentials::OAuthState;
use crate::user_tools::UserToolServerId;

/// The SHA-256 of an [`OAuthState`]: what an adapter keeps in place of the state, which is a bearer
/// value that completes a person's authorization.
///
/// `Debug` shows nothing of the bytes: a digest is not a secret (it cannot be turned back into a
/// state that has the entropy of one the broker makes), but nothing needs it in a log.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateDigest([u8; 32]);

impl StateDigest {
    /// The digest of `state`.
    pub fn of(state: &OAuthState) -> Self {
        StateDigest(Sha256::digest(state.as_str().as_bytes()).into())
    }

    /// The 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for StateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StateDigest(…)")
    }
}

/// Who began an authorization and for which server: what the callback learns from the `state` alone.
///
/// `Debug` shows the server (`u-` and ten characters, which says nothing of the person) and not the
/// owner.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingAuthorization {
    owner: UserId,
    server: UserToolServerId,
}

impl PendingAuthorization {
    /// The authorization of `server` that `owner` began.
    pub fn new(owner: UserId, server: UserToolServerId) -> Self {
        PendingAuthorization { owner, server }
    }

    /// The person who began it.
    pub fn owner(&self) -> &UserId {
        &self.owner
    }

    /// The server they began it for.
    pub fn server(&self) -> &UserToolServerId {
        &self.server
    }

    /// Takes the pieces apart.
    pub fn into_parts(self) -> (UserId, UserToolServerId) {
        (self.owner, self.server)
    }
}

impl fmt::Debug for PendingAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingAuthorization")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

/// Why the pending authorizations could not be kept or read. None holds a state, an owner or a
/// server.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PendingAuthorizationError {
    /// The store could not be reached or failed. Nothing was taken: a state that was not read is
    /// still there.
    #[error("the store of pending authorizations is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without states, owners, servers or URLs.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
}

impl PendingAuthorizationError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        PendingAuthorizationError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            PendingAuthorizationError::Unavailable { detail, .. } => {
                PendingAuthorizationError::Unavailable {
                    detail,
                    source: Some(source.into()),
                }
            }
        }
    }
}

impl Classify for PendingAuthorizationError {
    fn class(&self) -> ErrorClass {
        match self {
            PendingAuthorizationError::Unavailable { .. } => ErrorClass::Transient,
        }
    }
}

/// The authorizations that have begun and whose callback has not come: `state → (owner, server,
/// until)`, used once. See the module's docs for what an implementation owes.
pub trait PendingAuthorizations: Send + Sync + 'static {
    /// Keeps `state` for `owner`'s `server` until `expires_at`, and forgets what that server had
    /// pending before, every state that ended at or before `now`, and an earlier put of the same
    /// state.
    ///
    /// # Errors
    /// [`PendingAuthorizationError::Unavailable`] when the store fails; nothing was kept.
    fn put(
        &self,
        state: &OAuthState,
        owner: &UserId,
        server: &UserToolServerId,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> impl Future<Output = Result<(), PendingAuthorizationError>> + Send;

    /// Removes `state` and gives back who began it and for which server, if it is pending and
    /// `now < expires_at`; otherwise `None` (never put, taken, replaced by a later put of the same
    /// server, or ended). **At most once**: see the module's docs.
    ///
    /// # Errors
    /// [`PendingAuthorizationError::Unavailable`] when the store fails; the state is then still
    /// pending.
    fn take(
        &self,
        state: &OAuthState,
        now: Timestamp,
    ) -> impl Future<Output = Result<Option<PendingAuthorization>, PendingAuthorizationError>> + Send;
}

/// The store of a deployment that keeps no pending authorizations (`userToolServers` off, or no
/// `secrets` section, so nothing can be authorized): every call is
/// [`PendingAuthorizationError::Unavailable`] with a sentence that says why. A caller asks
/// [`UserToolPorts::mode`](crate::UserToolPorts::mode) first and does not call it; the model is
/// [`NoBroker`](crate::NoBroker). It does not pass the conformance suite, which is for the stores
/// that keep.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPendingAuthorizations;

impl NoPendingAuthorizations {
    fn refuse<T>() -> Result<T, PendingAuthorizationError> {
        Err(PendingAuthorizationError::unavailable(
            "no authorization can be pending (the secrets section is absent from the configuration, \
             or userToolServers is off)",
        ))
    }
}

impl PendingAuthorizations for NoPendingAuthorizations {
    async fn put(
        &self,
        _state: &OAuthState,
        _owner: &UserId,
        _server: &UserToolServerId,
        _expires_at: Timestamp,
        _now: Timestamp,
    ) -> Result<(), PendingAuthorizationError> {
        Self::refuse()
    }

    async fn take(
        &self,
        _state: &OAuthState,
        _now: Timestamp,
    ) -> Result<Option<PendingAuthorization>, PendingAuthorizationError> {
        Self::refuse()
    }
}
