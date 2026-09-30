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
//! - the **verifier's invocation**, when the gate asks a verifier agent (ADR 0018): a subagent of
//!   its own, named after the verifier, open from the `pending` `check_result` of the verifier
//!   source to its verdict (or to whatever ends the verification first);
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
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, AgentTarget, ArtifactData,
    CheckResult, CheckSource, CheckStatus, CiReport, ErrorData, Event, EventBody, GatePolicy,
    JobView, MAX_SURFACE_BYTES, Recognised, ReworkData, SurfaceOp, ThreadId, ThreadState,
    UiActionData, UiSurfaceData, UiVersion, UserId, UserMessageData, inspect, recognise_artifact,
    serialized_len,
};
use serde_json::{Value, json};

use crate::frame::{Audience, Frame};
use crate::translate::{KnownThread, ThreadView};
use crate::vocab::{
    A2UI_OPERATIONS_KEY, ACTIVITY_A2UI_SURFACE, ACTIVITY_ACTION, ACTIVITY_ARTIFACT, ACTIVITY_CHECK,
    ACTIVITY_CI, ACTIVITY_ERROR, ACTIVITY_REWORK, ACTIVITY_STATUS, CODE_AGENT_FAILED,
    CODE_CHECKS_FAILED, CODE_DELIVERY_FAILED, CODE_VERIFIER_FAILED, actor_metadata,
    problem_metadata, response_schema, status_content,
};

/// Characters of a commit hash a card shows (`shortSha`).
const SHORT_SHA_CHARS: usize = 7;

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
    /// The gate the thread's job runs under (`Job.gate`, fixed when the thread was created). A
    /// gate that requires something keeps the run open while the work is verified and adds
    /// `job` to every `STATE_SNAPSHOT` (ADR 0018); the default requires nothing.
    pub gate: GatePolicy,
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

