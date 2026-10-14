//! The secret store port: where the opaque material of a person's credentials is kept, sealed
//! (ADR 0046, the amendment of 2026-10-11, ADR 0009).
//!
//! The credential broker ([`CredentialBroker`](crate::CredentialBroker)) is the orchestrator's own
//! and runs the OAuth work; **the vault is behind this port**, so a deployment can keep the
//! material in its database (`orch-secrets-pg`, sealed), in Kubernetes Secrets or in a secrets
//! manager, by building one adapter crate and naming it in the configuration. The port knows
//! nothing of tokens: it keeps **opaque bytes** ([`SecretMaterial`]) under an owner and a
//! [`SecretServer`], and the broker serialises into it whatever it needs (a token set and its
//! expiry, the PKCE verifier of a flow that is under way, a client secret).
//!
//! | Call | Does |
//! |---|---|
//! | [`put`](SecretStore::put) | Creates or replaces; returns the new [`SecretVersion`]. |
//! | [`get`](SecretStore::get) | The version and the material; absent is `None`, not an error. |
//! | [`delete`](SecretStore::delete) | Removes; idempotent. |
//! | [`rotate`](SecretStore::rotate) | **Compare and swap**: replaces only when `expected` is the stored version, else [`SecretError::Conflict`]. |
//!
//! `rotate` is how a refreshed, rotated token set is stored without a lost update between
//! replicas: both read version 7, both refresh, one rotates from 7 and wins, the other gets a
//! conflict, reads again, and uses the winner's. It prevents a lost update, not a double spend of a
//! refresh token (ADR 0046, "kept open", 3).
//!
//! # What an implementation owes
//!
//! - **Sealed at rest**, in a way it chooses. The port neither says how nor looks.
//! - **Nothing of the material in an error, a log line or a `Debug`.** The conformance testkit
//!   plants a sentinel and looks for it everywhere.
//! - **A key is `(owner, server)`, and an owner never reaches another's**: with the same server id,
//!   two owners hold two secrets.
//! - **Versions that mean what `rotate` needs, and no more.** A caller compares two versions for
//!   **equality** and for nothing else. An adapter gives a version to every write of a key
//!   (`put` and `rotate`, even one that writes what is already there), and
//!   1. **never gives one twice for a key, not even across a `delete`**: a caller that read version
//!      3, and then saw the secret revoked and made again, must not be able to rotate from what it
//!      read and overwrite the new grant with a refresh of the old one. A counter that restarts at 1
//!      for a new row is not enough; one that is kept for the key (or for the whole store) is;
//!   2. **serializes the writers of one key and numbers them in the order they land**, so the
//!      version a key holds is the greatest it has given (the testkit's concurrent case asserts it);
//!      the numbers say nothing across two keys, and a caller must not use them to order anything.
//! - **No name derived from a string a person chose**: an adapter names what it creates by a hash of
//!   `(owner, server)` (as [`ArtifactKey`](crate::ArtifactKey) does for files).

use std::fmt;
use std::future::Future;
use std::str::FromStr;

use orch_core::{BoxError, Classify, ErrorClass, UserId};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use crate::user_tools::{DefinitionError, UserToolServerId};

/// The most bytes of one [`SecretMaterial`]: 16 KiB, more than a token set, a verifier and a client
/// secret of any server need, and little enough for every adapter to seal and keep.
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// Material over [`MAX_SECRET_BYTES`] was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("secret material is at most {MAX_SECRET_BYTES} bytes")]
pub struct MaterialTooLarge {
    /// How many bytes were offered (the length, never the material).
    pub len: usize,
}

