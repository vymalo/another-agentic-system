use std::future::Future;
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, AgentTaskState, Event, EventBody, ThreadId, ThreadRecord,
    ThreadState, UserId,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identifier of an outbox row. It doubles as the A2A `messageId` of the delegated message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OutboxId(pub Uuid);

impl std::fmt::Display for OutboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A thread to create together with its first [`Commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThreadRecord {
    /// Thread id (UUIDv7, so ids sort by creation time).
    pub id: ThreadId,
    /// Owner.
    pub owner: UserId,
    /// Title.
    pub title: String,
    /// Target agent and release.
    pub target: AgentTarget,
    /// The A2A context id every task of this thread shares.
    pub context_id: String,
    /// Creation time.
    pub now: Timestamp,
}

/// An event to append. `seq` is assigned by the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvent {
    /// When it happened (stamped by the application's clock).
    pub at: Timestamp,
    /// Who produced it.
    pub actor: Actor,
    /// Payload.
    pub body: EventBody,
    /// A replayed input carries the same key, so it never produces a second event.
    pub idempotency_key: Option<String>,
}

/// What an outbox row asks the dispatcher to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxKind {
    /// Send a message to the agent.
    Delegate,
    /// Cancel the running task.
    Cancel,
}

/// Payload of an outbox row (stored as JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxPayload {
    /// Delegate `text` (with the release selected at thread creation).
    Delegate {
        /// User text.
        text: String,
        /// Selected release channel or revision.
        release: Option<String>,
    },
    /// Cancel.
    Cancel,
}

impl OutboxPayload {
    /// The kind matching this payload.
    pub fn kind(&self) -> OutboxKind {
        match self {
            OutboxPayload::Delegate { .. } => OutboxKind::Delegate,
            OutboxPayload::Cancel => OutboxKind::Cancel,
        }
    }
}

/// A new outbox row, written in the same transaction as the state change that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutbox {
    /// Row id.
    pub id: OutboxId,
    /// What to do.
    pub payload: OutboxPayload,
}

/// Partial update of a thread's agent binding; `None` fields are left as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingUpdate {
    /// The A2A task id.
    pub task_id: Option<String>,
    /// Last known task state.
    pub task_state: Option<AgentTaskState>,
    /// Agent revision that served the task.
    pub revision: Option<String>,
}

/// Everything that changes in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The thread state after the commit.
    pub new_state: ThreadState,
    /// Events to append (seq assigned by the store, contiguous).
    pub events: Vec<NewEvent>,
    /// Outbox rows to insert as `pending`, due immediately.
    pub outbox: Vec<NewOutbox>,
    /// Binding fields to update.
    pub binding: Option<BindingUpdate>,
    /// Time of the commit (`updated_at`, outbox timestamps).
    pub now: Timestamp,
}

/// Result of [`ThreadStore::commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// Written.
    Applied {
        /// The thread after the commit.
        thread: ThreadRecord,
        /// The events as stored, with their `seq`.
        events: Vec<Event>,
    },
    /// An idempotency key was already present: nothing was written.
    Duplicate,
}

/// The A2A side of a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinding {
    /// Thread.
    pub thread_id: ThreadId,
    /// Agent.
    pub agent_id: AgentId,
    /// A2A context id.
    pub context_id: String,
    /// Current/last task id.
    pub task_id: Option<String>,
    /// Last known task state.
    pub task_state: Option<AgentTaskState>,
    /// Revision that served the task.
    pub revision: Option<String>,
}

/// Lifecycle of an outbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    /// Waiting to be claimed (possibly after a backoff).
    Pending,
    /// Claimed by a dispatcher under a lease.
    Inflight,
    /// Done.
    Delivered,
    /// Gave up.
    Dead,
    /// Obsolete (thread finished or cancelled before it ran).
    Skipped,
}

impl OutboxStatus {
    /// `pending` or `inflight`.
    pub fn is_open(self) -> bool {
        match self {
            OutboxStatus::Pending | OutboxStatus::Inflight => true,
            OutboxStatus::Delivered | OutboxStatus::Dead | OutboxStatus::Skipped => false,
        }
    }
}

/// An outbox row as seen by the dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    /// Row id (the A2A `messageId` for delegations).
    pub id: OutboxId,
    /// Thread.
    pub thread_id: ThreadId,
    /// Kind.
    pub kind: OutboxKind,
    /// Payload.
    pub payload: OutboxPayload,
    /// Status.
    pub status: OutboxStatus,
    /// How many times it has been claimed (including the current claim).
    pub attempts: u32,
    /// Set once the A2A message reached the agent and a task id is known.
    pub sent_at: Option<Timestamp>,
    /// Earliest next claim.
    pub next_attempt_at: Timestamp,
    /// Current lease holder.
    pub lease_owner: Option<String>,
    /// Lease expiry.
    pub lease_until: Option<Timestamp>,
    /// Last error text.
    pub last_error: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Final disposition of an outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxFinal {
    /// Handled.
    Delivered,
    /// Gave up; carries the last error.
    Dead {
        /// Error text.
        error: String,
    },
    /// Obsolete.
    Skipped,
}