/// An A2UI surface the thread has: every operation the agent sent for it so far, as sent. The
/// whole surface is what each snapshot carries, so the last snapshot of a surface renders it
/// completely on the live stream, on replay and in history.
#[derive(Debug, Clone)]
struct Surface {
    /// The `messageId` of its snapshots: `a2ui-<seq of the event that created it>`.
    message_id: String,
    /// The version its operations declare (the first one's).
    version: UiVersion,
    operations: Vec<Value>,
    bytes: usize,
    /// It outgrew [`MAX_SURFACE_BYTES`]: nothing more is replayed for it (the viewer keeps the
    /// last snapshot it got), and the viewer was told once.
    overflowed: bool,
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

/// How the verifier's invocation ends.
#[derive(Debug, Clone)]
enum VerifierClose {
    /// It answered: `passed` is its verdict. Either way it did its job.
    Verdict { passed: bool },
    /// The verification ended without its answer (another source failed the round, the user
    /// wrote, the thread was cancelled, or a hold whose cause the log does not name).
    Abandoned,
    /// The thread was held while the verifier was out, and the verifier is the only source that
    /// can have caused it: it could not be used or did not answer in time. Carries the reason
    /// the hold gave.
    Unavailable(String),
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
    /// The verifier's invocation, while a verification waits for its verdict.
    verifier: Option<Invocation>,
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
    /// The A2UI surfaces the thread has now (a deleted surface is gone).
    surfaces: BTreeMap<String, Surface>,
    /// The attempt the agent is on (`job.attempt` of the snapshot); from the `check_result` and
    /// `rework` events.
    attempt: u32,
    /// The commit the agent pushed in this attempt (`job.sha`); from its `branch` artifact.
    sha: Option<String>,
    /// How many verifications have started (the job's `verification`): the agent's `completed`
    /// under a gate starts one.
    verification: u32,
    /// A source of the gate failed and nothing has answered it yet: an `error` that follows is
    /// the gate running out of attempts.
    checks_failed: bool,
    /// The actor of the agent's last event, so that a rework can start the next attempt's
    /// invocation under the same name and revision.
    last_agent: Option<Actor>,
}

fn is_active(state: ThreadState) -> bool {
    match state {
        // A run stays open while the thread is verified (ADR 0018).
        ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => true,
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
            verifier: None,
            suspended: None,
            open_text: None,
            texts: BTreeMap::new(),
            interrupt: None,
            failure: None,
            pending_error: None,
            message_ids: BTreeSet::new(),
            run_ids: BTreeSet::new(),
            surfaces: BTreeMap::new(),
            attempt: 1,
            verification: 0,
            sha: None,
            checks_failed: false,
            last_agent: None,
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
            surfaces: self
                .surfaces
                .iter()
                .map(|(id, s)| (id.clone(), s.version))
                .collect(),
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
        if let Some(inv) = &self.verifier {
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
            EventBody::UiSurface(d) => self.on_ui_surface(event, d, &mut out),
            EventBody::UiAction(d) => self.on_ui_action(event, d, &mut out),
            EventBody::CheckResult(d) => self.on_check_result(event, d, &mut out),
            EventBody::Rework(d) => self.on_rework(event, d, &mut out),
            EventBody::CiResult(d) => self.on_ci_result(event, d, &mut out),
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
        // The thread leaves `blocked` when the user answers, and abandons a verification when
        // the user writes during one; an answer clears the wait.
        if self.run.is_some() {
            self.close_verifier(VerifierClose::Abandoned, out);
        }
        if matches!(self.state, ThreadState::Blocked | ThreadState::Verifying) {
            self.state = ThreadState::Queued;
        }
        self.interrupt = None;
        self.failure = None;
        self.checks_failed = false;
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
            AgentStatus::Completed => {
                self.close_invocation(InvocationClose::Finished, out);
                // Under a gate the agent finishing is not the end: the thread is verified, the
                // run stays open, and the `check_result` events that follow say how it went.
                if self.meta.gate.is_active() {
                    self.verification += 1;
                    self.state = ThreadState::Verifying;
                    out.push(self.state_snapshot());
                }
            }
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
        // What is verified is what the agent had pushed when it finished: the ledger is frozen
        // while the thread is verified, and so is `job.sha`.
        if self.meta.gate.is_active()
            && self.state != ThreadState::Verifying
            && let Recognised::Branch(pushed) = recognise_artifact(&d.name, d.text.as_deref())
        {
            self.sha = Some(pushed.commit);
        }
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

    /// An A2UI payload from the agent: each operation joins its surface, and each surface it
    /// touched is sent again **whole** as `a2ui-surface` with `replace: true`, so any one snapshot
    /// is enough to render the surface. A `deleteSurface` ends its surface: it is sent with the
    /// rest, and a later operation for that id starts a new surface under a new message id.
    ///
    /// The log holds only payloads that passed the envelope check; an operation that does not
    /// (an event written by something else) is skipped, never relayed.
    fn on_ui_surface(&mut self, ev: &Event, d: &UiSurfaceData, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);
        let mut touched: Vec<String> = Vec::new();
        let mut overflow: Option<String> = None;
        for op in &d.operations {
            let Ok(info) = inspect(op) else {
                continue;
            };
            let id = info.surface_id.to_owned();
            let size = serialized_len(op);
            let surface = self.surfaces.entry(id.clone()).or_insert_with(|| Surface {
                message_id: format!("a2ui-{}", ev.seq),
                version: info.version,
                operations: Vec::new(),
                bytes: 0,
                overflowed: false,
            });
            if surface.overflowed {
                continue;
            }
            if surface.bytes.saturating_add(size) > MAX_SURFACE_BYTES {
                surface.overflowed = true;
                overflow.get_or_insert(id);
                continue;
            }
            surface.bytes += size;
            surface.operations.push(op.clone());
            if !touched.contains(&id) {
                touched.push(id.clone());
            }
            if info.op == SurfaceOp::Delete {
                touched.retain(|t| t != &id);
                if let Some(gone) = self.surfaces.remove(&id) {
                    out.push(self.surface_activity(ev, &gone));
                }
            }
        }
        if let Some(id) = overflow {
            let message = format!(
                "surface {id:?} is larger than {MAX_SURFACE_BYTES} bytes; its later updates are not shown"
            );
            out.push(self.error_activity(ev, message));
        }
        for id in touched {
            if let Some(surface) = self.surfaces.get(&id).cloned() {
                out.push(self.surface_activity(ev, &surface));
            }
        }
        if opened {
            self.settle(ev, out);
        }
    }

    /// The user acted on a surface. Like a user message it answers a blocked thread and opens a
    /// run when none is open (the run id is the one the surface named, else `run-<seq>`); unlike
    /// one it says nothing in the transcript but a `vymalo.action` activity.
    fn on_ui_action(&mut self, ev: &Event, d: &UiActionData, out: &mut Vec<agui::Event>) {
        if self.run.is_some() {
            self.close_verifier(VerifierClose::Abandoned, out);
        }
        if matches!(self.state, ThreadState::Blocked | ThreadState::Verifying) {
            self.state = ThreadState::Queued;
        }
        self.interrupt = None;
        self.failure = None;
        self.checks_failed = false;
        if self.run.is_none() {
            let run_id = d
                .run_id
                .clone()
                .unwrap_or_else(|| format!("run-{}", ev.seq));
            self.open_run(run_id, true, out);
        }
        let mut content = Metadata::new();
        content.insert("surfaceId".to_owned(), Value::from(d.surface_id.clone()));
        content.insert("name".to_owned(), Value::from(d.name.clone()));
        content.insert(
            "sourceComponentId".to_owned(),
            Value::from(d.source_component_id.clone()),
        );
        content.insert("context".to_owned(), Value::Object(d.context.clone()));
        out.push(self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_ACTION,
            content,
            &ev.actor,
            false,
        ));
    }

    /// A source of the gate answered (ADR 0018): a `vymalo.check` activity. The card of one source
    /// in one attempt keeps its id, so a `pending` card is replaced by the answer; an answer
    /// that arrived for a verification that is no longer the current one (`stale`) is a card of
    /// its own and changes nothing else.
    fn on_check_result(&mut self, ev: &Event, d: &CheckResult, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        let id = if d.stale {
            format!("evt-{}", ev.seq)
        } else {
            // `job.sha` is the commit the agent pushed (its `branch` artifact), the same as
            // `Thread.job`. A check's `commit` is the commit the check ran on, which may differ.
            self.attempt = d.attempt.max(1);
            if d.status == orch_core::CheckStatus::Failed {
                self.checks_failed = true;
            }
            // The `completed` that started the verification says so already under a gate; a log
            // written under another gate is verified from its first answer.
            if self.state != ThreadState::Verifying && !self.state.is_terminal() {
                self.state = ThreadState::Verifying;
                out.push(self.state_snapshot());
            }
            // One card per source in one verification of one attempt: a second verification of
            // the same attempt (the user wrote, a held answer) is a card of its own, at its own
            // place in the transcript, and `replace` only ever updates within one.
            format!(
                "check-{}-{}-{}",
                d.attempt,
                self.verification.max(1),
                d.source.as_str()
            )
        };
        // The verifier is a subagent of its own for as long as its answer is awaited: it starts
        // with its pending card and ends with its verdict.
        let verifier = !d.stale && d.source == CheckSource::Verifier;
        if verifier && d.status == CheckStatus::Pending {
            self.open_verifier(out);
        }
        let content = match serde_json::to_value(d) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => Metadata::new(),
        };
        let mut snapshot = ActivitySnapshotEvent::new(id.clone(), ACTIVITY_CHECK, content);
        snapshot.replace = Some(true);
        snapshot.base.metadata = Some(actor_metadata(&ev.actor));
        self.message_ids.insert(id);
        out.push(snapshot.into());
        if verifier {
            match d.status {
                CheckStatus::Passed => {
                    self.close_verifier(VerifierClose::Verdict { passed: true }, out);
                }
                CheckStatus::Failed => {
                    self.close_verifier(VerifierClose::Verdict { passed: false }, out);
                }
                CheckStatus::Pending => {}
            }
        }
        if opened {
            self.settle(ev, out);
        }
    }

