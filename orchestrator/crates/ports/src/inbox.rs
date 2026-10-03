//! The inbox: unsolicited machine input (webhook reports, timers) that waits in the store until
//! a worker applies it to a thread (ADR 0016).
//!
//! The types here belong to the [`ThreadStore`](crate::ThreadStore) port, whose inbox methods
//! share a transaction with the thread commit: a row is marked applied in the same commit that
//! applies it, so a crash between the two cannot happen.
//!
//! ```text
//! pending ──claim──> inflight ──commit / complete──> applied
//!    ^                  │  ^                          dead
//!    │                  │  └── lease lapsed: claimed again (attempts + 1)
//!    └── watch added ── parked ──ttl──> expired
//! ```
//!
//! `attempts` counts claims and only ever goes up: it is the fencing token of a claim. Not every
//! claim is a try that failed, so a row also counts the claims it was *refunded*
//! ([`InboxItem::refunded`]): one handed back at shutdown, and every claim up to the one that
//! parked the row (a park is a wait for a watch, not a failure). What is left,
//! [`InboxItem::counted_attempts`], is what `INBOX_MAX_ATTEMPTS` limits.

use jiff::{SignedDuration, Timestamp};
use orch_core::{CiReport, ThreadId, Timer};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The `source` of the rows [`Commit::timers`](crate::Commit::timers) inserts. `App::receive`
/// refuses it: only the store makes timers.
pub const TIMER_SOURCE: &str = "timer";

/// Identifier of an inbox row. The row's idempotency key in the thread's log is `inbox:<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxId(pub Uuid);

impl std::fmt::Display for InboxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// What an inbox row carries (stored as JSON in the row's `payload`, tagged by `kind`). Closed:
/// a new kind is a new variant, a new `kind` value and a migration (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxPayload {
    /// A deadline armed by [`Command::Schedule`](orch_core::Command::Schedule). It names its
    /// thread, so it needs no watch.
    Timer {
        /// The thread that armed it.
        thread: ThreadId,
        /// What to feed back as [`Input::TimerFired`](orch_core::Input::TimerFired).
        timer: Timer,
    },
    /// A CI provider's completed check, already normalised by a surface. It finds its thread
    /// through the row's `correlation` (a watch key).
    CiReport(CiReport),
}

impl InboxPayload {
    /// The value of the row's `kind` column.
    pub fn kind(&self) -> &'static str {
        match self {
            InboxPayload::Timer { .. } => "timer",
            InboxPayload::CiReport(_) => "ci_report",
        }
    }
}

/// A row to receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInbox {
    /// Row id.
    pub id: InboxId,
    /// Who sent it (`github`, `generic`, …); `(source, idempotency_key)` is unique.
    pub source: String,
    /// The sender's own id of this delivery; a redelivery carries the same one.
    pub idempotency_key: String,
    /// What it says.
    pub payload: InboxPayload,
    /// The watch key that says which thread it is about, when the payload is not enough.
    pub correlation: Option<String>,
}

/// The answer to [`ThreadStore::receive`](crate::ThreadStore::receive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// A new row, `pending` and due at once.
    Stored {
        /// The row.
        id: InboxId,
    },
    /// A row with this `(source, idempotency_key)` exists already: nothing was written.
    Duplicate,
}

/// Lifecycle of an inbox row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxStatus {
    /// Waiting to be claimed, from `available_at` on.
    Pending,
    /// Claimed by a worker under a lease.
    Inflight,
    /// No watch matched its correlation yet; a later commit that adds the watch re-arms it.
    Parked,
    /// Done: the input was applied (or found to change nothing).
    Applied,
    /// Parked for longer than the time-to-live.
    Expired,
    /// Gave up: a permanent error, or too many attempts.
    Dead,
}

impl InboxStatus {
    /// The value of the `status` column.
    pub fn as_str(self) -> &'static str {
        match self {
            InboxStatus::Pending => "pending",
            InboxStatus::Inflight => "inflight",
            InboxStatus::Parked => "parked",
            InboxStatus::Applied => "applied",
            InboxStatus::Expired => "expired",
            InboxStatus::Dead => "dead",
        }
    }
}

