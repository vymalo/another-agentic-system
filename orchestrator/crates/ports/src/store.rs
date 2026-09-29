use std::future::Future;
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, AgentTaskState, BoxError, Classify, ErrorClass, Event, EventBody,
    ThreadId, ThreadRecord, ThreadState, UserId,
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
    /// Delegate the user's action on an A2UI surface (ADR 0013), with the time it happened. It is
    /// a `delegate` row like a message: the same claim, resume and retry rules apply.
    Action {
        /// The action.
        action: orch_core::UiActionData,
        /// When the user acted.
        at: Timestamp,
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
            OutboxPayload::Delegate { .. } | OutboxPayload::Action { .. } => OutboxKind::Delegate,
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
    /// The claim this commit is made under, for a commit that reports what a dispatcher
    /// worker learned from an agent; `None` for API commits. When set, the store applies the
    /// commit only while the row is still `inflight` under exactly this claim (see [`Lease`]),
    /// otherwise it writes nothing and answers [`CommitOutcome::Fenced`].
    /// [`ThreadStore::create_thread`] ignores it: a thread that does not exist yet has no
    /// outbox row to be claimed.
    pub lease: Option<Lease>,
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
    /// The commit's [`Lease`] is no longer the row's current claim (another worker claimed the
    /// row since, or it finished, or it was skipped): nothing was written.
    Fenced,
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

/// A worker's claim on an outbox row, and the fencing token of everything it writes.
///
/// `attempt` is the row's `attempts` counter as [`ThreadStore::claim_outbox`] left it: it grows
/// by one on every claim, so of two claims of the same row the later one has the larger
/// number. A store acts on behalf of a lease only while the row is `inflight`, held by
/// `owner`, at exactly `attempt`. A worker that was paused past its lease and resumes after
/// the row was claimed again (even by the same `owner` name) therefore has a token that no
/// longer matches, and every write it tries is refused. Expiry alone does not revoke a
/// lease: an expired claim nobody took over is still the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// The row.
    pub id: OutboxId,
    /// The claimer's name.
    pub owner: String,
    /// The row's `attempts` at claim time.
    pub attempt: u32,
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

impl OutboxItem {
    /// The claim this row was handed out under: `None` while nobody holds it (`lease_owner`
    /// is unset).
    pub fn lease(&self) -> Option<Lease> {
        self.lease_owner.as_ref().map(|owner| Lease {
            id: self.id,
            owner: owner.clone(),
            attempt: self.attempts,
        })
    }
}

/// A count of the open outbox rows at one instant, for metrics and autoscaling
/// ([`ThreadStore::outbox_stats`]). A row is open while it is `pending` or `inflight`; the
/// three counts partition the open rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutboxStats {
    /// Rows a worker could claim now: `pending` and due, or `inflight` with an expired lease.
    pub due: u64,
    /// Rows `pending` in backoff, not due yet.
    pub waiting: u64,
    /// Rows `inflight` with a live lease: a worker is on them.
    pub leased: u64,
    /// The earliest moment a currently due row became due (its `next_attempt_at`, or its
    /// `lease_until` when the lease lapsed). `None` when nothing is due.
    pub oldest_due_at: Option<Timestamp>,
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
///
/// Adapters box their driver's error as the `source` (ADR 0009: no driver type in a port). A
/// message describes this layer only; [`report`](orch_core::report) prints the chain.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The thread does not exist.
    #[error("not found")]
    NotFound,
    /// The thread changed since it was read; re-read and retry.
    #[error("version conflict")]
    VersionConflict,
    /// The backing store could not be reached, or gave up on a transient condition
    /// (connection loss, pool timeout, deadlock, serialization failure).
    #[error("store unavailable")]
    Unavailable {
        /// The driver's error.
        #[source]
        source: BoxError,
    },
    /// Stored data could not be decoded, or breaks an invariant.
    #[error("corrupt data: {detail}")]
    Corrupt {
        /// What is wrong, without secrets.
        detail: String,
        /// The decoding error, when there is one.
        #[source]
        source: Option<BoxError>,
    },
    /// The store failed in a way retrying cannot fix (a rejected statement, a missing table):
    /// a bug or a broken deployment.
    #[error("store failed")]
    Internal {
        /// The driver's error.
        #[source]
        source: BoxError,
    },
}