    /// The verifier starts: a subagent named after the verifier agent, with an id derived from
    /// the verification (`sub-verify-<n>`), so every replica and every replay says the same.
    fn open_verifier(&mut self, out: &mut Vec<agui::Event>) {
        if self.verifier.is_some() {
            return;
        }
        let actor = match &self.meta.gate.verifier {
            Some(agent) => Actor::agent(agent, None),
            None => Actor::system(),
        };
        let name = self
            .meta
            .gate
            .verifier
            .as_ref()
            .map_or_else(|| "verifier".to_owned(), ToString::to_string);
        let inv = Invocation {
            id: SubagentRunId::new(format!("sub-verify-{}", self.verification.max(1))),
            name,
            actor,
        };
        out.push(Self::subagent_started(&inv).into());
        self.verifier = Some(inv);
    }

    /// The verifier's invocation ends, when one is open.
    fn close_verifier(&mut self, how: VerifierClose, out: &mut Vec<agui::Event>) {
        let Some(inv) = self.verifier.take() else {
            return;
        };
        let result = match how {
            VerifierClose::Verdict { passed } => json!({"passed": passed}),
            VerifierClose::Abandoned => json!({"status": "canceled"}),
            VerifierClose::Unavailable(message) => {
                out.push(
                    SubagentErrorEvent::new(inv.id, message, Some(CODE_VERIFIER_FAILED.to_owned()))
                        .into(),
                );
                return;
            }
        };
        let mut finished = SubagentFinishedEvent::new(inv.id, None);
        finished.result = Some(result);
        out.push(finished.into());
    }

