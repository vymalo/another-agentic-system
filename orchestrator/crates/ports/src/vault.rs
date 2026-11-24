//! The vault port: a person's **named** secrets, a new version for every write, and a reference a
//! configuration can name without holding the value (ADR 0083, the amendment of 2026-11-21, and
//! decision row 114 of 2026-11-22: the vault makes the secret, a pasted key is checked before it is
//! saved, **an agent gets only a reference**, and a rotation keeps the old version for 24 hours).
//!
//! The agent writes `vault:<name>#v<n>` ([`VaultRef`]) into a configuration and the platform
//! injects the value at call time. The name is the person's own name for the secret, in their own
//! vault (the owner is the first half of every key here, as in
//! [`SecretStore`](crate::SecretStore)); the version is **always explicit**, because a reference
//! that named none would change meaning under a rotation and a configuration must not. A
//! reference is not a secret: it is safe in a log, a message and a transcript, which is why an
//! agent may write one.
//!
//! | Call | Does |
//! |---|---|
//! | [`put`](VaultStore::put) | Writes a version: `#v1` for a name never written, else one more than the greatest the name has had. Returns the reference. |
//! | [`get`](VaultStore::get) | The material a reference names. A reference the store does not hold is an error — never `None`, so no caller reads "absent" as "nothing to inject". |
//! | [`purge`](VaultStore::purge) | Removes the versions whose [`SUPERSEDED_GRACE`] is over, and nothing else. Housekeeping: correctness never depends on it. |
//!
//! # Why this is not [`SecretStore`](crate::SecretStore)
//!
//! That port keeps **one** secret per `(owner, server)` — a tool server's credential, at a version
//! that only its compare-and-swap `rotate` reads — and a write replaces it. A named secret is a
//! different thing: a write **adds** a version, and the versions before it stay readable, so a run
//! that resolved `#v3` finishes on it after a rotation made `#v4`. Widening `SecretStore` with a
//! name and a retained history would change every one of its implementations and every caller for
//! the benefit of one, so the two are two ports over whatever a deployment seals with.
//!
//! # What an implementation owes
//!
//! - **Sealed at rest**, in a way it chooses; the port neither says how nor looks.
//! - **Nothing of the material in an error, a log line or a `Debug`.** The conformance testkit
//!   plants a sentinel and looks for it everywhere, in hex as well.
//! - **The version a reference names, and no other.** A row moved to another owner, another name or
//!   another version does not open.
//! - **Versions that only go up, for a name, for ever.** The first write of a name is `#v1`; every
//!   later one is the greatest that name has had, plus one. A version is never given twice — not
//!   after a [`purge`](VaultStore::purge), so a configuration that pins `#v1` never silently points
//!   at another secret.
//! - **[`SUPERSEDED_GRACE`].** The write that supersedes a version starts the clock: the version
//!   reads until the grace is over, at the instant of the write plus the grace (an instant at
//!   exactly the grace is over), and it is refused from then on. A version that is still the
//!   current one never expires. **A read after the grace fails closed**: whether the row is still
//!   there is the store's business, and the answer is [`VaultError::UnknownVersion`].
//! - **Writers of one name are serialized**, so two writes at once take two versions and neither
//!   loses the other. Which of them a reader finds under a *later* write is that later write.
//! - **An owner never reaches another's**: the same name is two secrets to two people.

use std::fmt;
use std::future::Future;
use std::str::FromStr;

use jiff::SignedDuration;
use orch_core::{BoxError, Classify, ErrorClass, UserId};

use crate::secrets::{SecretError, SecretMaterial, SecretVersion};

/// The most characters of the name of a secret: 80, as a tool server's name is
/// ([`MAX_NAME_CHARS`](crate::MAX_NAME_CHARS)).
pub const MAX_VAULT_NAME_CHARS: usize = 80;

