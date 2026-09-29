//! The fold from the event log to AG-UI frames (`docs/api/agui.md`, "Outbound").
//!
//! # The model
//!
//! A [`Projector`] is the state of the projection after some prefix of a thread's log. Feeding
//! it the next event returns the frames for that event. The state is small and is a function of
//! the events alone (never of the frames it emitted or of the [`Audience`]), so a client that
//! reconnects can rebuild it by folding the events up to its cursor, and every replica agrees.
//!
//! What the state tracks:
//!
//! - the **run** that is open, if any. AG-UI wants events inside runs, so a run opens at the
//!   first event of a burst of activity (a user message, or an event nobody asked for: a
//!   *producer-initiated* run) and closes at the event that ends it;
//! - the **invocation** (AG-UI subagent) that is open: one per stretch of the agent working on
//!   the thread, closed as finished, suspended (it waits for input), cancelled or failed;
//! - the **text message** that is open (an agent message streamed as partials);
//! - the **thread state** implied by the log, the pending **interrupt**, and the last failure.
//!
//! # When does a run close?
//!
//! The core appends a `thread_state` event when the thread *enters* `blocked`, `done`, `failed`
//! or `cancelled`, always right after the event that caused it. So a run normally closes at a
//! `thread_state` event, and its cause (`agent_status`, `error`) only prepares the closing
//! frames. The exceptions have no `thread_state` after them, because the thread was in that
//! state already: an event that arrives when *no run is open* (the thread is blocked or
//! finished) opens a producer-initiated run and closes it in the same call, by the state the
//! thread stands in. The invariant is: after every event, a run is open exactly when the thread
//! is `queued` or `working`.
//!
//! One thing cannot be told from the log: an `error` is either a delivery failure (which the
//! core follows with `thread_state{blocked|failed}`) or a refused cancel (which it does not).
//! The projector therefore remembers that the *previous* event was an error, and lets the
//! `thread_state` that may follow it decide.

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_proto::{
    self as agui, ActivitySnapshotEvent, Interrupt, InterruptId, Metadata, RunErrorEvent,
    RunFinishedEvent, RunFinishedOutcome, RunId, RunStartedEvent, StateSnapshotEvent,
    SubagentErrorEvent, SubagentFinishedEvent, SubagentFinishedOutcome, SubagentRunId,
    SubagentStartedEvent, TextMessageContentEvent, TextMessageEndEvent, TextMessageRole,
    TextMessageStartEvent,
};
use orch_core::{
    Actor, AgentMessageData, AgentStatus, AgentStatusData, AgentTarget, ArtifactData, ErrorData,
    Event, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use serde_json::{Value, json};

use crate::frame::{Audience, Frame};
use crate::translate::{KnownThread, ThreadView};
use crate::vocab::{
    ACTIVITY_ARTIFACT, ACTIVITY_ERROR, ACTIVITY_STATUS, CODE_AGENT_FAILED, CODE_DELIVERY_FAILED,
    actor_metadata, problem_metadata, response_schema, status_content,
};

/// What the projection knows about the thread besides its log: the parts of the thread record
/// that appear in `STATE_SNAPSHOT.snapshot.thread`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMeta {
    /// The thread. Its UUID is the AG-UI `threadId`.
    pub thread_id: ThreadId,
    /// The title, as stored.
    pub title: String,
    /// The agent (and release) the thread targets.
    pub target: AgentTarget,
}

/// A subagent invocation that is open.
#[derive(Debug, Clone)]
struct Invocation {
    id: SubagentRunId,
    /// The agent id: the subagent's name.
    name: String,
    /// Who produced the event that opened it (carries the ADR 0008 revision).
    actor: Actor,
}

/// An agent message that is open on the wire.
#[derive(Debug, Clone)]
struct OpenText {
    /// The id the agent (A2A) gave the message.
    source_id: String,
    /// The `messageId` on the wire (the same unless a rewrite forced a new message).
    wire_id: agui::MessageId,
}