    /// A CI report (ADR 0017): a `vymalo.ci` activity, for every report, counted or not. The
    /// card is about a commit and a check, so its id is `ci-<sha>-<name>` and does not depend on
    /// anything the projector has folded: a check that runs again on the same commit replaces
    /// its card (`replace`), a report about another commit is a card of its own, and the same
    /// log projects to the same ids whoever reads it. The report changes neither the state nor
    /// the attempt (the `check_result` that follows it does, when it counts). One that arrives
    /// after the job ended opens a run of its own, like any late event, and closes it.
    fn on_ci_result(&mut self, ev: &Event, d: &CiReport, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        let id = format!("ci-{}-{}", d.sha, d.name);
        let mut content = Metadata::new();
        content.insert("name".to_owned(), Value::from(d.name.as_str()));
        content.insert("conclusion".to_owned(), Value::from(d.conclusion.as_str()));
        content.insert("passed".to_owned(), Value::Bool(d.conclusion.passes()));
        content.insert("sha".to_owned(), Value::from(d.sha.as_str()));
        content.insert(
            "shortSha".to_owned(),
            Value::from(d.sha.chars().take(SHORT_SHA_CHARS).collect::<String>()),
        );
        content.insert(
            "provider".to_owned(),
            serde_json::to_value(d.provider).unwrap_or(Value::Null),
        );
        content.insert("repository".to_owned(), Value::from(d.repository.as_str()));
        // A link is passed on only when it is `http` or `https`: a card is something to click.
        // (The webhook keeps only those already; the log is data, so this is checked again.)
        let link = d
            .url
            .as_deref()
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        for (key, value) in [
            ("branch", d.branch.as_deref()),
            ("url", link),
            ("summary", d.summary.as_deref()),
        ] {
            if let Some(value) = value {
                content.insert(key.to_owned(), Value::from(value));
            }
        }
        let mut snapshot = ActivitySnapshotEvent::new(id.clone(), ACTIVITY_CI, content);
        snapshot.replace = Some(true);
        snapshot.base.metadata = Some(actor_metadata(&ev.actor));
        self.message_ids.insert(id);
        out.push(snapshot.into());
        if opened {
            self.settle(ev, out);
        }
    }