impl Classify for MaterialTooLarge {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// Opaque secret bytes: at most [`MAX_SECRET_BYTES`], wiped from memory when dropped.
///
/// `Debug` shows `<redacted>` and there is no `Display`; [`expose`](SecretMaterial::expose) is the
/// one way to read it, for the code that needs the bytes (the broker, an adapter sealing them). The
/// bytes may be empty. It is `Clone` (each copy is wiped on its own), and deliberately not
/// `PartialEq` or `Serialize`: nothing compares secrets with `==`, and nothing writes one to JSON.
///
/// The wipe covers the copy this type holds. A buffer the caller built the bytes in, and a copy an
/// adapter makes while sealing, are theirs to wipe.
///
/// What makes this a promise the compiler keeps: `ZeroizeOnDrop` is **derived**, and the derive
/// refuses to compile unless every field is itself wiped (a `Zeroizing` or a type that is
/// `Zeroize`). A field of any other type, added later, is a compile error and not a gap. (That the
/// wipe really clears the memory is `zeroize`'s to prove: `tests/secrets.rs` does not.)
#[derive(Clone, ZeroizeOnDrop)]
pub struct SecretMaterial(Zeroizing<Vec<u8>>);

impl SecretMaterial {
    /// Wraps `bytes`.
    ///
    /// # Errors
    /// [`MaterialTooLarge`] over [`MAX_SECRET_BYTES`]; the bytes offered are wiped.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, MaterialTooLarge> {
        // wrapped first, so that bytes refused are wiped as well
        let bytes = Zeroizing::new(bytes.into());
        if bytes.len() > MAX_SECRET_BYTES {
            return Err(MaterialTooLarge { len: bytes.len() });
        }
        Ok(SecretMaterial(bytes))
    }

    /// The bytes, for the one place that needs them.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The version of a stored secret: a number that every write of a key makes a new one of. Opaque to
/// callers, who compare it **for equality only** (to [`rotate`](SecretStore::rotate)). It is ordered
/// for the adapters and their testkit (a key's versions increase), not for callers: see the module's
/// docs for what an adapter owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretVersion(u64);

impl SecretVersion {
    /// The version `number`. For adapters, and for a test.
    pub const fn new(number: u64) -> Self {
        SecretVersion(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SecretVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The second half of a secret's key (the first is the owner): the server whose credential it is,
/// or the pending flow of that server.
///
/// The broker keeps two things per server, so there are two slots: the **credential** (the token
/// set, the key or the client secret, `u-abcdefghij`) and the **pending flow**
/// (`u-abcdefghij#flow`: the PKCE verifier and the state of an authorization that has begun and not
/// ended). The text form, which [`Display`](fmt::Display) writes and [`parse`](Self::parse) reads,
/// is what an adapter hashes into the name of what it creates and binds into what it seals.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SecretServer {
    server: UserToolServerId,
    flow: bool,
}

impl SecretServer {
    /// The credential of `server`.
    pub fn credential(server: &UserToolServerId) -> Self {
        SecretServer {
            server: server.clone(),
            flow: false,
        }
    }

    /// The pending authorization flow of `server`.
    pub fn pending_flow(server: &UserToolServerId) -> Self {
        SecretServer {
            server: server.clone(),
            flow: true,
        }
    }

    /// Reads the text form: `u-abcdefghij` or `u-abcdefghij#flow`.
    ///
    /// # Errors
    /// [`DefinitionError::BadId`] for anything else.
    pub fn parse(text: &str) -> Result<Self, DefinitionError> {
        match text.strip_suffix("#flow") {
            Some(id) => Ok(Self::pending_flow(&UserToolServerId::parse(id)?)),
            None => Ok(Self::credential(&UserToolServerId::parse(text)?)),
        }
    }

    /// The server.
    pub fn server_id(&self) -> &UserToolServerId {
        &self.server
    }

    /// Whether this is the pending flow of the server and not its credential.
    pub fn is_pending_flow(&self) -> bool {
        self.flow
    }
}

impl fmt::Display for SecretServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.flow {
            write!(f, "{}#flow", self.server)
        } else {
            write!(f, "{}", self.server)
        }
    }
}

impl FromStr for SecretServer {
    type Err = DefinitionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// Why a secret store did not do what it was asked.
///
/// None holds material, a key or an owner: a `detail` is plain text about what was attempted, and
/// the lower error is the `source`, which an adapter has checked for secrets before it boxes it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SecretError {
    /// [`rotate`](SecretStore::rotate) was given a version that is not the stored one: the secret
    /// changed, or went, since it was read. Read again.
    #[error("the secret changed since it was read")]
    Conflict,
    /// The store could not be reached or failed (a database that is down, a refused connection).
    #[error("the secret store is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without paths, URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// A stored secret is sealed under a key this deployment does not hold (the key was removed
    /// before every row was re-sealed). That server is unavailable; nothing else is.
    #[error("a secret is sealed under a key this deployment does not hold")]
    KeyUnknown,
    /// What the store holds is not a secret it wrote: an envelope that does not open, a row moved to
    /// another owner, server or version. Alert.
    #[error("the secret store holds corrupt data: {0}")]
    Corrupt(String),
}

impl SecretError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        SecretError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The store holds something that is not a secret it wrote. `detail` says what, never the bytes.
    pub fn corrupt(detail: impl Into<String>) -> Self {
        SecretError::Corrupt(detail.into())
    }

