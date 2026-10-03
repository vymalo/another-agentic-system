use std::future::Future;
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, AgentTaskState, BoxError, Classify, ErrorClass, Event, EventBody,
    EventKind, ForkKind, ForkNode, Job, NONCE_LEN, PushedRef, ShareLevel, ShareNonce, ThreadId,
    ThreadRecord, ThreadState, UiDelivery, UserId, WatchKey,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::inbox::{
    InboxFinal, InboxId, InboxItem, InboxLease, NewInbox, NewTimer, Parking, Received,
};

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
    /// Description: a fork starts with its parent's (ADR 0035); `None` for every other thread.
    pub description: Option<String>,
    /// Target agent and release.
    pub target: AgentTarget,
    /// The A2A context id every task of this thread shares.
    pub context_id: String,
    /// Creation time.
    pub now: Timestamp,
}

/// Where a new thread is cut from, for [`ThreadStore::fork_thread`] (ADR 0029).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForkOrigin {
    /// The thread to copy, which must be the new thread's owner's.
    pub parent: ThreadId,
    /// The last event of the parent to copy (0 copies none). The parent's log must reach it.
    pub cut: i64,
    /// How the fork is made: an `edit` is a sibling of its parent in a family of edits.
    pub kind: ForkKind,
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
    /// Ask the verifier agent to review the pushed commit (ADR 0018). Its task is the row's own
    /// ([`OutboxItem::task_id`]), never the thread's binding: the verifier is not the worker.
    Verify,
    /// Ask the model for a title of the thread (an orchestrator's own request, not an agent's).
    Title,
    /// Ask the model for a description of the thread (ADR 0035).
    Description,
    /// Send a message into the agent's **running task** (`steer/v1`, ADR 0036). Claimed beside the
    /// thread's delegation in flight and in order among the thread's other steer rows; a row the
    /// agent cannot take becomes a `delegate` ([`ThreadStore::requeue_as_delegate`]).
    Steer,
    /// Ask an agent the person mentioned to do part of the work, for the job's agent (ADR 0026,
    /// `orch_core::Command::Ask`). Its task is the row's own ([`OutboxItem::task_id`]), never the
    /// thread's binding: the asked agent is not the worker. **Unordered**: it waits behind neither
    /// the thread's delegation (the asking agent is running inside it) nor another ask.
    Ask,
}