    /// The gate failed and the agent is sent back to work: a `vymalo.rework` activity, then the
    /// invocation of the next attempt. The agent has not answered yet, but the delegation is
    /// already on its way, so the run shows it working.
    fn on_rework(&mut self, ev: &Event, d: &ReworkData, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        // The round ended at a source other than the verifier's (or before its answer).
        self.close_verifier(VerifierClose::Abandoned, out);
        self.attempt = d.attempt;
        self.sha = None;
        self.checks_failed = false;
        self.state = ThreadState::Queued;
        let id = format!("rework-{}", d.attempt);
        let content = match serde_json::to_value(d) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => Metadata::new(),
        };
        let mut snapshot = ActivitySnapshotEvent::new(id.clone(), ACTIVITY_REWORK, content);
        snapshot.replace = Some(true);
        snapshot.base.metadata = Some(actor_metadata(&ev.actor));
        self.message_ids.insert(id);
        out.push(snapshot.into());
        if self.invocation.is_none() {
            let actor = self
                .last_agent
                .clone()
                .unwrap_or_else(|| Actor::agent(&self.meta.target.agent_id, None));
            let inv = Invocation {
                id: SubagentRunId::new(format!("sub-{}", ev.seq)),
                name: actor.name.clone(),
                actor,
            };
            out.push(Self::subagent_started(&inv).into());
            self.invocation = Some(inv);
        }
        out.push(self.state_snapshot());
        if opened {
            self.settle(ev, out);
        }
    }

    /// The `a2ui-surface` snapshot of a whole surface.
    fn surface_activity(&mut self, ev: &Event, surface: &Surface) -> agui::Event {
        let mut content = Metadata::new();
        content.insert(
            A2UI_OPERATIONS_KEY.to_owned(),
            Value::Array(surface.operations.clone()),
        );
        self.message_ids.insert(surface.message_id.clone());
        let mut snapshot =
            ActivitySnapshotEvent::new(surface.message_id.clone(), ACTIVITY_A2UI_SURFACE, content);
        snapshot.replace = Some(true);
        snapshot.subagent_run_id = self.invocation.as_ref().map(|i| i.id.clone());
        snapshot.base.metadata = Some(actor_metadata(&ev.actor));
        snapshot.into()
    }

    /// A `vymalo.error` activity that changes nothing else.
    fn error_activity(&mut self, ev: &Event, message: String) -> agui::Event {
        let mut content = Metadata::new();
        content.insert("message".to_owned(), Value::from(message));
        content.insert("retryable".to_owned(), Value::from(false));
        self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_ERROR,
            content,
            &ev.actor,
            false,
        )
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
        // An `error` right after a failed check is the gate running out of attempts; any other
        // is a delivery.
        let code = if self.checks_failed && !d.retryable {
            CODE_CHECKS_FAILED
        } else {
            CODE_DELIVERY_FAILED
        };
        let failure = Failure {
            message: d.message.clone(),
            code,
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
        let was_verifying = self.state == ThreadState::Verifying;
        self.state = new;
        if self.run.is_none() {
            self.open_run(format!("run-{}", ev.seq), false, out);
        }
        match new {
            ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => {
                out.push(self.state_snapshot());
            }
            // A thread that waits while it is verified (CI or the verifier did not answer in
            // time, or the verifier could not be reached) waits for the user like any blocked
            // thread: its run ends in an interrupt the user can answer, with the reason as the
            // message, not in a delivery failure.
            ThreadState::Blocked if was_verifying && pending_error.is_some() => {
                // With no CI to wait for, the only thing that can hold a verification while the
                // verifier is out is the verifier: it failed, or it was too slow. (With CI as a
                // source too, the hold may be CI's, and the log does not say which.)
                if let Some(message) = &pending_error
                    && self.verifier.is_some()
                    && !self.meta.gate.requires(CheckSource::Ci)
                {
                    self.close_verifier(VerifierClose::Unavailable(message.clone()), out);
                }
                self.interrupt = Some(PendingInterrupt {
                    id: format!("int-{}", ev.seq),
                    reason: "input_required",
                    message: pending_error,
                    subagent: None,
                });
                self.close_run(RunClose::Interrupt, ev.seq, out);
            }
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
        if ev.actor.r#type == ActorType::Agent {
            self.last_agent = Some(ev.actor.clone());
        }
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
        // A verifier still waited for when the run ends never answered: a timeout or a failure
        // held the thread, or another source decided the round.
        let verdictless = match &close {
            RunClose::Success => VerifierClose::Verdict { passed: true },
            RunClose::Cancelled | RunClose::Interrupt | RunClose::Error(_) => {
                VerifierClose::Abandoned
            }
        };
        self.close_verifier(verdictless, out);
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
                    CODE_CHECKS_FAILED => "Checks failed",
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
            ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => return,
        };
        self.close_run(close, ev.seq, out);
    }

    // ---- frames ------------------------------------------------------------------------

    fn state_snapshot(&self) -> agui::Event {
        let target = serde_json::to_value(&self.meta.target).unwrap_or(Value::Null);
        let mut snapshot = json!({
            "thread": {
                "state": self.state.as_str(),
                "title": self.meta.title,
                "target": target,
            }
        });
        // A job with a gate says where it stands; one without says nothing more than before.
        if self.meta.gate.is_active() {
            let job = JobView::new(&self.meta.gate, self.attempt, self.sha.clone());
            snapshot["job"] = serde_json::to_value(job).unwrap_or(Value::Null);
        }
        StateSnapshotEvent::new(snapshot).into()
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
