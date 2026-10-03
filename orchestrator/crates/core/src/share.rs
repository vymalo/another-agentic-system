//! Sharing a thread by a revocable link (ADR 0040): who may read it besides its owner, and the
//! two events that say so.
//!
//! A thread has a [`Visibility`], `private` (every thread starts so, a fork too), `internal` or
//! `public`. What is shared is a [`ShareLevel`], the two that are not private: a share is a level
//! and a nonce, the nonce being the capability the link is built on. The log keeps the audit trail
//! (`thread_shared` with the digest of the nonce, `thread_unshared`) and **never the nonce**: the
//! export carries the log, and the log must not hold a working capability. The thread's row keeps
//! the nonce ([`ThreadShare`]), applied in the same transaction as the event by
//! [`Command::SetSharing`](crate::Command::SetSharing) and
//! [`Command::ClearSharing`](crate::Command::ClearSharing).
//!
//! This module is pure: the nonce is drawn by the application, through a port, and handed to the
//! core in [`Input::Share`](crate::Input::Share). The cap a deployment puts on sharing, and the
//! link's MAC, are the application's.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bytes of a share's nonce.
pub const NONCE_LEN: usize = 16;

/// Who may read a thread besides its owner. Ordered by how wide: `Private < Internal < Public`.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Nobody: the thread is its owner's.
    #[default]
    Private,
    /// A person who is signed in and has the link.
    Internal,
    /// Anybody who has the link, signed in or not.
    Public,
}

impl Visibility {
    /// The wire / database spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Visibility::Private => "private",
            Visibility::Internal => "internal",
            Visibility::Public => "public",
        }
    }

    /// The visibility named `name`, exactly.
    pub fn parse(name: &str) -> Option<Visibility> {
        [
            Visibility::Private,
            Visibility::Internal,
            Visibility::Public,
        ]
        .into_iter()
        .find(|v| v.as_str() == name)
    }

    /// What is served when a deployment allows at most `cap`: the narrower of the two. A cap
    /// that is `disabled` is [`Visibility::Private`].
    #[must_use]
    pub fn capped_by(self, cap: Visibility) -> Visibility {
        self.min(cap)
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a share lets a reader do: the two visibilities that are not private. Closed (ADR 0004):
/// there is no share that is private, a revocation is [`Input::Unshare`](crate::Input::Unshare).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareLevel {
    /// Signed-in people with the link.
    Internal,
    /// Anybody with the link.
    Public,
}

impl ShareLevel {
    /// The visibility a thread has when it is shared at this level.
    pub const fn visibility(self) -> Visibility {
        match self {
            ShareLevel::Internal => Visibility::Internal,
            ShareLevel::Public => Visibility::Public,
        }
    }

    /// The wire / database spelling.
    pub const fn as_str(self) -> &'static str {
        self.visibility().as_str()
    }

    /// The level named `name`, exactly (`internal` or `public`).
    pub fn parse(name: &str) -> Option<ShareLevel> {
        match name {
            "internal" => Some(ShareLevel::Internal),
            "public" => Some(ShareLevel::Public),
            _ => None,
        }
    }
}

impl fmt::Display for ShareLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<ShareLevel> for Visibility {
    fn from(level: ShareLevel) -> Self {
        level.visibility()
    }
}

/// The 16 random bytes a link is built on. A capability: its `Debug` says nothing of the bytes, so
/// an input or a command that holds one can be logged.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShareNonce([u8; NONCE_LEN]);

impl ShareNonce {
    /// A nonce of these bytes.
    pub const fn new(bytes: [u8; NONCE_LEN]) -> Self {
        ShareNonce(bytes)
    }

    /// The bytes.
    pub const fn as_bytes(&self) -> &[u8; NONCE_LEN] {
        &self.0
    }

    /// The lower-case hex of the SHA-256 of the nonce: what the log records of it. It tells one
    /// link from another (a rotation changes it) and is useless as a link.
    pub fn sha256_hex(&self) -> String {
        let digest = Sha256::digest(self.0);
        let mut hex = String::with_capacity(64);
        for byte in digest {
            hex.push_str(&format!("{byte:02x}"));
        }
        hex
    }
}

impl fmt::Debug for ShareNonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ShareNonce(..)")
    }
}

/// A thread's share, as its row holds it: the level, the nonce the link is built on, and when it
/// was made (the last `thread_shared`, a rotation or a change of level included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadShare {
    /// Who may read.
    pub level: ShareLevel,
    /// The capability.
    pub nonce: ShareNonce,
    /// When the share was last set.
    pub shared_at: Timestamp,
}

/// `data` of a `thread_shared` (ADR 0040): the thread is shared at `visibility`, with the link
/// whose nonce has this digest. A share, a widening, a narrowing and a new link are each one.
/// Attributed to the owner. The nonce is not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSharedData {
    /// Who may read from now on.
    pub visibility: ShareLevel,
    /// The lower-case hex SHA-256 of the nonce in use.
    pub nonce_sha256: String,
}

/// `data` of a `thread_unshared` (ADR 0040): the owner took the link down. Nothing else is said.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadUnsharedData {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_is_ordered_by_how_wide_it_is() {
        assert!(Visibility::Private < Visibility::Internal);
        assert!(Visibility::Internal < Visibility::Public);
        assert_eq!(
            Visibility::Public.capped_by(Visibility::Internal),
            Visibility::Internal
        );
        assert_eq!(
            Visibility::Internal.capped_by(Visibility::Public),
            Visibility::Internal
        );
        assert_eq!(
            Visibility::Public.capped_by(Visibility::Private),
            Visibility::Private
        );
        assert_eq!(Visibility::default(), Visibility::Private);
    }

    #[test]
    fn names_round_trip_and_nothing_else_parses() {
        for v in [
            Visibility::Private,
            Visibility::Internal,
            Visibility::Public,
        ] {
            assert_eq!(Visibility::parse(v.as_str()), Some(v));
        }
        for l in [ShareLevel::Internal, ShareLevel::Public] {
            assert_eq!(ShareLevel::parse(l.as_str()), Some(l));
            assert_eq!(Visibility::from(l).as_str(), l.as_str());
        }
        for bad in ["", "Private", "private ", "all", "world"] {
            assert_eq!(Visibility::parse(bad), None, "{bad:?}");
        }
        assert_eq!(ShareLevel::parse("private"), None);
    }

    #[test]
    fn a_nonce_says_only_its_digest_and_never_prints() {
        let nonce = ShareNonce::new([7; NONCE_LEN]);
        assert_eq!(format!("{nonce:?}"), "ShareNonce(..)");
        let digest = nonce.sha256_hex();
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(digest, ShareNonce::new([7; NONCE_LEN]).sha256_hex());
        assert_ne!(digest, ShareNonce::new([8; NONCE_LEN]).sha256_hex());
    }
}