/// Store failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The thread does not exist.
    #[error("not found")]
    NotFound,
    /// The thread changed since it was read; re-read and retry.
    #[error("version conflict")]
    VersionConflict,
    /// The backing store could not be reached.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// Stored data could not be decoded.
    #[error("corrupt data: {0}")]
    Corrupt(String),
}

impl StoreError {
    /// Whether the same operation may succeed later.
    pub fn is_retryable(&self) -> bool {
        match self {
            StoreError::Unavailable(_) | StoreError::VersionConflict => true,
            StoreError::NotFound | StoreError::Corrupt(_) => false,
        }
    }
}

/// Threads, their append-only event logs, the agent binding and the outbox.
///
/// Every mutating method is atomic. Outbox methods that take an `owner` only act on a row
/// that is `inflight` and leased to that owner, and return `false` otherwise (a lost lease).
pub trait ThreadStore: Send + Sync + 'static {
    /// Cheap reachability check (readiness).
    fn ping(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Atomically inserts the thread (state and events from `first`, version 1), its binding
    /// row (agent from the target, context from `new`, then `first.binding`), the events
    /// (seq 1..) and the outbox rows.
    fn create_thread(
        &self,
        new: NewThreadRecord,
        first: Commit,
    ) -> impl Future<Output = Result<(ThreadRecord, Vec<Event>), StoreError>> + Send;

    /// `owner = None` is system access (dispatcher). An owner mismatch yields `Ok(None)`.
    fn get_thread(
        &self,
        owner: Option<&UserId>,
        id: ThreadId,
    ) -> impl Future<Output = Result<Option<ThreadRecord>, StoreError>> + Send;

    /// The owner's threads, newest first (id descending; ids are UUIDv7). `before` is an
    /// exclusive cursor; an unknown or foreign cursor yields an empty list.
    fn list_threads(
        &self,
        owner: &UserId,
        before: Option<ThreadId>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ThreadRecord>, StoreError>> + Send;

    /// One transaction. Locks the thread; `version != expected_version` gives
    /// [`StoreError::VersionConflict`] (nothing written); an idempotency key already present
    /// in the thread's log gives [`CommitOutcome::Duplicate`] (nothing written); otherwise
    /// appends the events with `seq = last_seq + 1..`, sets state, bumps version, `last_seq`
    /// and `updated_at`, inserts the outbox rows and applies the binding update.
    fn commit(
        &self,
        thread: ThreadId,
        expected_version: i64,
        commit: Commit,
    ) -> impl Future<Output = Result<CommitOutcome, StoreError>> + Send;

    /// Events with `seq > after`, oldest first.
    fn list_events(
        &self,
        thread: ThreadId,
        after: i64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;

    /// The thread's A2A binding.
    fn get_binding(
        &self,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Option<AgentBinding>, StoreError>> + Send;

    /// Claims up to `limit` rows (oldest first): `pending` and due, or `inflight` with an
    /// expired lease (`lease_until <= now`). A delegate row is claimable only if no older
    /// `pending`/`inflight` delegate row exists for the same thread (per-thread ordering,
    /// including rows claimed earlier in the same call); cancel rows are unrestricted.
    /// Sets `inflight`, owner, `lease_until = now + lease`, `attempts += 1`. Concurrent
    /// claimers never get the same row.
    fn claim_outbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<OutboxItem>, StoreError>> + Send;

    /// Extends the lease. `false` if the row is no longer leased to `owner`.
    fn renew_lease(
        &self,
        id: OutboxId,
        owner: &str,
        until: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Sets `sent_at` (the message reached the agent and a task id is known) and applies the
    /// binding update, in one transaction.
    fn mark_sent(
        &self,
        id: OutboxId,
        owner: &str,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Puts the row back to `pending`, due at `next_attempt_at`, and records the error.
    fn retry_outbox(
        &self,
        id: OutboxId,
        owner: &str,
        next_attempt_at: Timestamp,
        error: String,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Finishes the row.
    fn complete_outbox(
        &self,
        id: OutboxId,
        owner: &str,
        outcome: OutboxFinal,
        now: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Marks the thread's not-yet-sent delegate rows (`sent_at` is null) as `skipped`: those
    /// `pending`, and those `inflight` whose lease expired before `now`. Returns the count.
    fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> impl Future<Output = Result<u32, StoreError>> + Send;

    /// On shutdown: sets `lease_until = now` for rows leased to `owner` so another replica
    /// claims them immediately. Returns the count.
    fn release_leases(
        &self,
        owner: &str,
        now: Timestamp,
    ) -> impl Future<Output = Result<u32, StoreError>> + Send;

    /// Inspection: one outbox row, whatever its status.
    fn get_outbox(
        &self,
        id: OutboxId,
    ) -> impl Future<Output = Result<Option<OutboxItem>, StoreError>> + Send;

    /// Inspection: the thread's `pending`/`inflight` rows, oldest first.
    fn list_open_outbox(
        &self,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Vec<OutboxItem>, StoreError>> + Send;
}
