//! What the application does with what an agent says of its sessions (`child-sessions/v1`, ADR
//! 0072, `docs/orchestrator.md` "Child sessions"): a thread for each session, the parent's events
//! once each, and the round a child thread follows without sending anything.
//!
//! The adapter has read the wire strictly into [`SessionNews`]; the dispatcher hands it here from
//! the stream of a task. Nothing here is ordered by a notification: the store and the ledgers
//! decide, so the same news said twice, or said by two workers, writes nothing twice.
//!
//! Order of a report about a session of a parent: **the child thread first** (the parent's
//! `child_session_opened` names it), then the parent's events. A crash between the two leaves a
//! child thread the parent has not noted; the next report finds it by `(parent, session)`
//! ([`ChildThread::Existing`]) and writes the parent's events.

use std::sync::atomic::{AtomicU64, Ordering};

use orch_core::{
    Actor, AgentTarget, ChildOf, ForkKind, Input, MAX_CHILD_SESSIONS, PersonPrompt, RoundReport,
    SessionBy, SessionNews, SessionReport, ThreadId, ThreadRecord, TransitionError, child_title,
    start_child_for,
};
use orch_ports::{
    ChildThread, Clock, IdGen, Lease, NewThreadRecord, Ports, StoreError, ThreadStore, Topic,
};

use super::{App, AppError, ApplyOutcome, LateSource};

/// Why news about a session was not applied, for the counter's `reason` (a closed set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildSessionDrop {
    /// The adapter refused what the agent said (`AgentUpdate::ChildSessionsRejected`): it broke
    /// the contract.
    Rejected,
    /// The report failed the core's bounds.
    Malformed,
    /// The parent notes [`MAX_CHILD_SESSIONS`] sessions already; no thread was made for the next.
    LedgerFull,
    /// The parent notes the session, and its child thread was deleted: the thread is not made
    /// again.
    ChildDeleted,
    /// The news is about a thread that cannot have it: a child is no parent, a round is no news of
    /// a thread that is no child, or another session's round.
    Refused,
    /// The agent answered a followed round with a task of another context than the child thread's:
    /// nothing of it was applied and the follow failed.
    ForeignContext,
}

impl ChildSessionDrop {
    /// Every reason, in the order the counter lists them.
    pub const ALL: [ChildSessionDrop; 6] = [
        ChildSessionDrop::Rejected,
        ChildSessionDrop::Malformed,
        ChildSessionDrop::LedgerFull,
        ChildSessionDrop::ChildDeleted,
        ChildSessionDrop::Refused,
        ChildSessionDrop::ForeignContext,
    ];

    /// The label of the counter.
    pub const fn as_str(self) -> &'static str {
        match self {
            ChildSessionDrop::Rejected => "rejected",
            ChildSessionDrop::Malformed => "malformed",
            ChildSessionDrop::LedgerFull => "ledger_full",
            ChildSessionDrop::ChildDeleted => "child_deleted",
            ChildSessionDrop::Refused => "refused",
            ChildSessionDrop::ForeignContext => "foreign_context",
        }
    }

    const fn index(self) -> usize {
        match self {
            ChildSessionDrop::Rejected => 0,
            ChildSessionDrop::Malformed => 1,
            ChildSessionDrop::LedgerFull => 2,
            ChildSessionDrop::ChildDeleted => 3,
            ChildSessionDrop::Refused => 4,
            ChildSessionDrop::ForeignContext => 5,
        }
    }
}

/// What became of a report about a session of a parent.
enum Told {
    /// Applied, or dropped and counted.
    Applied,
    /// A session a person started, which the parent's start has not answered yet: the start's
    /// answer makes its thread, so this report waits for it (a later listing says it again).
    Deferred,
    /// The claim is no longer ours.
    Fenced,
}

/// The most parents whose last listing is remembered (the memory is emptied past it).
const MAX_LISTINGS_KEPT: usize = 1024;

/// A key of what a listing says: equal listings have equal keys.
fn listing_key(reports: &[SessionReport]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(reports)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug, Default)]
pub(crate) struct ChildSessionCounters {
    dropped: [AtomicU64; 6],
    /// The last `Listed` applied for each parent (a hash), so that the same listing, which a poll
    /// repeats, is not read from the store again; bounded.
    last_listed: std::sync::Mutex<std::collections::HashMap<ThreadId, u64>>,
}