/// What was emitted so far for one agent message id.
#[derive(Debug, Clone)]
struct TextRecord {
    text: String,
}

/// A wait for the user that no run has answered yet.
#[derive(Debug, Clone)]
struct PendingInterrupt {
    id: String,
    reason: &'static str,
    message: Option<String>,
    subagent: Option<SubagentRunId>,
}

/// Why the thread (or the last delegation) failed.
#[derive(Debug, Clone)]
struct Failure {
    message: String,
    code: &'static str,
}

/// How a run ends.
#[derive(Debug, Clone)]
enum RunClose {
    Success,
    Cancelled,
    Interrupt,
    Error(Failure),
}

/// How an invocation ends.
#[derive(Debug, Clone)]
enum InvocationClose {
    Finished,
    Canceled,
    Suspended(Vec<InterruptId>),
    Error(Failure),
}

/// The projection of one thread's log, folded event by event.
#[derive(Debug, Clone)]
pub struct Projector {
    meta: ThreadMeta,
    state: ThreadState,
    run: Option<RunId>,
    invocation: Option<Invocation>,
    /// The invocation that suspended and will reappear when the thread continues.
    suspended: Option<SubagentRunId>,
    open_text: Option<OpenText>,
    texts: BTreeMap<String, TextRecord>,
    interrupt: Option<PendingInterrupt>,
    failure: Option<Failure>,
    /// The message of the event just applied, when it was an `error` in an open run; consumed by
    /// the next event.
    pending_error: Option<String>,
    message_ids: BTreeSet<String>,
    run_ids: BTreeSet<String>,
}

fn is_active(state: ThreadState) -> bool {
    match state {
        ThreadState::Queued | ThreadState::Working => true,
        ThreadState::Blocked | ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
            false
        }
    }
}

impl Projector {
    /// A projector before the first event of the thread.
    pub fn new(meta: ThreadMeta) -> Self {
        Projector {
            meta,
            state: ThreadState::Queued,
            run: None,
            invocation: None,
            suspended: None,
            open_text: None,
            texts: BTreeMap::new(),
            interrupt: None,
            failure: None,
            pending_error: None,
            message_ids: BTreeSet::new(),
            run_ids: BTreeSet::new(),
        }
    }

    /// Whether a run is open: the projected stream is inside a `RUN_STARTED` … terminal pair.
    pub fn run_open(&self) -> bool {
        self.run.is_some()
    }

    /// The thread state the log implies so far.
    pub fn thread_state(&self) -> ThreadState {
        self.state
    }

    /// What [`translate`](crate::translate) needs to know about the thread, for `user` (the
    /// identity the edge vouched for).
    pub fn view(&self, user: &UserId) -> ThreadView {
        ThreadView::Known(KnownThread {
            user: user.clone(),
            agent: self.meta.target.agent_id.clone(),
            state: self.state,
            run_open: self.run.is_some(),
            open_interrupts: match (&self.interrupt, self.state) {
                (Some(i), ThreadState::Blocked) => vec![i.id.clone()],
                _ => Vec::new(),
            },
            message_ids: self.message_ids.clone(),
            run_ids: self.run_ids.clone(),
        })
    }