impl StoreError {
    /// A transient store failure.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        StoreError::Unavailable {
            source: source.into(),
        }
    }

    /// Bad stored data with no lower error.
    pub fn corrupt(detail: impl Into<String>) -> Self {
        StoreError::Corrupt {
            detail: detail.into(),
            source: None,
        }
    }

    /// Bad stored data caused by `source`.
    pub fn corrupt_with(detail: impl Into<String>, source: impl Into<BoxError>) -> Self {
        StoreError::Corrupt {
            detail: detail.into(),
            source: Some(source.into()),
        }
    }

    /// A failure retrying cannot fix.
    pub fn internal(source: impl Into<BoxError>) -> Self {
        StoreError::Internal {
            source: source.into(),
        }
    }
}

impl Classify for StoreError {
    fn class(&self) -> ErrorClass {
        match self {
            StoreError::NotFound => ErrorClass::NotFound,
            StoreError::VersionConflict => ErrorClass::Conflict,
            StoreError::Unavailable { .. } => ErrorClass::Transient,
            StoreError::Corrupt { .. } => ErrorClass::Corrupt,
            StoreError::Internal { .. } => ErrorClass::Internal,
        }
    }
}

/// Threads, their append-only event logs, the agent binding and the outbox.
///
/// Every mutating method is atomic. Outbox methods that take a [`Lease`] only act on a row
/// that is `inflight`, held by the lease's owner at the lease's attempt, and return `false`
/// otherwise (a lost lease); [`commit`](Self::commit) does the same for its `lease` field and
/// answers [`CommitOutcome::Fenced`].
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

    /// One transaction. Locks the thread; if `commit.lease` is set and is not the current
    /// claim of its outbox row (see [`Lease`]; the row must belong to this thread) the result
    /// is [`CommitOutcome::Fenced`] (nothing written; the lease is checked before the
    /// version, so a fenced worker never retries); `version != expected_version` gives
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

    /// Extends the lease. `false` if `lease` is no longer the row's current claim.
    fn renew_lease(
        &self,
        lease: &Lease,
        until: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Sets `sent_at` (the message reached the agent and a task id is known) and applies the
    /// binding update, in one transaction.
    fn mark_sent(
        &self,
        lease: &Lease,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Puts the row back to `pending`, due at `next_attempt_at`, and records the error.
    fn retry_outbox(
        &self,
        lease: &Lease,
        next_attempt_at: Timestamp,
        error: String,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Finishes the row.
    fn complete_outbox(
        &self,
        lease: &Lease,
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

    /// Counts the open outbox rows at `now`: `due` is `pending` and due, or `inflight` with an
    /// expired lease (`lease_until <= now`, the [`claim_outbox`](Self::claim_outbox)
    /// predicate); `waiting` is `pending` and not due; `leased` is `inflight` with a live
    /// lease. `oldest_due_at` is the earliest `next_attempt_at` (or lapsed `lease_until`)
    /// among the due rows. Rows in any other status are not counted. Read-only.
    fn outbox_stats(
        &self,
        now: Timestamp,
    ) -> impl Future<Output = Result<OutboxStats, StoreError>> + Send;

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod error_tests {
    use super::*;

    #[test]
    fn class_table() {
        let all = [
            StoreError::NotFound,
            StoreError::VersionConflict,
            StoreError::unavailable(std::io::Error::other("down")),
            StoreError::corrupt("bad row"),
            StoreError::corrupt_with("bad row", std::io::Error::other("json")),
            StoreError::internal(std::io::Error::other("syntax")),
        ];
        for e in all {
            // Exhaustive: a new variant forces a class decision.
            let expected = match &e {
                StoreError::NotFound => ErrorClass::NotFound,
                StoreError::VersionConflict => ErrorClass::Conflict,
                StoreError::Unavailable { .. } => ErrorClass::Transient,
                StoreError::Corrupt { .. } => ErrorClass::Corrupt,
                StoreError::Internal { .. } => ErrorClass::Internal,
            };
            assert_eq!(e.class(), expected, "{e}");
        }
    }

    #[test]
    fn a_display_never_repeats_its_source() {
        for e in [
            StoreError::unavailable(std::io::Error::other("pool timed out")),
            StoreError::corrupt_with("bad row", std::io::Error::other("bad json")),
            StoreError::internal(std::io::Error::other("syntax error")),
        ] {
            let source = std::error::Error::source(&e).expect("a source").to_string();
            assert!(!e.to_string().contains(&source), "{e}");
            assert!(orch_core::report(&e).ends_with(&source));
        }
    }
}