fn is_false(value: &bool) -> bool {
    !*value
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
        /// The message starts the thread's next job (ADR 0020): it is sent as a new A2A task
        /// whatever the binding says of the last one. Absent (`false`) in a row written before
        /// the field existed.
        #[serde(default, skip_serializing_if = "is_false")]
        new_job: bool,
        /// What to tell the agent of the person's UI catalog (ADR 0023): the catalog itself, or a
        /// reference to the current one. Absent when the thread has none, and in a row written
        /// before the field existed. Public data, never a secret.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_catalog: Option<UiDelivery>,
        /// The agents `text` mentions (ADR 0026, `mentions/v1`), with their offsets in `text`:
        /// the references the person sent, as the `user_message` records them. Absent when there
        /// are none and in a row written before the field existed. The agent is told them only
        /// when its live card lists the extension.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mentions: Vec<orch_core::Mention>,
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
        /// What to tell the agent of the person's UI catalog, as for
        /// [`OutboxPayload::Delegate`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_catalog: Option<UiDelivery>,
    },
    /// Cancel. `job` is the job of the thread the person asked to stop (ADR 0020): a row claimed
    /// after that job ended and the next began is finished without calling the agent. A row
    /// written before the field existed has none and means the thread's current job. (Such a row
    /// was stored as the bare string `"cancel"`; the Postgres codec reads it as `{"cancel": {}}`.)
    Cancel {
        /// The job to cancel, when the row says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job: Option<u32>,
    },
    /// Ask `verifier` to review `pushed` (ADR 0018). `attempt` and `verification` say which
    /// verification of the job this answers: a row whose verification is over is dropped, and
    /// the verdict it produces carries them, so the core can tell a stale one.
    Verify {
        /// The attempt the request was made in.
        attempt: u32,
        /// The verification it was made in.
        verification: u32,
        /// The agent that reviews.
        verifier: AgentId,
        /// What it reviews (for the operator: the prompt is `text`).
        pushed: PushedRef,
        /// The prompt, written by the core.
        text: String,
    },
    /// Ask the model for a title of the thread (`orch_core::Command::RequestTitle`). `ask` is the
    /// number of the request in the thread's ledger. The conversation to title is read from the
    /// log when the row is worked, so the row holds nothing of it.
    Title {
        /// Which request.
        ask: u8,
    },
    /// Ask the model for a description of the thread (`orch_core::Command::RequestDescription`,
    /// ADR 0035): `job` is the job whose end asks. The conversation is read from the log when the
    /// row is worked, so the row holds nothing of it.
    Description {
        /// The job whose end asks.
        job: u32,
    },
    /// Send `text` to the agent's running task (`orch_core::Command::Steer`, ADR 0036). It holds
    /// what the delegation it may become holds, so that the fallback is that delegation byte for
    /// byte; the message itself carries no catalog (`steer/v1` has none).
    Steer {
        /// User text.
        text: String,
        /// Selected release channel or revision, for the delegation it may become: a steer goes to
        /// a task that runs, so it is never sent with one.
        release: Option<String>,
        /// What to tell the agent of the person's UI catalog, as for
        /// [`OutboxPayload::Delegate`]: used only when the row becomes a delegation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ui_catalog: Option<UiDelivery>,
        /// The agents `text` mentions (ADR 0026, `mentions/v1`), as for
        /// [`OutboxPayload::Delegate`]: a steer is told them like a delegation is (when the live
        /// card lists `mentions/v1`), and the delegation it becomes keeps them. Absent when there
        /// are none and in a row written before the field existed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mentions: Vec<orch_core::Mention>,
    },
    /// Ask `agent` to do `text` for the job's agent (`orch_core::Command::Ask`, ADR 0026). `job` and
    /// `ask` say which ask of which job this is: a row whose ask has ended (its deadline, a stop,
    /// the end of the asking task) is dropped, and the result it produces carries `ask`, so the core
    /// can tell a late one. The context the message is sent in is
    /// [`ask_context`](orch_core::ask_context), derived from the thread and the agent.
    Ask {
        /// The job the ask belongs to.
        job: u32,
        /// The ask's number in the job, from 1.
        ask: u32,
        /// The agent asked.
        agent: AgentId,
        /// How deep in a chain of asks: 1 for the addressed agent's ask. The asked agent's grant to
        /// the thread's tools carries it.
        depth: u8,
        /// What it is asked: untrusted text from an agent.
        text: String,
        /// The task to continue, when the agent's last ask of this job ended waiting for an
        /// answer; absent for a new task.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        continue_task: Option<String>,
        /// For a new task, the earlier tasks of this agent in this job it refers to, oldest first.
        /// Absent when there are none.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reference_task_ids: Vec<String>,
    },
}

impl OutboxPayload {
    /// The kind matching this payload.
    pub fn kind(&self) -> OutboxKind {
        match self {
            OutboxPayload::Delegate { .. } | OutboxPayload::Action { .. } => OutboxKind::Delegate,
            OutboxPayload::Cancel { .. } => OutboxKind::Cancel,
            OutboxPayload::Verify { .. } => OutboxKind::Verify,
            OutboxPayload::Title { .. } => OutboxKind::Title,
            OutboxPayload::Description { .. } => OutboxKind::Description,
            OutboxPayload::Steer { .. } => OutboxKind::Steer,
            OutboxPayload::Ask { .. } => OutboxKind::Ask,
        }
    }