    /// Keeps `source` as the cause of an `Unavailable` error; any other variant is returned
    /// unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            SecretError::Unavailable { detail, .. } => SecretError::Unavailable {
                detail,
                source: Some(source.into()),
            },
            other @ (SecretError::Conflict | SecretError::KeyUnknown | SecretError::Corrupt(_)) => {
                other
            }
        }
    }
}

impl Classify for SecretError {
    fn class(&self) -> ErrorClass {
        match self {
            SecretError::Conflict => ErrorClass::Conflict,
            SecretError::Unavailable { .. } => ErrorClass::Transient,
            // not retryable, and an operator is told: the key was removed too early, or a row is
            // not what was written
            SecretError::KeyUnknown | SecretError::Corrupt(_) => ErrorClass::Corrupt,
        }
    }
}

/// Where the opaque material of credentials is kept, by `(owner, server)`.
///
/// See the module's docs for what an implementation owes. The calls are individually atomic:
/// a `put` or a `rotate` is wholly there or not at all, and `rotate` is atomic with its version
/// check. The writers of one key are serialized by the implementation.
pub trait SecretStore: Send + Sync + 'static {
    /// Keeps `material` under `(owner, server)`: creates the secret, or **replaces** the one there
    /// whatever its version. Returns the new version: one this key has never had, even when the
    /// material is the same as what was kept.
    ///
    /// # Errors
    /// [`SecretError`]; its class says whether asking again can help.
    fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> impl Future<Output = Result<SecretVersion, SecretError>> + Send;

    /// The version and the material under `(owner, server)`; `None` when there is none.
    ///
    /// # Errors
    /// [`SecretError::KeyUnknown`] and [`SecretError::Corrupt`] for a row that cannot be opened;
    /// [`SecretError::Unavailable`] when the store fails.
    fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> impl Future<Output = Result<Option<(SecretVersion, SecretMaterial)>, SecretError>> + Send;

    /// Removes the secret under `(owner, server)`. **Idempotent**: removing what is not there
    /// succeeds.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] when the store fails.
    fn delete(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> impl Future<Output = Result<(), SecretError>> + Send;

    /// **Compare and swap**: replaces the material under `(owner, server)` with `material` only
    /// when the stored version is `expected`, and returns the new version. A secret that is not
    /// there, or is at another version, is [`SecretError::Conflict`], and nothing is written.
    ///
    /// # Errors
    /// [`SecretError::Conflict`]; [`SecretError`] otherwise as for the other calls.
    fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> impl Future<Output = Result<SecretVersion, SecretError>> + Send;
}

/// The secret store of a deployment that keeps none (`secrets` absent from the configuration, or
/// `userToolServers` off): every call is [`SecretError::Unavailable`] with a sentence that says why
/// and holds no owner, server or material. The model is
/// [`NoArtifacts`](crate::NoArtifacts); it does not pass the conformance suite, which is for the
/// stores that keep.
///
/// It refuses `get` too (absent is `None` for a store that keeps; here that would read as "this
/// server has no credential" when the truth is "nothing can be kept"), and the class is
/// `Transient` as for any store that cannot be reached: no port signature or error type is added
/// for it.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSecrets;

impl NoSecrets {
    fn refuse<T>() -> Result<T, SecretError> {
        Err(SecretError::unavailable(
            "no secret store is configured (the secrets section is absent from the configuration, \
             or userToolServers is off)",
        ))
    }
}

impl SecretStore for NoSecrets {
    async fn put(
        &self,
        _owner: &UserId,
        _server: &SecretServer,
        _material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        Self::refuse()
    }

    async fn get(
        &self,
        _owner: &UserId,
        _server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        Self::refuse()
    }

    async fn delete(&self, _owner: &UserId, _server: &SecretServer) -> Result<(), SecretError> {
        Self::refuse()
    }

    async fn rotate(
        &self,
        _owner: &UserId,
        _server: &SecretServer,
        _expected: SecretVersion,
        _material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        Self::refuse()
    }
}
