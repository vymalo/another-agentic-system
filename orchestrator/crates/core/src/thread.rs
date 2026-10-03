use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::fork::ForkedFrom;
use crate::gate::{Job, JobView, Snapshot};
use crate::ids::{AgentId, ThreadId, UserId};
use crate::share::{ThreadShare, Visibility};

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

fn is_false(value: &bool) -> bool {
    !*value
}

/// A thread as stored. Serialises to the contract `Thread` (the version stays internal). The
/// owner's organisation of their list (`pinned`, `archived`, `nestedUnder`) is written only when
/// it is set; the readers of a shared thread are served a projection that has none of it
/// (ADR 0042).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadRecord {
    /// Thread id.
    pub id: ThreadId,
    /// Owner: the e-mail of the person the thread belongs to. Serialised, so that a person who may
    /// read other people's threads (an administrator, ADR 0033) can tell theirs from the rest.
    pub owner: UserId,
    /// Title.
    pub title: String,
    /// What the thread is about now, in a sentence or two (ADR 0035); absent until the model or
    /// a person writes one, and again when a person clears it.
    pub description: Option<String>,
    /// Target agent and release.
    pub target: AgentTarget,
    /// Current state.
    pub state: ThreadState,
    /// The job ledger. Serialised as the contract `Thread.job`, the part of it clients see, and
    /// only under an active gate; the servers attached to the thread (`job.tools`) are the contract
    /// `Thread.tools`, whatever the gate.
    pub job: Job,
    /// Optimistic-concurrency version (never serialised).
    pub version: i64,
    /// Where the thread was forked from (ADR 0029); `None` for a thread that was not.
    pub forked_from: Option<ForkedFrom>,
    /// The thread's share (ADR 0040): who may read it besides its owner, and the nonce the link
    /// is built on; `None` for a thread that is private. Never serialised with the thread: the
    /// link is the owner's alone, and the application says it (`share` of `GET /api/threads/{id}`).
    pub share: Option<ThreadShare>,
    /// When the owner pinned the thread (ADR 0042); `None` while it is not. Kept on the row and
    /// never in the log, so that no fork, export or reader of a shared thread sees it.
    pub pinned_at: Option<Timestamp>,
    /// When the owner archived the thread; `None` while it is not.
    pub archived_at: Option<Timestamp>,
    /// The thread this one is nested under in the owner's list, one level deep; `None` for a row of
    /// the list's own. Display grouping only: the lineage is [`ThreadRecord::forked_from`].
    pub rail_parent: Option<ThreadId>,
    /// The thread's place among the owner's top-level threads: a key of [`crate::rank`], compared
    /// bytewise, ties broken by id (newest first). Never serialised.
    pub rail_rank: String,
    /// Sequence number of the last event, 0 when empty.
    pub last_seq: i64,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last change time.
    pub updated_at: Timestamp,
}

impl Serialize for ThreadRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            id: ThreadId,
            owner: &'a UserId,
            title: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: Option<&'a str>,
            target: &'a AgentTarget,
            state: ThreadState,
            #[serde(skip_serializing_if = "Option::is_none")]
            job: Option<JobView>,
            #[serde(skip_serializing_if = "<[String]>::is_empty")]
            tools: &'a [String],
            #[serde(skip_serializing_if = "Option::is_none")]
            forked_from: Option<&'a ForkedFrom>,
            #[serde(skip_serializing_if = "is_false")]
            pinned: bool,
            #[serde(skip_serializing_if = "is_false")]
            archived: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            nested_under: Option<ThreadId>,
            last_seq: i64,
            created_at: Timestamp,
            updated_at: Timestamp,
        }
        Wire {
            id: self.id,
            owner: &self.owner,
            title: &self.title,
            description: self.description.as_deref(),
            target: &self.target,
            state: self.state,
            job: self.job.view(),
            tools: &self.job.tools,
            forked_from: self.forked_from.as_ref(),
            pinned: self.pinned_at.is_some(),
            archived: self.archived_at.is_some(),
            nested_under: self.rail_parent,
            last_seq: self.last_seq,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
        .serialize(serializer)
    }
}

impl ThreadRecord {
    /// Who may read the thread besides its owner, before any cap the deployment puts on it.
    pub fn visibility(&self) -> Visibility {
        self.share
            .as_ref()
            .map_or(Visibility::Private, |s| s.level.visibility())
    }

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