    /// The delegation a steer stands for when it is not sent into the task: the same text, release,
    /// catalog and mentions, a message of the job it was written in (`new_job` false). `None` for
    /// any other payload. The stores' [`requeue_as_delegate`](ThreadStore::requeue_as_delegate) rewrites the
    /// row with it.
    pub fn steer_as_delegate(&self) -> Option<OutboxPayload> {
        match self {
            OutboxPayload::Steer {
                text,
                release,
                ui_catalog,
                mentions,
            } => Some(OutboxPayload::Delegate {
                text: text.clone(),
                release: release.clone(),
                new_job: false,
                ui_catalog: ui_catalog.clone(),
                mentions: mentions.clone(),
            }),
            OutboxPayload::Delegate { .. }
            | OutboxPayload::Action { .. }
            | OutboxPayload::Cancel { .. }
            | OutboxPayload::Verify { .. }
            | OutboxPayload::Title { .. }
            | OutboxPayload::Description { .. }
            | OutboxPayload::Ask { .. } => None,
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

/// A change of a thread's share, written to its row in the commit of the `thread_shared` or
/// `thread_unshared` event that says so (ADR 0040).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingChange {
    /// Set the level and the nonce the link is built on, and `shared_at` to the commit's `now`
    /// ([`Command::SetSharing`](orch_core::Command::SetSharing)). A nonce that another thread
    /// has is [`StoreError::Corrupt`] and nothing is written: nonces are unique across threads.
    Set {
        /// Who may read.
        level: ShareLevel,
        /// The capability.
        nonce: ShareNonce,
    },
    /// Make the thread private and forget the nonce
    /// ([`Command::ClearSharing`](orch_core::Command::ClearSharing)).
    Clear,
}

/// Everything that changes in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The thread state after the commit.
    pub new_state: ThreadState,
    /// The job ledger after the commit, written in the same transaction as the state and under
    /// the same version check; `None` leaves the stored job as it is. On
    /// [`ThreadStore::create_thread`] `None` starts the thread with [`Job::default`] (no gate).
    pub job: Option<Job>,
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
    /// Watches to insert for this thread ([`Command::Watch`](orch_core::Command::Watch)). In
    /// the same transaction the store re-arms every `parked` inbox row whose correlation is one
    /// of these keys (`pending`, due at `now`), so a report that arrived before its watch is
    /// not lost. Inserting a key that is watched already is a no-op: the first thread keeps it
    /// (a store may log that another thread asked for it).
    pub watches: Vec<WatchKey>,
    /// Timers to arm ([`Command::Schedule`](orch_core::Command::Schedule)): each becomes an
    /// inbox row with `source = "timer"`, due `after` this commit's `now`, and a key derived
    /// from the thread and the timer ([`NewTimer::idempotency_key`]), so arming the same timer
    /// twice is one row.
    pub timers: Vec<NewTimer>,
    /// The inbox claim this commit is made under, for a commit that applies an inbox row to the
    /// thread; `None` otherwise. When set, the store applies the commit only while the row is
    /// still `inflight` under exactly this claim (otherwise it writes nothing and answers
    /// [`CommitOutcome::Fenced`]) and marks the row `applied` in the same transaction.
    /// [`ThreadStore::create_thread`] ignores it.
    ///
    /// A commit that carries this and nothing else (see
    /// [`only_finishes_inbox`](Self::only_finishes_inbox)) is how an input that changes nothing
    /// finishes its row. It is checked against `expected_version` like any commit, so a "nothing
    /// to do" decided on a thread that has moved since is refused
    /// ([`StoreError::VersionConflict`]) and decided again, and the input is never dropped
    /// behind the thread's back. It leaves the thread as it is: no version bump, no
    /// `updated_at`, no wakeup.
    pub inbox: Option<InboxLease>,
    /// With [`lease`](Self::lease): also finish the claimed outbox row as this, in the same
    /// transaction (and under the same fence). It is how a row that hands its work on (a
    /// redelivered message that starts the next job, ADR 0020) ends together with what it did,
    /// so a crash between the two cannot leave the work done and the row claimable again.
    /// Ignored without a lease and by [`ThreadStore::create_thread`].
    pub finishes_outbox: Option<OutboxFinal>,
    /// The thread's new title ([`Command::SetTitle`](orch_core::Command::SetTitle)), written to
    /// the thread in the same transaction as the `thread_titled` event that says so; `None`
    /// leaves the title as it is. Ignored by [`ThreadStore::create_thread`], which takes the
    /// title from the new thread.
    pub title: Option<String>,
    /// The thread's new description ([`Command::SetDescription`](orch_core::Command::SetDescription)),
    /// written to the thread in the same transaction as the `thread_described` event that says so;
    /// `None` leaves it as it is, and `Some("")` clears it. Ignored by
    /// [`ThreadStore::create_thread`], which takes the description from the new thread.
    pub description: Option<String>,
    /// The thread's new share ([`Command::SetSharing`](orch_core::Command::SetSharing) or
    /// [`Command::ClearSharing`](orch_core::Command::ClearSharing)), written to the thread in the
    /// same transaction as the `thread_shared` or `thread_unshared` event that says so (ADR 0040);
    /// `None` leaves it as it is. Ignored by [`ThreadStore::create_thread`] and
    /// [`ThreadStore::fork_thread`]: a new thread, a fork included, is private.
    pub sharing: Option<SharingChange>,
    /// Finish the thread's unsent `delegate` and `steer` rows as `skipped`, in this transaction and
    /// before this commit's own rows are inserted (the rows
    /// [`skip_unsent_delegates`](ThreadStore::skip_unsent_delegates) finishes: `pending`, or
    /// `inflight` with an expired lease, and never sent). It is how the commit that starts the
    /// next job after a Stop & send (ADR 0036) supersedes the abandoned job's delegations: done
    /// by a separate call before the commit, a second worker that decided the same thing could
    /// skip the row the first one had just written. A commit that is refused (a version
    /// conflict, a lost fence, a duplicate) skips nothing. Ignored by
    /// [`ThreadStore::create_thread`].
    pub skip_unsent_delegates: bool,
}

impl Commit {
    /// Whether this commit writes nothing to the thread but the completion of its inbox row:
    /// an `inbox` claim and no events, outbox rows, binding update, job, title, description, watches or timers.
    /// (A store also requires `new_state` to be the thread's current state before it leaves the
    /// thread untouched.)
    pub fn only_finishes_inbox(&self) -> bool {
        self.inbox.is_some()
            && self.events.is_empty()
            && self.outbox.is_empty()
            && self.binding.is_none()
            && self.job.is_none()
            && self.title.is_none()
            && self.description.is_none()
            && self.sharing.is_none()
            && self.watches.is_empty()
            && self.timers.is_empty()
    }
}

/// Result of [`ThreadStore::commit`].
// One value per commit, matched at a handful of call sites; the thread carries its job ledger,
// which makes `Applied` large. Boxing it would only add a deref to every one of them.
#[allow(clippy::large_enum_variant)]
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
    /// The verifier's, or the asked agent's, A2A task, for a `verify` or an `ask` row once its
    /// message reached the agent ([`ThreadStore::mark_verify_sent`]); `None` for every other row,
    /// whose task is on the thread's binding.
    pub task_id: Option<String>,
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

/// Threads, their append-only event logs, the agent binding, the outbox, the watches and the
/// inbox.
///
/// Every mutating method is atomic. Outbox methods that take a [`Lease`] only act on a row
/// that is `inflight`, held by the lease's owner at the lease's attempt, and return `false`
/// otherwise (a lost lease); [`commit`](Self::commit) does the same for its `lease` field and
/// answers [`CommitOutcome::Fenced`]. The inbox methods that take an [`InboxLease`] work the
/// same way, and so does `commit` for its `inbox` field. The inbox is part of this trait and
/// not a port of its own because a commit must be atomic across the thread and the inbox row
/// it applies (ADR 0016).
pub trait ThreadStore: Send + Sync + 'static {
    /// Cheap reachability check (readiness).
    fn ping(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Atomically inserts the thread (state, job and events from `first`, version 1), its binding
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
    /// exclusive cursor; an unknown or foreign cursor yields an empty list. A thread made by an
    /// edit of another's message ([`ForkKind::Edit`]) is listed only with `include_edits`: it is
    /// a branch of a conversation the list already shows (ADR 0029). The limit counts the threads
    /// listed, so a page is as long as it can be whatever is hidden.
    fn list_threads(
        &self,
        owner: &UserId,
        before: Option<ThreadId>,
        limit: u32,
        include_edits: bool,
    ) -> impl Future<Output = Result<Vec<ThreadRecord>, StoreError>> + Send;

    /// Atomically inserts a thread that begins as a copy of another's log (ADR 0029): the thread
    /// (state, job and events from `first`, version 1, `forked_from` set), its binding (agent from
    /// the target, context from `new`, then `first.binding`), the parent's events `1..=origin.cut`
    /// **as they are** (same `seq`, time, actor and data, no idempotency key), then `first`'s
    /// events as `cut + 1..` and its outbox rows, watches and timers, as
    /// [`create_thread`](Self::create_thread) does.
    ///
    /// The parent must be `new.owner`'s, else [`StoreError::NotFound`] (a foreign thread and a
    /// missing one look alike) and nothing is written; a parent whose log is shorter than the cut,
    /// or a `new.id` that exists, is [`StoreError::Corrupt`]. A parent that is deleted afterwards
    /// leaves the fork whole, with [`ForkedFrom::thread_id`](orch_core::ForkedFrom) unset.
    fn fork_thread(
        &self,
        new: NewThreadRecord,
        origin: ForkOrigin,
        first: Commit,
    ) -> impl Future<Output = Result<(ThreadRecord, Vec<Event>), StoreError>> + Send;

    /// The thread whose share has this nonce, or `None` when no thread does: the lookup behind a
    /// link (ADR 0040). Nobody is asked who: the caller checks the link's MAC and what the
    /// thread is shared at. A thread that was unshared, or shared again with another nonce, is
    /// not found by the old one, and a fork is never found by its parent's.
    fn thread_by_share_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
    ) -> impl Future<Output = Result<Option<ThreadRecord>, StoreError>> + Send;