/// How long a version stays readable after the write that superseded it: 24 hours (ADR 0083,
/// amended 2026-11-21, decision 2 — "so a run that resolved the old reference finishes on it").
pub const SUPERSEDED_GRACE: SignedDuration = SignedDuration::from_hours(24);

/// The prefix of every reference.
pub const VAULT_PREFIX: &str = "vault:";

/// What a name or a reference is not. The sender's mistake: the same text never becomes one, and no
/// message quotes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VaultRefError {
    /// A name that is empty, over [`MAX_VAULT_NAME_CHARS`] characters, has a control character, or
    /// holds a `#` (which the reference form needs for the version).
    #[error(
        "a secret name is 1 to {MAX_VAULT_NAME_CHARS} characters with no control character and no '#'"
    )]
    BadName,
    /// Not `vault:<name>#v<n>`, in that spelling, with `n` a positive number without leading
    /// zeros.
    #[error("not a vault reference: `vault:<name>#v<n>`")]
    BadReference,
}

impl Classify for VaultRefError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// A person's name for one of their secrets: 1 to [`MAX_VAULT_NAME_CHARS`] characters, with no
/// control character and no `#`, and the white space around it cut.
///
/// Not a secret, and not unique across people: a name is one person's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VaultName(String);

impl VaultName {
    /// Reads a name.
    ///
    /// # Errors
    /// [`VaultRefError::BadName`] for anything that is not one; the text is not quoted back.
    pub fn parse(text: &str) -> Result<Self, VaultRefError> {
        let name = text.trim();
        let ok = !name.is_empty()
            && name.chars().count() <= MAX_VAULT_NAME_CHARS
            && !name.contains('#')
            && !name.chars().any(char::is_control);
        if ok {
            Ok(VaultName(name.to_owned()))
        } else {
            Err(VaultRefError::BadName)
        }
    }

    /// The name, as it was written (`edit` in `edit: openai` stays as it is, without its white
    /// space).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VaultName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for VaultName {
    type Err = VaultRefError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// What a configuration names instead of a value: `vault:<name>#v<n>`.
///
/// The version is a [`SecretVersion`], the number the store gave the write (1 for a name's first).
/// A reference is comparable and printable, and holds no material: it is what an agent may see, a
/// message may carry and a log may keep.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VaultRef {
    name: VaultName,
    version: SecretVersion,
}

impl VaultRef {
    /// The reference of `name` at `version`.
    pub fn new(name: VaultName, version: SecretVersion) -> Self {
        VaultRef { name, version }
    }

    /// Reads `vault:<name>#v<n>`.
    ///
    /// # Errors
    /// [`VaultRefError::BadReference`] for another spelling (no prefix, no `#v`, a version that is
    /// not a positive number, leading zeros, trailing text), and [`VaultRefError::BadName`] for a
    /// name the reference form cannot carry.
    pub fn parse(text: &str) -> Result<Self, VaultRefError> {
        let rest = text
            .strip_prefix(VAULT_PREFIX)
            .ok_or(VaultRefError::BadReference)?;
        let (name, digits) = rest.rsplit_once("#v").ok_or(VaultRefError::BadReference)?;
        let number: u64 = digits.parse().map_err(|_| VaultRefError::BadReference)?;
        // the canonical spelling only: `#v0`, `#v01`, `#v+1` and `#v1 ` are not references this
        // store wrote, and one spelling per version is what lets a configuration be compared
        if number == 0 || number.to_string() != digits {
            return Err(VaultRefError::BadReference);
        }
        Ok(VaultRef {
            name: VaultName::parse(name)?,
            version: SecretVersion::new(number),
        })
    }

    /// The name.
    pub fn name(&self) -> &VaultName {
        &self.name
    }

    /// The version.
    pub fn version(&self) -> SecretVersion {
        self.version
    }
}

impl fmt::Display for VaultRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{VAULT_PREFIX}{}#v{}", self.name, self.version)
    }
}