    /// The frames that re-open the current run for a client that joins in the middle of it: the
    /// same `RUN_STARTED`, a `SUBAGENT_STARTED` for the open invocation, and a `STATE_SNAPSHOT`.
    /// Every AG-UI stream must start with `RUN_STARTED`. Empty when no run is open (the next
    /// event that opens one starts the stream). None of the frames is a resume point.
    ///
    /// A cursor is always a resume point (no message open), but a client may send any number, so
    /// a message that is open is opened again with the text it has so far.
    pub fn resume_preamble(&self) -> Vec<Frame> {
        let Some(run) = &self.run else {
            return Vec::new();
        };
        let mut out: Vec<agui::Event> =
            vec![RunStartedEvent::new(self.meta.thread_id.to_string(), run.clone()).into()];
        if let Some(inv) = &self.invocation {
            out.push(Self::subagent_started(inv).into());
        }
        out.push(self.state_snapshot());
        if let (Some(open), Some(inv)) = (&self.open_text, &self.invocation)
            && let Some(record) = self.texts.get(&open.source_id)
        {
            let mut start =
                TextMessageStartEvent::new(open.wire_id.clone(), TextMessageRole::Assistant);
            start.name = Some(inv.name.clone());
            start.subagent_run_id = Some(inv.id.clone());
            start.base.metadata = Some(actor_metadata(&inv.actor));
            out.push(start.into());
            let mut content =
                TextMessageContentEvent::new(open.wire_id.clone(), record.text.clone());
            content.subagent_run_id = Some(inv.id.clone());
            out.push(content.into());
        }
        out.into_iter()
            .map(|event| Frame {
                event,
                resume_id: None,
            })
            .collect()
    }

    /// Folds the next log event in and returns its frames.
    ///
    /// The last frame carries `resume_id: Some(event.seq)` unless a text message is open after
    /// it. An event that produces no frames (a requester that already holds the only thing the
    /// event says) produces no resume point either.
    pub fn apply(&mut self, event: &Event, audience: Audience<'_>) -> Vec<Frame> {
        let pending_error = self.pending_error.take();
        let mut out: Vec<agui::Event> = Vec::new();
        match &event.body {
            EventBody::UserMessage(d) => self.on_user_message(event, d, audience, &mut out),
            EventBody::AgentMessage(d) => self.on_agent_message(event, d, &mut out),
            EventBody::AgentStatus(d) => self.on_agent_status(event, d, &mut out),
            EventBody::Artifact(d) => self.on_artifact(event, d, &mut out),
            EventBody::ThreadState(d) => {
                self.on_thread_state(event, d.state, pending_error, &mut out)
            }
            EventBody::Error(d) => self.on_error(event, d, &mut out),
        }
        let resumable = self.open_text.is_none();
        let last = out.len().checked_sub(1);
        out.into_iter()
            .enumerate()
            .map(|(i, event_out)| Frame {
                event: event_out,
                resume_id: (resumable && Some(i) == last).then_some(event.seq),
            })
            .collect()
    }

    // ---- events ------------------------------------------------------------------------

    fn on_user_message(
        &mut self,
        ev: &Event,
        d: &UserMessageData,
        audience: Audience<'_>,
        out: &mut Vec<agui::Event>,
    ) {
        let message_id = d
            .message_id
            .clone()
            .unwrap_or_else(|| format!("evt-{}", ev.seq));
        // The thread leaves `blocked` when the user answers; an answer clears the wait.
        if self.state == ThreadState::Blocked {
            self.state = ThreadState::Queued;
        }
        self.interrupt = None;
        self.failure = None;
        if self.run.is_none() {
            let run_id = d
                .run_id
                .clone()
                .unwrap_or_else(|| format!("run-{}", ev.seq));
            self.open_run(run_id, true, out);
        }
        self.message_ids.insert(message_id.clone());
        if audience.holds(&message_id) {
            return;
        }
        let mut start = TextMessageStartEvent::new(message_id.clone(), TextMessageRole::User);
        start.base.metadata = Some(actor_metadata(&ev.actor));
        out.push(start.into());
        out.push(TextMessageContentEvent::new(message_id.clone(), d.text.clone()).into());
        out.push(TextMessageEndEvent::new(message_id).into());
    }

