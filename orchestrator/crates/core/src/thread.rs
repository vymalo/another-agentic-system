use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ThreadId, UserId};

/// MVP subset of the job lifecycle (contract `ThreadState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    /// Accepted, not yet picked up by the agent.
    Queued,
    /// The agent is working.
    Working,
    /// Waiting for the user (or for a retry after a delivery failure).
    Blocked,
    /// Finished successfully.
    Done,
    /// Finished unsuccessfully.
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl ThreadState {
    /// `done`, `failed` and `cancelled` are absorbing.
    pub fn is_terminal(self) -> bool {
        match self {
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => true,
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => false,
        }
    }

    /// The wire / database spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadState::Queued => "queued",
            ThreadState::Working => "working",
            ThreadState::Blocked => "blocked",
            ThreadState::Done => "done",
            ThreadState::Failed => "failed",
            ThreadState::Cancelled => "cancelled",
        }
    }
}

/// Contract `AgentTarget`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTarget {
    /// Configuration key of the agent.
    pub agent_id: AgentId,
    /// Channel or exact revision; only valid when the agent offers releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}

/// Contract `Agent.releases` (release-channels extension, ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Releases {
    /// Channel used when the user picks none.
    pub default_channel: String,
    /// Channel name to revision.
    pub channels: BTreeMap<String, String>,
    /// Exact revisions that can be selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<String>>,
}

impl Releases {
    /// Whether `selector` names a channel or a revision.
    pub fn accepts(&self, selector: &str) -> bool {
        self.channels.contains_key(selector)
            || self
                .revisions
                .as_ref()
                .is_some_and(|r| r.iter().any(|x| x == selector))
    }

    /// The revision a selector resolves to, when it is a channel or a known revision.
    pub fn resolve(&self, selector: &str) -> Option<String> {
        if let Some(rev) = self.channels.get(selector) {
            return Some(rev.clone());
        }
        self.revisions
            .as_ref()
            .and_then(|r| r.iter().find(|x| x.as_str() == selector).cloned())
    }
}

/// A thread as stored. Serialises to the contract `Thread` (owner and version stay internal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRecord {
    /// Thread id.
    pub id: ThreadId,
    /// Owner (never serialised).
    #[serde(skip)]
    pub owner: UserId,
    /// Title.
    pub title: String,
    /// Target agent and release.
    pub target: AgentTarget,
    /// Current state.
    pub state: ThreadState,
    /// Optimistic-concurrency version (never serialised).
    #[serde(skip)]
    pub version: i64,
    /// Sequence number of the last event, 0 when empty.
    pub last_seq: i64,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change time.
    pub updated_at: Timestamp,
}

/// Contract `Agent`: a configured agent, with its live release data when it offers any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    /// Configuration key, e.g. `coder`.
    pub id: AgentId,
    /// Display name.
    pub name: String,
    /// Description from the live card, when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// URL of the agent card.
    pub card_url: String,
    /// Present only when the live card advertises the release-channels extension (ADR 0008).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub releases: Option<Releases>,
}
