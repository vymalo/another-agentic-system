use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::fork::ForkedFrom;
use crate::gate::{Job, Snapshot};
use crate::ids::{AgentId, ThreadId, UserId};

/// MVP subset of the job lifecycle (contract `ThreadState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadState {
    /// Accepted, not yet picked up by the agent.
    Queued,
    /// The agent is working.
    Working,
    /// The agent finished and the verification gate is checking its work (ADR 0018). Only a
    /// thread under an active gate is ever here; it is implied by the `check_result` events,
    /// never announced by a `thread_state` event.
    Verifying,
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
    /// `done`, `failed` and `cancelled`: the current job is over. The thread is not: a user message
    /// starts the next job (ADR 0020); every other input is refused or dropped.
    pub fn is_terminal(self) -> bool {
        match self {
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => true,
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Verifying
            | ThreadState::Blocked => false,
        }
    }

    /// The wire / database spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadState::Queued => "queued",
            ThreadState::Working => "working",
            ThreadState::Verifying => "verifying",
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
    /// What the thread is about now, in a sentence or two (ADR 0035); absent until the model or
    /// a person writes one, and again when a person clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Target agent and release.
    pub target: AgentTarget,
    /// Current state.
    pub state: ThreadState,
    /// The job ledger. Serialised as the contract `Thread.job`, the part of it clients see, and
    /// only under an active gate.
    #[serde(
        serialize_with = "serialize_job",
        skip_serializing_if = "job_is_hidden",
        rename = "job"
    )]
    pub job: Job,
    /// Optimistic-concurrency version (never serialised).
    #[serde(skip)]
    pub version: i64,
    /// Where the thread was forked from (ADR 0029); `None` for a thread that was not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkedFrom>,
    /// Sequence number of the last event, 0 when empty.
    pub last_seq: i64,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change time.
    pub updated_at: Timestamp,
}

fn job_is_hidden(job: &Job) -> bool {
    !job.gate.is_active()
}

fn serialize_job<S: serde::Serializer>(job: &Job, serializer: S) -> Result<S::Ok, S::Error> {
    job.view().serialize(serializer)
}

impl ThreadRecord {
    /// The state and job [`transition`](crate::transition) works on.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state,
            job: self.job.clone(),
        }
    }
}

/// Where an agent is listed from (contract `Agent.source`; ADR 0022).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// The deployment's own list (`AGENTS_FILE`), fixed for the life of the process.
    #[default]
    Static,
    /// A registry the orchestrator reads live (the platform's `agent-registry/v1`).
    Registry,
}

/// Contract `Agent`: a listed agent, with its live release data when it offers any.
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
    /// URL of the agent card. Absent for an agent hosted in the orchestrator's own process,
    /// which has no card URL (ADR 0015).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_url: Option<String>,
    /// Present only when the live card advertises the release-channels extension (ADR 0008).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub releases: Option<Releases>,
    /// Where the agent is listed from.
    #[serde(default)]
    pub source: AgentSource,
    /// Labels the registry keeps on the agent, for the UI to show and to filter by; they route,
    /// authorise and select nothing. Omitted when there are none (a static agent has none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}