impl FromStr for VaultRef {
    type Err = VaultRefError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// Why the vault did not give what it was asked for.
///
/// None holds material: a `detail` is plain text about what was attempted, and the lower error is
/// the `source`, which an adapter has checked for secrets before it boxes it.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VaultError {
    /// Nothing is kept under that name for this person.
    #[error("this person has no secret of that name")]
    UnknownName,
    /// The name is there, but not that version: it was never written, or it was superseded more
    /// than [`SUPERSEDED_GRACE`] ago. **The value is refused, not returned stale.**
    #[error("the vault holds no such version of that secret")]
    UnknownVersion,
    /// The store could not be reached or failed (a database that is down, a refused connection).
    #[error("the vault is unavailable: {detail}")]
    Unavailable {
        /// What was attempted, without paths, URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// A stored secret is sealed under a key this deployment does not hold (the key was removed
    /// before every row was re-sealed). That secret is unavailable; nothing else is.
    #[error("a secret is sealed under a key this deployment does not hold")]
    KeyUnknown,
    /// What the store holds is not a secret it wrote: an envelope that does not open, a row moved
    /// to another owner, name or version. Alert.
    #[error("the vault holds corrupt data: {0}")]
    Corrupt(String),
}

impl VaultError {
    /// The store failed or could not be reached.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        VaultError::Unavailable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The store holds something that is not a secret it wrote. `detail` says what, never the
    /// bytes.
    pub fn corrupt(detail: impl Into<String>) -> Self {
        VaultError::Corrupt(detail.into())
    }
}

impl Classify for VaultError {
    fn class(&self) -> ErrorClass {
        match self {
            VaultError::UnknownName | VaultError::UnknownVersion => ErrorClass::NotFound,
            VaultError::Unavailable { .. } => ErrorClass::Transient,
            VaultError::KeyUnknown | VaultError::Corrupt(_) => ErrorClass::Corrupt,
        }
    }
}

impl From<SecretError> for VaultError {
    /// The infrastructure side of a failure, as the store that shares this crate's database says
    /// it. [`SecretError::Conflict`] is a compare-and-swap's answer to a caller of
    /// [`SecretStore`](crate::SecretStore); the vault makes no such call, so one is a bug and is
    /// kept as corruption (which alerts) rather than hidden.
    fn from(error: SecretError) -> Self {
        match error {
            SecretError::Unavailable { detail, source } => {
                VaultError::Unavailable { detail, source }
            }
            SecretError::KeyUnknown => VaultError::KeyUnknown,
            SecretError::Corrupt(detail) => VaultError::Corrupt(detail),
            SecretError::Conflict => {
                VaultError::corrupt("a write of the vault was reported as a lost race")
            }
        }
    }
}

/// Where a person's named secrets are kept, sealed, by `(owner, name)`, with the versions a
/// rotation leaves behind still readable for [`SUPERSEDED_GRACE`].
///
/// See the module's docs for what an implementation owes. Every call is atomic on its own; the
/// writers of one name are serialized by the implementation.
pub trait VaultStore: Send + Sync + 'static {
    /// Writes `material` as the next version of `name` under `owner`, and returns the reference a
    /// configuration names: `#v1` for a name never written, else one more than the greatest the
    /// name has had.
    ///
    /// The version that was current until now becomes **superseded**: it reads for
    /// [`SUPERSEDED_GRACE`] from this write and is refused after it.
    ///
    /// # Errors
    /// [`VaultError`]; its class says whether asking again can help.
    fn put(
        &self,
        owner: &UserId,
        name: &VaultName,
        material: &SecretMaterial,
    ) -> impl Future<Output = Result<VaultRef, VaultError>> + Send;

    /// The material `reference` names.
    ///
    /// # Errors
    /// [`VaultError::UnknownName`] when the person has no secret of that name,
    /// [`VaultError::UnknownVersion`] when the name is there and that version is not (never
    /// written, or past its [`SUPERSEDED_GRACE`]), [`VaultError::KeyUnknown`] and
    /// [`VaultError::Corrupt`] for a row that cannot be opened, [`VaultError::Unavailable`] when
    /// the store fails.
    fn get(
        &self,
        owner: &UserId,
        reference: &VaultRef,
    ) -> impl Future<Output = Result<SecretMaterial, VaultError>> + Send;