/// An inbox row as seen by a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    /// Row id.
    pub id: InboxId,
    /// Who sent it.
    pub source: String,
    /// The sender's id of the delivery.
    pub idempotency_key: String,
    /// The `kind` column: [`InboxPayload::kind`] of what was written.
    pub kind: String,
    /// The `payload` column as stored; [`decode`](Self::decode) reads it. It is not decoded by
    /// the store, so one row this build cannot read never spoils the batch it is claimed with.
    pub payload: serde_json::Value,
    /// The watch key it is matched by.
    pub correlation: Option<String>,
    /// Status.
    pub status: InboxStatus,
    /// Earliest claim (a timer's due time; a re-armed row's re-arm time).
    pub available_at: Timestamp,
    /// How many times it has been claimed (including the current claim). It never goes down:
    /// it is the fencing token of [`InboxLease`].
    pub attempts: u32,
    /// How many of those claims do not count against the attempt limit: claims released at
    /// shutdown, and all claims up to the one that parked the row. See
    /// [`counted_attempts`](Self::counted_attempts).
    pub refunded: u32,
    /// Current lease holder.
    pub lease_owner: Option<String>,
    /// Lease expiry.
    pub lease_until: Option<Timestamp>,
    /// When it was parked, while it is.
    pub parked_at: Option<Timestamp>,
    /// Last error text.
    pub last_error: Option<String>,
    /// When it was received.
    pub created_at: Timestamp,
}

/// The payload of a row could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot read the {kind} payload of inbox row {id}: {detail}")]
pub struct UndecodablePayload {
    /// The row.
    pub id: InboxId,
    /// Its `kind` column.
    pub kind: String,
    /// What is wrong, without any of the payload.
    pub detail: String,
}

impl InboxItem {
    /// The claims that count against the attempt limit, the current one included: those that
    /// ended in a failure to retry, or in a worker that vanished (its lease lapsed). A claim
    /// handed back at shutdown, or one that parked the row, is not among them.
    pub fn counted_attempts(&self) -> u32 {
        self.attempts.saturating_sub(self.refunded)
    }

    /// The claim this row was handed out under: `None` while nobody holds it.
    pub fn lease(&self) -> Option<InboxLease> {
        self.lease_owner.as_ref().map(|owner| InboxLease {
            id: self.id,
            owner: owner.clone(),
            attempt: self.attempts,
        })
    }

    /// The payload as a typed value.
    ///
    /// # Errors
    /// [`UndecodablePayload`] when the JSON is not a payload this build knows (a row written by
    /// a newer build, or damaged).
    pub fn decode(&self) -> Result<InboxPayload, UndecodablePayload> {
        serde_json::from_value(self.payload.clone()).map_err(|e| UndecodablePayload {
            id: self.id,
            kind: self.kind.clone(),
            // The error text may quote the payload; a payload came from outside.
            detail: format!("not a payload this build knows ({:?})", e.classify()),
        })
    }
}

/// A worker's claim on an inbox row, and the fencing token of everything it writes. As
/// [`Lease`](crate::Lease) is for the outbox: of two claims of the same row the later has the
/// larger `attempt`, and a store acts on a claim only while the row is `inflight`, held by
/// `owner`, at exactly `attempt`. It is a type of its own so that an outbox claim can never be
/// presented as an inbox one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxLease {
    /// The row.
    pub id: InboxId,
    /// The claimer's name.
    pub owner: String,
    /// The row's `attempts` at claim time.
    pub attempt: u32,
}

/// Final disposition of an inbox row that a worker finishes without a thread commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxFinal {
    /// Handled (the input changed nothing, or was a repeat).
    Applied,
    /// Gave up; carries the last error.
    Dead {
        /// Error text.
        error: String,
    },
}

