//! Notification intents: a pure fold of a thread's log (ADR 0077, slice N1).
//!
//! Nothing outside the log makes a push. [`fold`] reads the events that exist and says what a
//! person should be told ([`NotifyIntent`]) and what is answered ([`Notify::Withdraw`]). It does no
//! I/O and reads no clock: an intent's expiry is the event's own time plus a [`NotifyTtl`].
//!
//! * The dedupe key is `(thread, cause, kind)`, `cause` being the `seq` of the event that made the
//!   intent, so folding the same events twice makes no second intent.
//! * [`collapse`] keeps, per thread and class, only the newest intent that was not withdrawn.
//! * Quiet rules that need no I/O: the person's own act makes no intent, and the end of a child
//!   thread makes none (its parent's does); a child's wait does. What depends on a device (a
//!   thread open on it) is the dispatcher's, never the fold's.
//!
//! Kinds the log cannot show yet (`checkpoint`, `over_limit`, and a `cancelled` that is not the
//! person's own stop) are not variants: a new one is a variant, so every match fails to compile.

use std::collections::BTreeSet;

use jiff::SignedDuration;

use crate::event::{ActorType, AgentStatus, Event, EventBody};
use crate::gate::pull_request_url;
use crate::ids::ThreadId;
use crate::thread::ThreadState;

/// What an intent is about, closed (ADR 0004): it is also the collapse class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotifyClass {
    /// Something waits for the person.
    NeedsYou,
    /// Something ended.
    Done,
}

/// What a client localises, closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NotifyKind {
    /// A form, a question or a credential waits.
    NeedsYou,
    /// The job finished.
    Done,
    /// The job failed.
    Failed,
    /// The job produced a pull request.
    PullRequest,
}

impl NotifyKind {
    /// The class the kind collapses under.
    pub fn class(self) -> NotifyClass {
        match self {
            NotifyKind::NeedsYou => NotifyClass::NeedsYou,
            NotifyKind::Done | NotifyKind::Failed | NotifyKind::PullRequest => NotifyClass::Done,
        }
    }
}

/// Where the thread stands in its family: a child thread is a session of another thread
/// (ADR 0072).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    /// A thread of its own.
    Top,
    /// A child session's thread: its ends make no intent, its waits do.
    Child,
}

/// How long a push stays useful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyTtl {
    /// For a wait.
    pub needs_you: SignedDuration,
    /// For an end.
    pub done: SignedDuration,
}

impl Default for NotifyTtl {
    /// The proposal of ADR 0077: 24 h and 6 h.
    fn default() -> Self {
        NotifyTtl {
            needs_you: SignedDuration::from_hours(24),
            done: SignedDuration::from_hours(6),
        }
    }
}

/// A thing to tell the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyIntent {
    /// The collapse class.
    pub class: NotifyClass,
    /// What happened.
    pub kind: NotifyKind,
    /// The thread.
    pub thread: ThreadId,
    /// What waits for the person at this event for `needs_you`, `1` otherwise.
    pub count: u32,
    /// The `seq` of the event that made it.
    pub cause: i64,
    /// When the push is useless.
    pub expires: Option<jiff::Timestamp>,
}

/// What the fold says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notify {
    /// Tell the person.
    Intent(NotifyIntent),
    /// The wait is answered: the pending pushes of `(thread, class)` are superseded.
    Withdraw {
        /// The thread.
        thread: ThreadId,
        /// The class.
        class: NotifyClass,
    },
}

/// The intents and withdrawals of the events with `seq > after`, in order. `events` is the
/// thread's log from its start (the count of waits needs what came before `after`).
///
/// Pure: the same log and `after` give the same answer, and a log folded up to `after` already
/// gives nothing for its own events.
pub fn fold(
    thread: ThreadId,
    role: ThreadRole,
    ttl: &NotifyTtl,
    after: i64,
    events: &[Event],
) -> Vec<Notify> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<(i64, NotifyKind)> = BTreeSet::new();
    let mut waiting: u32 = 0;
    for event in events {
        let fresh = event.seq > after;
        let mut intent = |kind: NotifyKind, count: u32, ttl: SignedDuration| {
            if fresh && seen.insert((event.seq, kind)) {
                out.push(Notify::Intent(NotifyIntent {
                    class: kind.class(),
                    kind,
                    thread,
                    count,
                    cause: event.seq,
                    expires: event.at.checked_add(ttl).ok(),
                }));
            }
        };
        match &event.body {
            EventBody::AgentStatus(s)
                if matches!(
                    s.status,
                    AgentStatus::InputRequired | AgentStatus::AuthRequired
                ) =>
            {
                waiting = waiting.saturating_add(1);
                intent(NotifyKind::NeedsYou, waiting, ttl.needs_you);
            }
            // The person answered: a surface action, or a message of theirs.
            EventBody::UiAction(_) => {
                if waiting > 0 && fresh {
                    out.push(Notify::Withdraw {
                        thread,
                        class: NotifyClass::NeedsYou,
                    });
                }
                waiting = 0;
            }
            EventBody::UserMessage(_) if event.actor.r#type == ActorType::User => {
                if waiting > 0 && fresh {
                    out.push(Notify::Withdraw {
                        thread,
                        class: NotifyClass::NeedsYou,
                    });
                }
                waiting = 0;
            }
            EventBody::ThreadState(s) => match s.state {
                ThreadState::Done if role == ThreadRole::Top => {
                    intent(NotifyKind::Done, 1, ttl.done);
                }
                ThreadState::Failed if role == ThreadRole::Top => {
                    intent(NotifyKind::Failed, 1, ttl.done);
                }
                // A cancel is the person's own act: no push to them for it.
                ThreadState::Cancelled
                | ThreadState::Done
                | ThreadState::Failed
                | ThreadState::Queued
                | ThreadState::Working
                | ThreadState::Verifying
                | ThreadState::Blocked => {}
            },
            EventBody::Artifact(a) if role == ThreadRole::Top => {
                if pull_request_url(&a.name, a.uri.as_deref(), a.text.as_deref()).is_some() {
                    intent(NotifyKind::PullRequest, 1, ttl.done);
                }
            }
            _ => {}
        }
    }
    out
}

/// What is left to send: per thread and class, the newest intent, and none the person has
/// answered after it (a `needs_you` followed by its withdrawal). The order of the rest is kept.
pub fn collapse(notifications: Vec<Notify>) -> Vec<NotifyIntent> {
    let mut kept: Vec<NotifyIntent> = Vec::new();
    for n in notifications {
        match n {
            Notify::Intent(i) => {
                kept.retain(|k| !(k.thread == i.thread && k.class == i.class));
                kept.push(i);
            }
            Notify::Withdraw { thread, class } => {
                kept.retain(|k| !(k.thread == thread && k.class == class));
            }
        }
    }
    kept
}