    /// Removes every version whose [`SUPERSEDED_GRACE`] is over, and nothing else: never the
    /// current version of a name. Returns how many it removed; a store with nothing to remove says
    /// `0`. Running it twice is not an error.
    ///
    /// # Errors
    /// [`VaultError::Unavailable`] when the store fails.
    fn purge(&self) -> impl Future<Output = Result<u64, VaultError>> + Send;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_the_name_and_the_version_in_one_spelling() {
        let reference = VaultRef::parse("vault:my-search#v3").unwrap();
        assert_eq!(reference.name().as_str(), "my-search");
        assert_eq!(reference.version(), SecretVersion::new(3));
        assert_eq!(reference.to_string(), "vault:my-search#v3");
        assert_eq!(reference, "vault:my-search#v3".parse::<VaultRef>().unwrap());
        // a name with a space and a colon is a name
        let spaced = VaultRef::parse("vault:openai key (prod): 2#v41").unwrap();
        assert_eq!(spaced.name().as_str(), "openai key (prod): 2");
        assert_eq!(spaced.version().get(), 41);
        assert_eq!(spaced.to_string(), "vault:openai key (prod): 2#v41");
        // and the version the reference names is the whole tail, not the first `#v`
        let awkward = VaultRef::parse("vault:team#vault#v7").unwrap_err();
        assert_eq!(awkward, VaultRefError::BadName, "a name cannot hold a #");
    }

    #[test]
    fn only_the_canonical_spelling_reads() {
        for bad in [
            "",
            "my-search#v3",
            "vault:my-search",
            "vault:my-search#",
            "vault:my-search#v",
            "vault:my-search#v0",
            "vault:my-search#v01",
            "vault:my-search#v1 ",
            "vault:my-search#v18446744073709551616",
            "vault:my-search#v-1",
            "VAULT:my-search#v1",
            "vault:my-search#V1",
        ] {
            assert_eq!(
                VaultRef::parse(bad).unwrap_err(),
                VaultRefError::BadReference,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_name_is_trimmed_bounded_and_has_no_control_character_or_hash() {
        assert_eq!(
            VaultName::parse("  prod key  ").unwrap().as_str(),
            "prod key"
        );
        assert_eq!(VaultName::parse("tökén ✓").unwrap().as_str(), "tökén ✓");
        let longest = "x".repeat(MAX_VAULT_NAME_CHARS);
        assert!(VaultName::parse(&longest).is_ok());
        for bad in ["", "   ", "a#b", "a\nb", "a\tb", "a\0b"] {
            assert_eq!(
                VaultName::parse(bad).unwrap_err(),
                VaultRefError::BadName,
                "{bad:?}"
            );
        }
        assert_eq!(
            VaultName::parse(&format!("{longest}x")).unwrap_err(),
            VaultRefError::BadName
        );
    }

    #[test]
    fn a_missing_reference_is_not_found_and_a_store_that_fails_is_transient() {
        assert_eq!(VaultError::UnknownName.class(), ErrorClass::NotFound);
        assert_eq!(VaultError::UnknownVersion.class(), ErrorClass::NotFound);
        assert_eq!(
            VaultError::unavailable("down").class(),
            ErrorClass::Transient
        );
        assert_eq!(VaultError::corrupt("a row").class(), ErrorClass::Corrupt);
        assert_eq!(
            VaultError::from(SecretError::Conflict).class(),
            ErrorClass::Corrupt
        );
        assert_eq!(VaultRefError::BadName.class(), ErrorClass::Invalid);
    }

    #[test]
    fn the_grace_is_a_day() {
        assert_eq!(SUPERSEDED_GRACE.as_secs(), 86_400);
    }
}