    fn on_agent_message(&mut self, ev: &Event, d: &AgentMessageData, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);
        self.say(ev, d, out);
        if opened {
            self.settle(ev, out);
        }
    }

    /// Streams an agent message: `START` + `CONTENT`, then `CONTENT(suffix)` for each partial or
    /// final that extends what was said, and `END` on the final.
    ///
    /// A text that does not extend what was said cannot be a delta (open question 14): the open
    /// message is closed and the text starts a new message, under a derived id.
    fn say(&mut self, ev: &Event, d: &AgentMessageData, out: &mut Vec<agui::Event>) {
        let source = d.message_id.as_str();
        let Some(inv_id) = self.invocation.as_ref().map(|i| i.id.clone()) else {
            return;
        };
        let known = self.texts.get(source).cloned();
        match (self.open_text.clone(), known) {
            (Some(open), Some(record)) if open.source_id == source => {
                if let Some(suffix) = d.text.strip_prefix(record.text.as_str()) {
                    if !suffix.is_empty() {
                        let mut c = TextMessageContentEvent::new(open.wire_id.clone(), suffix);
                        c.subagent_run_id = Some(inv_id);
                        out.push(c.into());
                    }
                    self.texts.insert(
                        source.to_owned(),
                        TextRecord {
                            text: d.text.clone(),
                        },
                    );
                    if d.is_final {
                        self.close_text(out);
                    }
                } else {
                    self.close_text(out);
                    self.say_fresh(ev, d, true, out);
                }
            }
            (Some(_), _) => {
                self.close_text(out);
                let derive = self.texts.contains_key(source);
                self.say_fresh(ev, d, derive, out);
            }
            (None, Some(record)) => {
                // A message already said. The same words again are a duplicate delivery.
                if record.text != d.text {
                    self.say_fresh(ev, d, true, out);
                }
            }
            (None, None) => self.say_fresh(ev, d, false, out),
        }
    }

    fn say_fresh(
        &mut self,
        ev: &Event,
        d: &AgentMessageData,
        derive: bool,
        out: &mut Vec<agui::Event>,
    ) {
        let Some(inv) = self.invocation.clone() else {
            return;
        };
        let wire = if derive || self.message_ids.contains(&d.message_id) {
            format!("{}~{}", d.message_id, ev.seq)
        } else {
            d.message_id.clone()
        };
        let wire_id = agui::MessageId::new(wire);
        self.message_ids.insert(wire_id.as_str().to_owned());
        let mut start = TextMessageStartEvent::new(wire_id.clone(), TextMessageRole::Assistant);
        start.name = Some(inv.name.clone());
        start.subagent_run_id = Some(inv.id.clone());
        start.base.metadata = Some(actor_metadata(&ev.actor));
        out.push(start.into());
        let mut content = TextMessageContentEvent::new(wire_id.clone(), d.text.clone());
        content.subagent_run_id = Some(inv.id.clone());
        out.push(content.into());
        self.texts.insert(
            d.message_id.clone(),
            TextRecord {
                text: d.text.clone(),
            },
        );
        if d.is_final {
            let mut end = TextMessageEndEvent::new(wire_id);
            end.subagent_run_id = Some(inv.id);
            out.push(end.into());
        } else {
            self.open_text = Some(OpenText {
                source_id: d.message_id.clone(),
                wire_id,
            });
        }
    }

    fn on_agent_status(&mut self, ev: &Event, d: &AgentStatusData, out: &mut Vec<agui::Event>) {
        // The state effects the log implies without a `thread_state`: the agent starts working.
        let moved_to_working =
            d.status == AgentStatus::Working && self.state != ThreadState::Working;
        if d.status == AgentStatus::Working {
            self.state = ThreadState::Working;
        }
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);
        out.push(self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_STATUS,
            status_content(d.status, d.detail.as_deref()),
            &ev.actor,
            true,
        ));
        match d.status {
            AgentStatus::Submitted => {
                if opened {
                    self.settle(ev, out);
                }
            }
            AgentStatus::Working => {
                if moved_to_working && !opened {
                    out.push(self.state_snapshot());
                }
            }
            AgentStatus::InputRequired | AgentStatus::AuthRequired => {
                let reason = if d.status == AgentStatus::AuthRequired {
                    "auth_required"
                } else {
                    "input_required"
                };
                let id = format!("int-{}", ev.seq);
                let subagent = self.invocation.as_ref().map(|i| i.id.clone());
                self.interrupt = Some(PendingInterrupt {
                    id: id.clone(),
                    reason,
                    message: d.detail.clone(),
                    subagent,
                });
                self.close_invocation(InvocationClose::Suspended(vec![InterruptId::new(id)]), out);
                // Entering `blocked` is announced by the `thread_state` that follows. A wait
                // repeated while the thread already is blocked is not: close the run here.
                if self.state == ThreadState::Blocked {
                    self.close_run(RunClose::Interrupt, ev.seq, out);
                }
            }
            AgentStatus::Completed => self.close_invocation(InvocationClose::Finished, out),
            AgentStatus::Failed => {
                let failure = Failure {
                    message: d
                        .detail
                        .clone()
                        .unwrap_or_else(|| "the agent failed".to_owned()),
                    code: CODE_AGENT_FAILED,
                };
                self.failure = Some(failure.clone());
                self.close_invocation(InvocationClose::Error(failure), out);
            }
            AgentStatus::Canceled => self.close_invocation(InvocationClose::Canceled, out),
        }
    }

    fn on_artifact(&mut self, ev: &Event, d: &ArtifactData, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);
        let mut content = Metadata::new();
        content.insert("name".to_owned(), Value::from(d.name.clone()));
        for (key, value) in [
            ("mimeType", &d.mime_type),
            ("uri", &d.uri),
            ("text", &d.text),
        ] {
            if let Some(value) = value {
                content.insert(key.to_owned(), Value::from(value.clone()));
            }
        }
        out.push(self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_ARTIFACT,
            content,
            &ev.actor,
            true,
        ));
        if opened {
            self.settle(ev, out);
        }
    }

    fn on_error(&mut self, ev: &Event, d: &ErrorData, out: &mut Vec<agui::Event>) {
        let was_open = self.run.is_some();
        if !was_open {
            self.open_run(format!("run-{}", ev.seq), true, out);
        }
        let mut content = Metadata::new();
        content.insert("message".to_owned(), Value::from(d.message.clone()));
        content.insert("retryable".to_owned(), Value::from(d.retryable));
        out.push(self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_ERROR,
            content,
            &ev.actor,
            false,
        ));
        let failure = Failure {
            message: d.message.clone(),
            code: CODE_DELIVERY_FAILED,
        };
        self.failure = Some(failure.clone());
        if was_open || is_active(self.state) {
            // A delivery failure is followed by `thread_state{blocked|failed}`; a refused cancel
            // is not, and the run simply goes on. The next event tells which. (A thread that is
            // active has a run open, or is at the very start of its log, before any run.)
            self.pending_error = Some(d.message.clone());
        } else {
            // Nothing follows: the thread was blocked or finished already.
            let close = if self.state == ThreadState::Blocked && self.interrupt.is_some() {
                RunClose::Interrupt
            } else {
                RunClose::Error(failure)
            };
            self.close_run(close, ev.seq, out);
        }
    }

    fn on_thread_state(
        &mut self,
        ev: &Event,
        new: ThreadState,
        pending_error: Option<String>,
        out: &mut Vec<agui::Event>,
    ) {
        self.state = new;
        if self.run.is_none() {
            self.open_run(format!("run-{}", ev.seq), false, out);
        }
        match new {
            ThreadState::Queued | ThreadState::Working => out.push(self.state_snapshot()),
            ThreadState::Blocked => match pending_error {
                Some(message) => self.close_run(
                    RunClose::Error(Failure {
                        message,
                        code: CODE_DELIVERY_FAILED,
                    }),
                    ev.seq,
                    out,
                ),
                None => self.close_run(RunClose::Interrupt, ev.seq, out),
            },
            ThreadState::Done => self.close_run(RunClose::Success, ev.seq, out),
            ThreadState::Failed => {
                let failure = self.failure.clone().unwrap_or(Failure {
                    message: "the thread failed".to_owned(),
                    code: CODE_AGENT_FAILED,
                });
                self.close_run(RunClose::Error(failure), ev.seq, out);
            }
            ThreadState::Cancelled => self.close_run(RunClose::Cancelled, ev.seq, out),
        }
    }

    // ---- runs, invocations, text -------------------------------------------------------

    /// Opens a run: `RUN_STARTED`, then the `STATE_SNAPSHOT` (unless the caller sends its own).
    fn open_run(&mut self, run_id: String, snapshot: bool, out: &mut Vec<agui::Event>) {
        self.run_ids.insert(run_id.clone());
        let run = RunId::new(run_id);
        out.push(RunStartedEvent::new(self.meta.thread_id.to_string(), run.clone()).into());
        self.run = Some(run);
        if snapshot {
            out.push(self.state_snapshot());
        }
    }

    /// Opens a producer-initiated run if none is open. Returns whether it did.
    fn ensure_run(&mut self, ev: &Event, out: &mut Vec<agui::Event>) -> bool {
        if self.run.is_some() {
            return false;
        }
        self.open_run(format!("run-{}", ev.seq), true, out);
        true
    }

    /// Announces the agent's invocation if it is not open. A suspended invocation reappears
    /// under its own id: the same A2A task continues.
    fn ensure_invocation(&mut self, ev: &Event, out: &mut Vec<agui::Event>) {
        if self.invocation.is_some() {
            return;
        }
        let id = self
            .suspended
            .take()
            .unwrap_or_else(|| SubagentRunId::new(format!("sub-{}", ev.seq)));
        let inv = Invocation {
            id,
            name: ev.actor.name.clone(),
            actor: ev.actor.clone(),
        };
        out.push(Self::subagent_started(&inv).into());
        self.invocation = Some(inv);
    }

    fn subagent_started(inv: &Invocation) -> SubagentStartedEvent {
        let mut started = SubagentStartedEvent::new(inv.id.clone(), inv.name.clone());
        started.base.metadata = Some(actor_metadata(&inv.actor));
        started
    }

    fn close_text(&mut self, out: &mut Vec<agui::Event>) {
        if let Some(open) = self.open_text.take() {
            let mut end = TextMessageEndEvent::new(open.wire_id);
            end.subagent_run_id = self.invocation.as_ref().map(|i| i.id.clone());
            out.push(end.into());
        }
    }

    fn close_invocation(&mut self, how: InvocationClose, out: &mut Vec<agui::Event>) {
        self.close_text(out);
        let Some(inv) = self.invocation.take() else {
            return;
        };
        match how {
            InvocationClose::Finished => {
                out.push(SubagentFinishedEvent::new(inv.id, None).into());
            }
            InvocationClose::Canceled => {
                // The 1.0 outcome union has no cancelled member (open question 16).
                let mut finished = SubagentFinishedEvent::new(inv.id, None);
                finished.result = Some(json!({"status": "canceled"}));
                out.push(finished.into());
            }
            InvocationClose::Suspended(ids) => {
                self.suspended = Some(inv.id.clone());
                out.push(
                    SubagentFinishedEvent::new(
                        inv.id,
                        Some(SubagentFinishedOutcome::Suspended {
                            interrupt_ids: Some(ids),
                        }),
                    )
                    .into(),
                );
            }
            InvocationClose::Error(failure) => {
                out.push(
                    SubagentErrorEvent::new(inv.id, failure.message, Some(failure.code.to_owned()))
                        .into(),
                );
            }
        }
    }

    /// Closes the open run: everything it opened first (a text message, the invocation), then a
    /// `STATE_SNAPSHOT` and the terminal event.
    fn close_run(&mut self, close: RunClose, seq: i64, out: &mut Vec<agui::Event>) {
        self.close_text(out);
        match &close {
            RunClose::Success => self.close_invocation(InvocationClose::Finished, out),
            RunClose::Cancelled => self.close_invocation(InvocationClose::Canceled, out),
            RunClose::Interrupt => {
                let id = self.interrupt_or_fallback(seq).id;
                self.close_invocation(InvocationClose::Suspended(vec![InterruptId::new(id)]), out);
            }
            RunClose::Error(failure) => {
                self.close_invocation(InvocationClose::Error(failure.clone()), out);
            }
        }
        out.push(self.state_snapshot());
        let Some(run) = self.run.take() else {
            return;
        };
        let thread = self.meta.thread_id.to_string();
        match close {
            RunClose::Success => {
                out.push(RunFinishedEvent::new(thread, run, RunFinishedOutcome::success()).into());
            }
            RunClose::Cancelled => {
                out.push(RunFinishedEvent::new(thread, run, RunFinishedOutcome::Cancelled).into());
            }
            RunClose::Interrupt => {
                let pending = self.interrupt_or_fallback(seq);
                let mut interrupt = Interrupt::new(pending.id, pending.reason);
                interrupt.message = pending.message;
                interrupt.subagent_run_id = pending.subagent;
                interrupt.response_schema = Some(response_schema());
                out.push(
                    RunFinishedEvent::new(
                        thread,
                        run,
                        RunFinishedOutcome::Interrupt {
                            interrupts: vec![interrupt],
                        },
                    )
                    .into(),
                );
            }
            RunClose::Error(failure) => {
                let title = match failure.code {
                    CODE_AGENT_FAILED => "Agent failed",
                    CODE_DELIVERY_FAILED => "Delivery failed",
                    _ => "Error",
                };
                let mut error =
                    RunErrorEvent::new(failure.message.clone(), Some(failure.code.to_owned()));
                error.base.metadata = Some(problem_metadata(title, &failure.message));
                self.failure = Some(failure);
                out.push(error.into());
            }
        }
    }

    /// The pending interrupt, or (a thread that blocked with nothing asking) one minted from
    /// the event that closes the run, so the outcome is always answerable.
    fn interrupt_or_fallback(&mut self, seq: i64) -> PendingInterrupt {
        self.interrupt
            .get_or_insert_with(|| PendingInterrupt {
                id: format!("int-{seq}"),
                reason: "input_required",
                message: None,
                subagent: None,
            })
            .clone()
    }

    /// Closes a producer-initiated run at once, by the state the thread stands in. Used for an
    /// event that opened a run when the thread was not active: nothing follows it.
    fn settle(&mut self, ev: &Event, out: &mut Vec<agui::Event>) {
        if is_active(self.state) {
            return;
        }
        let close = match self.state {
            ThreadState::Blocked if self.interrupt.is_some() => RunClose::Interrupt,
            ThreadState::Blocked | ThreadState::Failed => {
                RunClose::Error(self.failure.clone().unwrap_or(Failure {
                    message: "the thread failed".to_owned(),
                    code: CODE_DELIVERY_FAILED,
                }))
            }
            ThreadState::Done => RunClose::Success,
            ThreadState::Cancelled => RunClose::Cancelled,
            ThreadState::Queued | ThreadState::Working => return,
        };
        self.close_run(close, ev.seq, out);
    }

    // ---- frames ------------------------------------------------------------------------

    fn state_snapshot(&self) -> agui::Event {
        let target = serde_json::to_value(&self.meta.target).unwrap_or(Value::Null);
        StateSnapshotEvent::new(json!({
            "thread": {
                "state": self.state.as_str(),
                "title": self.meta.title,
                "target": target,
            }
        }))
        .into()
    }

    /// An `ACTIVITY_SNAPSHOT`, attributed to the open invocation when `attribute`. The activity
    /// is a message of its own, so its id joins the ids the thread already holds.
    fn activity(
        &mut self,
        id: String,
        activity_type: &str,
        content: Metadata,
        actor: &Actor,
        attribute: bool,
    ) -> agui::Event {
        self.message_ids.insert(id.clone());
        let mut snapshot = ActivitySnapshotEvent::new(id, activity_type, content);
        if attribute {
            snapshot.subagent_run_id = self.invocation.as_ref().map(|i| i.id.clone());
        }
        snapshot.base.metadata = Some(actor_metadata(actor));
        snapshot.into()
    }
}