    /// The family of edits `thread` belongs to, the owner's: the thread it started from (found by
    /// following edit links up while the parent exists), and every thread made from those by an
    /// edit, oldest first (ties by id), at most 1000. Each [`ForkNode`] says how its thread
    /// relates to its parent: for a thread made by an edit, the parent, the cut and the seq of
    /// the message that replaces the parent's at `cut + 1` (the first message of a person after the
    /// `thread_forked` event). A thread made by [`ForkKind::Fork`], or whose parent is gone, has
    /// no link. A thread that is not the owner's, or does not exist, has no family: empty.
    fn fork_family(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Vec<ForkNode>, StoreError>> + Send;

    /// One transaction. Locks the thread; if `commit.lease` is set and is not the current
    /// claim of its outbox row (see [`Lease`]; the row must belong to this thread) the result
    /// is [`CommitOutcome::Fenced`] (nothing written; the lease is checked before the
    /// version, so a fenced worker never retries); `version != expected_version` gives
    /// [`StoreError::VersionConflict`] (nothing written); an idempotency key already present
    /// in the thread's log gives [`CommitOutcome::Duplicate`] (nothing written); otherwise
    /// appends the events with `seq = last_seq + 1..`, sets state (and the job, when the commit
    /// carries one), bumps version, `last_seq` and `updated_at`, inserts the outbox rows,
    /// applies the binding update, inserts the watches and re-arms the parked inbox rows they
    /// match, arms the timers and marks the commit's inbox row `applied`. The `inbox` claim is
    /// checked like the `lease` one, before the version.
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

    /// The newest `limit` events of `kind`, newest first: one bounded read for "the last artifact",
    /// which would otherwise be a scan of the whole log.
    fn latest_events(
        &self,
        thread: ThreadId,
        kind: EventKind,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;

    /// The newest `ui_catalog` event of `thread` whose catalog has this `digest`, or `None` when the
    /// thread has none (or no thread has this id): the thread-tools endpoint reads the catalog its
    /// ledger names as current with it (ADR 0023). A query by what the event holds, not a window
    /// of the newest events: the ledger records every new digest, so any number of later,
    /// lower-versioned catalogs can follow the current one in the log.
    fn ui_catalog_event(
        &self,
        thread: ThreadId,
        digest: &str,
    ) -> impl Future<Output = Result<Option<Event>, StoreError>> + Send;

    /// The thread's A2A binding.
    fn get_binding(
        &self,
        thread: ThreadId,
    ) -> impl Future<Output = Result<Option<AgentBinding>, StoreError>> + Send;

    /// Claims up to `limit` rows (oldest first): `pending` and due, or `inflight` with an
    /// expired lease (`lease_until <= now`). A delegate row is claimable only if no older
    /// `pending`/`inflight` delegate row exists for the same thread (per-thread ordering,
    /// including rows claimed earlier in the same call); a steer row likewise waits for an older
    /// open **steer** row of its thread and for nothing else (the delegation in flight stays open
    /// until the agent's turn ends, and a steer is for that very turn: ADR 0036); cancel, verify,
    /// ask, title and description rows are unrestricted (a verification runs while the delegation
    /// that caused it is still being finished, and never waits for a later delegation; an ask is
    /// made by the agent that the delegation in flight is running, so it never waits behind it,
    /// and two asks of a job run side by side).
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

    /// For a `verify` or an `ask` row: sets `sent_at` and the row's
    /// [`task_id`](OutboxItem::task_id) (the message reached the verifier, or the asked agent, and
    /// its task is known), and nothing on the thread's binding, which is the worker's. `false` if
    /// `lease` is no longer the row's current claim.
    fn mark_verify_sent(
        &self,
        lease: &Lease,
        task_id: String,
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

    /// Rewrites a `steer` row the agent did not take into the delegation it stands for
    /// ([`OutboxPayload::steer_as_delegate`]): kind `delegate`, `pending` and due at `now`, the lease
    /// released. The row keeps its place in the thread's order (its position among the outbox rows
    /// and its creation time), so the delegation waits behind the one in flight and goes out in
    /// the order the person wrote; `attempts`, which fences the leases, is not reset. `false` if
    /// `lease` is no longer the row's current claim, or the row is not a `steer` row.
    fn requeue_as_delegate(
        &self,
        lease: &Lease,
        now: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Marks the thread's not-yet-sent `delegate` and `steer` rows (`sent_at` is null) as
    /// `skipped`: those `pending`, and those `inflight` whose lease expired before `now`. Returns
    /// the count.
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

    // ---------------------------------------------------------------- inbox

    /// Stores an inbound report as a `pending` row, due at `now`. A row with the same
    /// `(source, idempotency_key)` (in any status) makes this [`Received::Duplicate`] and
    /// writes nothing.
    fn receive(
        &self,
        row: NewInbox,
        now: Timestamp,
    ) -> impl Future<Output = Result<Received, StoreError>> + Send;

    /// Claims up to `limit` rows, earliest `available_at` first (then by id): `pending` and
    /// due (`available_at <= now`), or `inflight` with an expired lease (`lease_until <= now`).
    /// Sets `inflight`, owner, `lease_until = now + lease` and `attempts += 1`. Concurrent
    /// claimers never get the same row. A timer whose time has not come is not claimed.
    fn claim_inbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<InboxItem>, StoreError>> + Send;

    /// Sets a claimed row aside until its watch exists: `inflight` becomes `parked`, stamped
    /// `now`. If the watch for the row's correlation exists by then (a commit added it since
    /// the worker looked), the row goes back to `pending`, due at `now`, and the answer is
    /// [`Parking::Rearmed`]; the check and the write are one step that a concurrent commit
    /// adding the watch cannot interleave with, so a parked row is never left behind a watch.
    /// Either way the claim ends without having failed: the row's claims so far are refunded
    /// ([`InboxItem::refunded`](crate::InboxItem::refunded)), so parking and re-arming never use
    /// up the attempt limit.
    fn park_inbox(
        &self,
        lease: &InboxLease,
        now: Timestamp,
    ) -> impl Future<Output = Result<Parking, StoreError>> + Send;

    /// Puts the claimed row back to `pending`, due at `available_at`, and records the error.
    /// `false` if `lease` is no longer the row's current claim.
    fn retry_inbox(
        &self,
        lease: &InboxLease,
        available_at: Timestamp,
        error: String,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Finishes the claimed row without a thread commit. `false` if `lease` is no longer the
    /// row's current claim (a commit under it already marked the row `applied`, or another
    /// worker holds it now).
    fn complete_inbox(
        &self,
        lease: &InboxLease,
        outcome: InboxFinal,
        now: Timestamp,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Marks `parked` rows whose `parked_at <= parked_at_or_before` as `expired`. Returns the
    /// count. A worker calls it with `now - INBOX_PARKED_TTL_SECS`.
    fn expire_parked_inbox(
        &self,
        parked_at_or_before: Timestamp,
        now: Timestamp,
    ) -> impl Future<Output = Result<u32, StoreError>> + Send;

    /// On shutdown: sets `lease_until = now` for inbox rows leased to `owner` so another
    /// replica claims them immediately, and refunds the released claim (it did not fail, so it
    /// does not count against the attempt limit). Returns the count.
    fn release_inbox_leases(
        &self,
        owner: &str,
        now: Timestamp,
    ) -> impl Future<Output = Result<u32, StoreError>> + Send;

    /// The thread that watches `key` (a [`WatchKey`] as text, which is what a row's
    /// `correlation` holds), if any.
    fn get_watch(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<Option<ThreadId>, StoreError>> + Send;

    /// Inspection: one inbox row, whatever its status.
    fn get_inbox(
        &self,
        id: InboxId,
    ) -> impl Future<Output = Result<Option<InboxItem>, StoreError>> + Send;

    /// Inspection: the inbox row of `(source, idempotency_key)`, whatever its status.
    fn find_inbox(
        &self,
        source: &str,
        idempotency_key: &str,
    ) -> impl Future<Output = Result<Option<InboxItem>, StoreError>> + Send;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod payload_tests {
    use orch_core::{AgentId, Mention};
    use serde_json::json;

    use super::*;

    fn mention() -> Mention {
        Mention {
            agent_id: AgentId::new("coder"),
            label: "@coder".to_owned(),
            start: 5,
            end: 11,
            card_url: None,
        }
    }

    /// A steer row written before mentions existed has no such member and reads without any; a
    /// steer without any does not write the member; one with them writes and reads them back.
    #[test]
    fn a_steer_payload_reads_without_mentions_and_round_trips_with_them() {
        let old: OutboxPayload =
            serde_json::from_value(json!({"steer": {"text": "hi", "release": null}})).unwrap();
        let OutboxPayload::Steer { mentions, .. } = &old else {
            panic!("a steer");
        };
        assert!(mentions.is_empty());
        assert_eq!(
            serde_json::to_value(&old).unwrap(),
            json!({"steer": {"text": "hi", "release": null}}),
            "an empty set is not written"
        );

        let with = OutboxPayload::Steer {
            text: "echo @coder".to_owned(),
            release: None,
            ui_catalog: None,
            mentions: vec![mention()],
        };
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["steer"]["mentions"][0]["label"], "@coder");
        assert_eq!(json["steer"]["mentions"].as_array().map(Vec::len), Some(1));
        assert_eq!(serde_json::from_value::<OutboxPayload>(json).unwrap(), with);
        // the delegation it becomes holds the same references
        let Some(OutboxPayload::Delegate { mentions, .. }) = with.steer_as_delegate() else {
            panic!("a delegation");
        };
        assert_eq!(mentions, [mention()]);
    }
}
