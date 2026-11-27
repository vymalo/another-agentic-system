use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A thread identifier (UUID, wire format `format: uuid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub Uuid);

impl ThreadId {
    /// The UUID version of the ids the MCP surface derives from a user and a `client_request_id`
    /// (ADR 0019). No other surface creates a thread with such an id, so a retried `start_job`
    /// can find its job and nobody can take the id first.
    pub const DERIVED_VERSION: usize = 8;

    /// Whether the id is in the namespace reserved for derived ids. The AG-UI run route, where
    /// the consumer chooses the id, refuses to create a thread with one.
    pub fn is_derived(&self) -> bool {
        self.0.get_version_num() == Self::DERIVED_VERSION
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for ThreadId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(ThreadId)
    }
}

/// An authenticated user: the trimmed, lower-cased e-mail from the proxy.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    /// Normalises (trim + lower-case) an e-mail into a user id.
    pub fn new(email: &str) -> Self {
        UserId(email.trim().to_lowercase())
    }

    /// The normalised e-mail.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest [`AccountId`], in bytes: the platform's own bound for an identifier it meters.
pub const MAX_ACCOUNT_ID_LEN: usize = 128;

/// The account a run's spending is charged to (ADR 0106): the person's identity-provider `sub`,
/// and only a value of the platform's closed alphabet.
///
/// The alphabet is `[A-Za-z0-9._:/-]`, 1 to 128 bytes: what the platform's usage contract
/// (`aap-usage`, `PodMinuteEvent::validate`) accepts for an identifier, so that free text and
/// personal data cannot pass. **An e-mail address has an `@` and is therefore never one**, and the
/// only way to build an `AccountId` is [`AccountId::parse`], so a value that left the broker's
/// routes was checked by construction, not by a caller's care.
///
/// `Debug` shows `<redacted>`: like a [`UserId`], an account names a person and is in no log line.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(String);

/// Not an [`AccountId`]: 1 to 128 bytes of letters, digits, `.`, `_`, `:`, `/` and `-`. It holds
/// no part of what it refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an account id: 1 to 128 bytes of letters, digits, '.', '_', ':', '/' and '-'")]
pub struct AccountIdError;

impl AccountId {
    /// Reads `text` as an account id, exactly as written: no trimming, no case change.
    ///
    /// # Errors
    /// [`AccountIdError`] for an empty text, one over 128 bytes, or one with a byte outside
    /// `[A-Za-z0-9._:/-]` (an e-mail address, a space, a `|`, a non-ASCII letter).
    pub fn parse(text: &str) -> Result<Self, AccountIdError> {
        let in_alphabet =
            |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-');
        if text.is_empty() || text.len() > MAX_ACCOUNT_ID_LEN || !text.bytes().all(in_alphabet) {
            return Err(AccountIdError);
        }
        Ok(AccountId(text.to_owned()))
    }

    /// The account of a person: their [`UserId`] when it is an account id, which it is when the
    /// deployment reads the identity provider's `sub` (`auth.jwt.userClaim: sub`).
    ///
    /// # Errors
    /// [`AccountIdError`] when the person's id is not one, as an e-mail address is not.
    pub fn of_user(user: &UserId) -> Result<Self, AccountIdError> {
        Self::parse(user.as_str())
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountId(<redacted>)")
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The configuration key of an agent, e.g. `coder`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

impl AgentId {
    /// Builds an agent id from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        AgentId(id.into())
    }

    /// The raw id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest agent id, in bytes.
pub const MAX_AGENT_ID_LEN: usize = 63;

/// Whether `id` is an agent id: `^[a-z0-9][a-z0-9-]{0,62}$` (lower-case letters, digits and
/// dashes, starting with a letter or a digit). Such an id is safe in a URL, a file name and a
/// label, which is why every source of agents, the `AGENTS_FILE` and a registry document alike,
/// is held to it.
pub fn is_valid_agent_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && id.len() <= MAX_AGENT_ID_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_id_is_the_platforms_closed_alphabet() {
        let longest = "a".repeat(128);
        for good in [
            "a",
            "4f2a9c1e-0b7d-4c55-9e1a-6d3f8a2b7c10",
            "Abc.def_ghi:jkl/mno-pqr",
            "auth0:5f7c",
            longest.as_str(),
        ] {
            assert_eq!(AccountId::parse(good).unwrap().as_str(), good, "{good:?}");
        }
        let too_long = "a".repeat(129);
        for bad in [
            "",
            "person@example.com",
            "a@b",
            "auth0|5f7c",
            "has space",
            " lead",
            "trail\n",
            "caf\u{e9}",
            "a;b",
            "a+b",
            too_long.as_str(),
        ] {
            assert_eq!(AccountId::parse(bad), Err(AccountIdError), "{bad:?}");
        }
    }

    #[test]
    fn an_e_mail_person_has_no_account_and_a_sub_person_has_its_own_text() {
        assert!(AccountId::of_user(&UserId::new("Person@Example.com")).is_err());
        let sub = UserId::new("  4F2A9C1E-0B7D  ");
        assert_eq!(AccountId::of_user(&sub).unwrap().as_str(), "4f2a9c1e-0b7d");
    }

    #[test]
    fn an_account_id_is_redacted_in_debug_and_refusals_quote_nothing() {
        let account = AccountId::parse("sub-123").unwrap();
        assert!(!format!("{account:?}").contains("sub-123"));
        assert!(!AccountIdError.to_string().contains('@'));
    }

    #[test]
    fn agent_ids_are_safe_slugs() {
        for good in ["a", "0", "coder", "coder-2", "a-", &"a".repeat(63)] {
            assert!(is_valid_agent_id(good), "{good:?} is an id");
        }
        for bad in [
            "",
            "Coder",
            "-x",
            "has space",
            "a/b",
            "a_b",
            "a.b",
            "é",
            "a\n",
            &"a".repeat(64),
        ] {
            assert!(!is_valid_agent_id(bad), "{bad:?} is not an id");
        }
    }
}