/// What [`ThreadStore::park_inbox`](crate::ThreadStore::park_inbox) did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parking {
    /// The row is `parked`, waiting for its watch.
    Parked,
    /// The watch appeared while the worker was looking: the row went back to `pending`, due at
    /// once, and the next claim will find the watch.
    Rearmed,
    /// The lease is no longer the row's current claim: nothing was written.
    Lost,
}

/// A deadline to arm in a commit: the store makes an inbox row of it, due `after` the commit's
/// `now`, so the core never reads a clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTimer {
    /// Row id.
    pub id: InboxId,
    /// How long after the commit's `now`. A negative delay counts as none.
    pub after: SignedDuration,
    /// What to feed back.
    pub timer: Timer,
}

impl NewTimer {
    /// The row's idempotency key under [`TIMER_SOURCE`]: the thread, the timer and the attempt
    /// and verification (or the job and ask) it belongs to, so the same deadline armed twice (a
    /// replayed commit) is one row.
    pub fn idempotency_key(&self, thread: ThreadId) -> String {
        match self.timer {
            Timer::CiDeadline {
                attempt,
                verification,
            } => format!("{thread}:ci_deadline:{attempt}:{verification}"),
            Timer::VerifierDeadline {
                attempt,
                verification,
            } => format!("{thread}:verifier_deadline:{attempt}:{verification}"),
            Timer::AskDeadline { job, ask } => format!("{thread}:ask_deadline:{job}:{ask}"),
        }
    }

    /// The payload of the row.
    pub fn payload(&self, thread: ThreadId) -> InboxPayload {
        InboxPayload::Timer {
            thread,
            timer: self.timer,
        }
    }

    /// When the row is due, for a commit made at `now`.
    pub fn due(&self, now: Timestamp) -> Timestamp {
        let after = self.after.max(SignedDuration::ZERO);
        now.checked_add(after).unwrap_or(now)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn thread() -> ThreadId {
        ThreadId(Uuid::from_u128(7))
    }

    #[test]
    fn a_timer_key_names_the_thread_the_timer_and_the_verification() {
        let ci = NewTimer {
            id: InboxId(Uuid::from_u128(1)),
            after: SignedDuration::from_secs(60),
            timer: Timer::CiDeadline {
                attempt: 2,
                verification: 3,
            },
        };
        let next = NewTimer {
            timer: Timer::CiDeadline {
                attempt: 2,
                verification: 4,
            },
            ..ci.clone()
        };
        let verifier = NewTimer {
            timer: Timer::VerifierDeadline {
                attempt: 2,
                verification: 3,
            },
            ..ci.clone()
        };
        let ask = NewTimer {
            timer: Timer::AskDeadline { job: 2, ask: 3 },
            ..ci.clone()
        };
        let next_ask = NewTimer {
            timer: Timer::AskDeadline { job: 2, ask: 4 },
            ..ci.clone()
        };
        let keys = [
            ci.idempotency_key(thread()),
            next.idempotency_key(thread()),
            verifier.idempotency_key(thread()),
            ci.idempotency_key(ThreadId(Uuid::from_u128(8))),
            ask.idempotency_key(thread()),
            next_ask.idempotency_key(thread()),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(keys[0], ci.idempotency_key(thread()), "deterministic");
    }

    #[test]
    fn payloads_round_trip_through_json() {
        let timer = InboxPayload::Timer {
            thread: thread(),
            timer: Timer::VerifierDeadline {
                attempt: 1,
                verification: 1,
            },
        };
        let json = serde_json::to_value(&timer).unwrap();
        assert_eq!(json["kind"], "timer");
        assert_eq!(serde_json::from_value::<InboxPayload>(json).unwrap(), timer);
        assert_eq!(timer.kind(), "timer");
    }

    #[test]
    fn a_negative_delay_is_due_at_once() {
        let now: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let timer = NewTimer {
            id: InboxId(Uuid::from_u128(1)),
            after: SignedDuration::from_secs(-5),
            timer: Timer::CiDeadline {
                attempt: 1,
                verification: 1,
            },
        };
        assert_eq!(timer.due(now), now);
    }
}