/// The counter of news about sessions this process did not apply, by why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildSessionStats {
    /// `child_sessions_dropped_total{reason}`.
    pub dropped: [(ChildSessionDrop, u64); 6],
}

impl<P: Ports> App<P> {
    /// The counter of news about sessions this process did not apply.
    pub fn child_session_stats(&self) -> ChildSessionStats {
        ChildSessionStats {
            dropped: ChildSessionDrop::ALL.map(|why| {
                (
                    why,
                    self.child_session_counters.dropped[why.index()].load(Ordering::Relaxed),
                )
            }),
        }
    }

    /// News about a session of `thread` was not applied, for `why`. The counter names no thread and
    /// no agent; the debug line says which thread.
    pub(crate) fn child_session_dropped(&self, why: ChildSessionDrop, thread: ThreadId) {
        self.child_session_counters.dropped[why.index()].fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%thread, reason = why.as_str(), "news about a child session was not applied");
    }

    /// Applies what an agent said of its sessions on the stream of `thread`'s task (ADR 0072).
    ///
    /// For a **parent**, each report of [`SessionNews::Listed`] or [`SessionNews::Event`]: the child
    /// thread is made (once per `(parent, session)`: a session whose thread exists is told the
    /// report instead), then the parent's `child_session_opened`, `child_session_reported` and
    /// `child_session_closed` that the parent's ledger says are owed. For a **child**, a
    /// [`SessionNews::OfRound`]: the child is told what its session looks like, and the parent hears
    /// of the round. A report that fails the core's bounds, a session past the parent's 256 and the
    /// session of a thread that was deleted are dropped and counted
    /// ([`child_session_stats`](Self::child_session_stats)); none of it fails the task.
    ///
    /// `lease` is the claim of the outbox row the caller works for: the writes to `thread` are
    /// fenced with it. Returns `false` when it is no longer ours (the caller stops), `true` when the
    /// news was applied or dropped.
    ///
    /// # Errors
    /// [`AppError::Store`] when the store fails, [`AppError::Contended`] when `thread` kept changing.
    pub async fn child_sessions_news(
        &self,
        thread: ThreadId,
        lease: Option<&Lease>,
        news: SessionNews,
    ) -> Result<bool, AppError> {
        match news {
            SessionNews::Listed(reports) => {
                // a poll says the same listing again and again: once applied, it is not read again
                let key = listing_key(&reports);
                if self.listed_last(thread) == Some(key) {
                    return Ok(true);
                }
                let mut deferred = false;
                for report in reports {
                    match Box::pin(self.session_of_parent(thread, lease, report)).await? {
                        Told::Fenced => return Ok(false),
                        Told::Deferred => deferred = true,
                        Told::Applied => {}
                    }
                }
                // a listing that left a session waiting is read again
                if !deferred {
                    self.listed_applied(thread, key);
                }
                Ok(true)
            }
            SessionNews::Event(_, report) => Ok(!matches!(
                Box::pin(self.session_of_parent(thread, lease, report)).await?,
                Told::Fenced
            )),
            SessionNews::OfRound(report) => {
                Box::pin(self.session_of_round(thread, lease, report)).await
            }
        }
    }

    /// A report about a session of `parent_id`.
    async fn session_of_parent(
        &self,
        parent_id: ThreadId,
        lease: Option<&Lease>,
        report: SessionReport,
    ) -> Result<Told, AppError> {
        if report.check().is_err() {
            self.child_session_dropped(ChildSessionDrop::Malformed, parent_id);
            return Ok(Told::Applied);
        }
        let store = self.ports.store();
        let Some(parent) = store.get_thread(None, parent_id).await? else {
            self.late_input_dropped(LateSource::Dispatcher, parent_id);
            return Ok(Told::Applied);
        };
        // a child has no children of its own
        if parent.child_of.is_some() {
            self.child_session_dropped(ChildSessionDrop::Refused, parent_id);
            return Ok(Told::Applied);
        }
        // The session a person started is made by the answer to the start, with the person's prompt
        // before its first round: this report must not make it first, without
        if report.by == Some(SessionBy::Person)
            && parent.job.sessions.note(&report.session).is_none()
            && !parent.job.session_starts.is_empty()
        {
            return Ok(Told::Deferred);
        }
        let child = match parent.job.sessions.note(&report.session) {
            Some(note) => match store.get_thread(None, note.child).await? {
                Some(child) => {
                    self.tell_child(child.id, &report).await?;
                    child.id
                }
                None => {
                    self.child_session_dropped(ChildSessionDrop::ChildDeleted, parent_id);
                    return Ok(Told::Applied);
                }
            },
            None => {
                if parent.job.sessions.len() >= MAX_CHILD_SESSIONS {
                    self.child_session_dropped(ChildSessionDrop::LedgerFull, parent_id);
                    return Ok(Told::Applied);
                }
                let Some((child, created)) =
                    Box::pin(self.open_child(&parent, &report, None)).await?
                else {
                    return Ok(Told::Applied);
                };
                if !created {
                    self.tell_child(child.id, &report).await?;
                }
                child.id
            }
        };
        Ok(
            if Box::pin(self.tell_parent(parent_id, lease, report, child)).await? {
                Told::Applied
            } else {
                Told::Fenced
            },
        )
    }

    /// A round's own word about the session it belongs to, on the stream of the child thread
    /// `child_id`'s followed task.
    async fn session_of_round(
        &self,
        child_id: ThreadId,
        lease: Option<&Lease>,
        report: SessionReport,
    ) -> Result<bool, AppError> {
        if report.check().is_err() {
            self.child_session_dropped(ChildSessionDrop::Malformed, child_id);
            return Ok(true);
        }
        let Some(child) = self.ports.store().get_thread(None, child_id).await? else {
            self.late_input_dropped(LateSource::Dispatcher, child_id);
            return Ok(true);
        };
        let Some(of) = child.child_of else {
            self.child_session_dropped(ChildSessionDrop::Refused, child_id);
            return Ok(true);
        };
        if of.session != report.session {
            self.child_session_dropped(ChildSessionDrop::Refused, child_id);
            return Ok(true);
        }
        match self
            .apply(
                child_id,
                Input::ChildSessionSaid(report.clone()),
                None,
                None,
                lease,
            )
            .await
        {
            Ok(ApplyOutcome::Fenced) => return Ok(false),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => {}
            Err(e) => return Err(e),
        }
        Box::pin(self.tell_parent(of.thread, None, report, child_id)).await
    }

    /// The child thread `child` hears what its session looks like now. A thread that is gone has
    /// nobody to hear it.
    pub(super) async fn tell_child(
        &self,
        child: ThreadId,
        report: &SessionReport,
    ) -> Result<(), AppError> {
        match self
            .apply(
                child,
                Input::ChildSessionSaid(report.clone()),
                None,
                None,
                None,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(AppError::NotFound) => {
                self.child_session_dropped(ChildSessionDrop::ChildDeleted, child);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn listed_last(&self, parent: ThreadId) -> Option<u64> {
        self.child_session_counters
            .last_listed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&parent)
            .copied()
    }

    fn listed_applied(&self, parent: ThreadId, key: u64) {
        let mut map = self
            .child_session_counters
            .last_listed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() >= MAX_LISTINGS_KEPT {
            map.clear();
        }
        map.insert(parent, key);
    }

    /// The parent's three events for `report`, each once: written through the core, whose ledger
    /// decides what is owed. `lease` fences the write when the news came on the parent's own stream.
    pub(super) async fn tell_parent(
        &self,
        parent: ThreadId,
        lease: Option<&Lease>,
        report: SessionReport,
        child: ThreadId,
    ) -> Result<bool, AppError> {
        let Some(binding) = self.ports.store().get_binding(parent).await? else {
            self.late_input_dropped(LateSource::Dispatcher, parent);
            return Ok(true);
        };
        let input = Input::ChildSession {
            actor: Actor::agent(&binding.agent_id, binding.revision),
            report,
            child,
        };
        match self.apply(parent, input, None, None, lease).await {
            Ok(ApplyOutcome::Fenced) => Ok(false),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(true),
            Err(AppError::NotFound) => {
                self.late_input_dropped(LateSource::Dispatcher, parent);
                Ok(true)
            }
            Err(AppError::Transition(TransitionError::ChildSessionRefused(why))) => {
                tracing::debug!(%parent, %why, "the parent refused a child session");
                self.child_session_dropped(
                    match why {
                        orch_core::SessionRefusal::LedgerFull => ChildSessionDrop::LedgerFull,
                        orch_core::SessionRefusal::Malformed => ChildSessionDrop::Malformed,
                        // a person's start is refused to the person, never reported by an agent
                        orch_core::SessionRefusal::TooManyStarts
                        | orch_core::SessionRefusal::ChildThread => ChildSessionDrop::Refused,
                    },
                    parent,
                );
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// Makes the thread of the session `report` names, nested under `parent`'s row of the person's
    /// list, in one commit with its first decision (no event, the order to follow the report's
    /// round). `Some((thread, true))` for the thread this call made, `Some((thread, false))` for
    /// one that existed, `None` when the store refused it (counted).
    pub(super) async fn open_child(
        &self,
        parent: &ThreadRecord,
        report: &SessionReport,
        prompt: Option<PersonPrompt>,
    ) -> Result<Option<(ThreadRecord, bool)>, AppError> {
        let (snapshot, cmds) = start_child_for(report, prompt);
        let now = self.ports.clock().now();
        let target = AgentTarget {
            agent_id: parent.target.agent_id.clone(),
            release: parent.target.release.clone(),
        };
        let commit = self.build_commit(
            &target,
            snapshot.state,
            Some(snapshot.job),
            cmds,
            None,
            None,
            now,
        );
        let rail_parent = self
            .fork_rail_row(&parent.owner, parent, ForkKind::Fork)
            .await?
            .map(|row| row.id);
        let id = ThreadId(self.ports.ids().new_id());
        let new = NewThreadRecord {
            id,
            owner: parent.owner.clone(),
            title: child_title(report),
            // the helper's own words, fixed: nothing replaces them
            description: Some(report.description.clone()).filter(|d| !d.trim().is_empty()),
            target,
            context_id: Some(report.context_id.clone()),
            rail_parent,
            child_of: Some(ChildOf {
                thread: parent.id,
                session: report.session.clone(),
            }),
            now,
        };
        match self.ports.store().open_child_thread(new, commit).await {
            Ok(ChildThread::Created { thread, .. }) => {
                self.notify(Topic::Thread(thread.id)).await;
                self.notify(Topic::Thread(parent.id)).await;
                self.notify(Topic::Outbox).await;
                Ok(Some((thread, true)))
            }
            Ok(ChildThread::Existing(thread)) => Ok(Some((thread, false))),
            // the parent went while this was decided: nothing to make a child of
            Err(StoreError::NotFound) => {
                self.late_input_dropped(LateSource::Dispatcher, parent.id);
                Ok(None)
            }
            Err(StoreError::Refused(code)) => {
                tracing::debug!(parent = %parent.id, code, "the store refused a child thread");
                self.child_session_dropped(ChildSessionDrop::Refused, parent.id);
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The followed round of the child thread `child_id` is over: the parent hears of it before the
    /// delegate row ends (a crash between the two follows the round again and reports again, which
    /// writes nothing twice). The report is built from what the child thread itself knows: its
    /// ledger, its context and the state its binding records for the round's task.
    pub(crate) async fn report_round_over(&self, child_id: ThreadId) -> Result<(), AppError> {
        let store = self.ports.store();
        let Some(child) = store.get_thread(None, child_id).await? else {
            return Ok(());
        };
        let (Some(of), Some(ledger), Some(observed)) =
            (child.child_of, child.job.child, child.job.observed)
        else {
            return Ok(());
        };
        let Some(binding) = store.get_binding(child_id).await? else {
            return Ok(());
        };
        let report = SessionReport {
            session: of.session,
            context_id: binding.context_id.unwrap_or_default(),
            agent: ledger.agent,
            description: child
                .description
                .unwrap_or_default()
                .chars()
                .take(orch_core::MAX_SESSION_DESCRIPTION_CHARS)
                .collect(),
            keep_open: ledger.keep_open,
            by: observed.by,
            state: ledger.state,
            closing: ledger.closing,
            round: Some(RoundReport {
                number: observed.number,
                task_id: Some(observed.task_id),
                state: binding.task_state,
            }),
            parent_context_id: None,
        };
        if report.check().is_err() {
            self.child_session_dropped(ChildSessionDrop::Malformed, child_id);
            return Ok(());
        }
        self.tell_parent(of.thread, None, report, child_id).await?;
        Ok(())
    }
}
