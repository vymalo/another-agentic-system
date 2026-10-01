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
