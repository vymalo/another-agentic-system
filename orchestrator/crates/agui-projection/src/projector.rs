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
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, AgentStepData, AgentTarget,
    ArtifactData, CheckResult, CheckSource, CheckStatus, CiReport, ErrorData, Event, EventBody,
    ForkedFrom, GatePolicy, JobStartedData, JobView, MAX_SURFACE_BYTES, Recognised, ReworkData,
    StepKind, StepPhase, SurfaceOp, ThreadForkedData, ThreadId, ThreadState, ThreadTitledData,
    UiActionData, UiCatalogLedger, UiSurfaceData, UiVersion, UserId, UserMessageData, inspect,
    recognise_artifact, serialized_len,
};
use serde_json::{Value, json};

use crate::frame::{Audience, Frame};
use crate::translate::{KnownThread, ThreadView};
use crate::vocab::{
    A2UI_OPERATIONS_KEY, ACTIVITY_A2UI_SURFACE, ACTIVITY_ACTION, ACTIVITY_ARTIFACT, ACTIVITY_CHECK,
    ACTIVITY_CI, ACTIVITY_ERROR, ACTIVITY_FORK, ACTIVITY_JOB, ACTIVITY_REWORK, ACTIVITY_STATUS,
    ACTIVITY_STEP, AT_KEY, CODE_AGENT_FAILED, CODE_CHECKS_FAILED, CODE_DELIVERY_FAILED,
    CODE_STEP_FAILED, CODE_VERIFIER_FAILED, actor_metadata, problem_metadata, response_schema,
    status_content,
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

/// A step of the agent's work that has not ended (ADR 0025): what the projection says about it,
/// so every later event of the step says it again as the same activity.
#[derive(Debug, Clone)]
struct StepView {
    /// The `seq` of the first logged event of this run of the step: it names the step's activity
    /// (`step-<seq>`) and its subagent (`sub-step-<seq>`). A step that starts again after its end
    /// is another run of it, with another seq.
    seq: i64,
    /// The log's step: its id, path, kind, label, state, icon and detail as last said.
    data: AgentStepData,
    /// When the step started (RFC 3339).
    started_at: String,
    /// Who reported it.
    actor: Actor,
    /// Its subagent, for a sub-agent step.
    sub: Option<SubagentRunId>,
    /// The subagent it was started in, for a sub-agent step: said once, when it starts, and again
    /// to a client that joins while it is open.
    parent: Option<SubagentRunId>,
    /// Its `SUBAGENT_STARTED` is open in the run that is open. A suspended one is not (the run
    /// that closed it is over), and the step's later events only say its activity again.
    sub_open: bool,
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
    /// The UI catalogs the log recorded and which is current (ADR 0023), folded with the rule the
    /// core uses, so the state snapshot and the thread's ledger agree.
    catalog: UiCatalogLedger,
    /// Which job of the thread the log is in (from 1; `job_started` moves it, ADR 0020).
    job_number: u32,
    /// The attempt the agent is on (`job.attempt` of the snapshot); from the `check_result` and
    /// `rework` events, and back to 1 with a new job.
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
    /// The text of the open invocation's last final agent message: a status whose words are the
    /// same is not said twice.
    last_final: Option<String>,
    /// The steps that have not ended, by id (ADR 0025).
    steps: BTreeMap<String, StepView>,
    /// Where the thread was forked from, once its `thread_forked` has been read (ADR 0029): every
    /// `STATE_SNAPSHOT` from then on says so, as the thread's own.
    forked_from: Option<ForkedFrom>,
    /// The time of the event being applied (RFC 3339), for the frames that close what is open
    /// without an event of their own to say when.
    now: String,
}

/// The event's time, as the log writes it (RFC 3339): the `at` of every `vymalo.*` activity.
fn at_of(ev: &Event) -> Value {
    Value::from(ev.at.to_string())
}

/// The first `n` characters of a commit hash.
fn short_sha(commit: &str) -> String {
    commit.chars().take(SHORT_SHA_CHARS).collect()
}

/// `SUBAGENT_FINISHED` of a subagent that was cut short (the 1.0 outcome union has no cancelled
/// member, open question 16).
fn canceled_subagent(id: SubagentRunId) -> SubagentFinishedEvent {
    let mut finished = SubagentFinishedEvent::new(id, None);
    finished.result = Some(json!({"status": "canceled"}));
    finished
}

/// What a `vymalo.artifact` adds to the artifact as sent: its `kind` and the fields a card needs,
/// from [`recognise_artifact`]. A `branch` or `checks` artifact that cannot be used is a `file`.
fn typed_artifact(d: &ArtifactData) -> Metadata {
    let mut out = Metadata::new();
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_owned(), value);
    };
    match recognise_artifact(&d.name, d.uri.as_deref(), d.text.as_deref()) {
        Recognised::Branch(pushed) => {
            put("kind", Value::from("branch"));
            put("repository", Value::from(pushed.repository));
            put("branch", Value::from(pushed.branch));
            put("shortSha", Value::from(short_sha(&pushed.commit)));
            put("sha", Value::from(pushed.commit));
        }
        Recognised::Checks(report) => {
            put("kind", Value::from("checks"));
            put("passed", Value::from(report.passed));
            put("shortSha", Value::from(short_sha(&report.commit)));
            put("sha", Value::from(report.commit));
        }
        Recognised::PullRequest(pr) => {
            put("kind", Value::from("pull_request"));
            put("url", Value::from(pr.url));
            if let Some(number) = pr.number {
                put("number", Value::from(number));
            }
            if let Some(repository) = pr.repository {
                put("repository", Value::from(repository));
            }
            if let Some(branch) = pr.branch {
                put("branch", Value::from(branch));
            }
        }
        Recognised::Malformed { .. } | Recognised::Other => put("kind", Value::from("file")),
    }
    out
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
            catalog: UiCatalogLedger::default(),
            job_number: 1,
            attempt: 1,
            verification: 0,
            sha: None,
            checks_failed: false,
            last_agent: None,
            last_final: None,
            steps: BTreeMap::new(),
            forked_from: None,
            now: String::new(),
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

    /// The subagent run id of the agent's invocation, while one is open (`sub-<seq>`): what an
    /// agent the orchestrator asked on its behalf is started under.
    pub fn invocation_run_id(&self) -> Option<&SubagentRunId> {
        self.invocation.as_ref().map(|i| &i.id)
    }

    /// The agent's invocation, while one is open: its subagent run id, its name (the agent id) and
    /// the actor of the event that opened it. What the live words of a reply are attributed to.
    pub fn open_invocation(&self) -> Option<(&SubagentRunId, &str, &Actor)> {
        self.invocation
            .as_ref()
            .map(|i| (&i.id, i.name.as_str(), &i.actor))
    }

    /// Whether the log has already said an agent message with this id (the id the agent gave it):
    /// its words are in the transcript, so live text for it is late and is not shown.
    pub fn has_message(&self, id: &str) -> bool {
        self.texts.contains_key(id) || self.message_ids.contains(id)
    }

    /// The subagent run id of the sub-agent step `id` (`sub-step-<seq>`), while it is open and its
    /// subagent is: what the steps of an agent asked under that step are nested under. `None` for
    /// a step that is not a sub-agent step, has ended, or was suspended with its invocation.
    pub fn step_run_id(&self, id: &str) -> Option<&SubagentRunId> {
        self.steps
            .get(id)
            .filter(|step| step.sub_open)
            .and_then(|step| step.sub.as_ref())
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
        // The step subagents that are open, parents first: what a step says next is attributed
        // to one of them.
        for id in self.open_step_subagents(true) {
            if let Some(step) = self.steps.get(&id) {
                out.push(self.step_subagent_started(step).into());
            }
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
        self.now = event.at.to_string();
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
            EventBody::JobStarted(d) => self.on_job_started(event, d, &mut out),
            // The UI's catalog is not part of the transcript: the ledger moves, nothing is said,
            // and an `error` before it still explains the `thread_state` that follows.
            EventBody::UiCatalog(d) => {
                self.catalog.observe(&d.reference());
                self.pending_error = pending_error;
            }
            EventBody::AgentStep(d) => self.on_agent_step(event, d, &mut out),
            // Like the catalog, not part of the transcript; unlike it, the screen is told: the
            // title is part of every `STATE_SNAPSHOT`.
            EventBody::ThreadTitled(d) => {
                self.on_thread_titled(event, d, &mut out);
                self.pending_error = pending_error;
            }
            EventBody::ThreadForked(d) => self.on_thread_forked(event, d, &mut out),
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
        // A message on a finished thread starts the next job (ADR 0020): the run it opens is the
        // new job's, and says so from its first snapshot. The `job_started` that follows in the
        // log adds the `vymalo.job` activity.
        if self.state.is_terminal() {
            self.begin_job(self.job_number.saturating_add(1));
        }
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
        if d.is_final {
            self.last_final = Some(d.text.clone());
        }
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
        // What the agent says when it finishes or asks is its answer: an assistant message in the
        // transcript, not a detail of the status (unless it said exactly that already).
        let speaks = matches!(
            d.status,
            AgentStatus::Completed | AgentStatus::InputRequired | AgentStatus::AuthRequired
        );
        if speaks
            && let Some(detail) = d.detail.as_deref()
            && !detail.trim().is_empty()
            && self.last_final.as_deref().map(str::trim) != Some(detail.trim())
        {
            self.say_status(ev, detail, out);
        }
        let detail = if speaks { None } else { d.detail.as_deref() };
        out.push(self.activity(
            format!("evt-{}", ev.seq),
            ACTIVITY_STATUS,
            status_content(d.status, detail),
            ev,
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

    /// The words of a `completed`, `input_required` or `auth_required` status, as an assistant
    /// message of the open invocation: `TEXT_MESSAGE_START/CONTENT/END` with the id `st-<seq>`,
    /// named after the agent. A message still open is closed first.
    fn say_status(&mut self, ev: &Event, detail: &str, out: &mut Vec<agui::Event>) {
        self.close_text(out);
        let Some(inv) = self.invocation.clone() else {
            return;
        };
        let wire_id = agui::MessageId::new(format!("st-{}", ev.seq));
        self.message_ids.insert(wire_id.as_str().to_owned());
        let mut start = TextMessageStartEvent::new(wire_id.clone(), TextMessageRole::Assistant);
        start.name = Some(inv.name.clone());
        start.subagent_run_id = Some(inv.id.clone());
        start.base.metadata = Some(actor_metadata(&ev.actor));
        out.push(start.into());
        let mut content = TextMessageContentEvent::new(wire_id.clone(), detail);
        content.subagent_run_id = Some(inv.id.clone());
        out.push(content.into());
        let mut end = TextMessageEndEvent::new(wire_id);
        end.subagent_run_id = Some(inv.id);
        out.push(end.into());
    }

    fn on_artifact(&mut self, ev: &Event, d: &ArtifactData, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);
        // What is verified is what the agent had pushed when it finished: the ledger is frozen
        // while the thread is verified, and so is `job.sha`.
        if self.meta.gate.is_active()
            && self.state != ThreadState::Verifying
            && let Recognised::Branch(pushed) =
                recognise_artifact(&d.name, d.uri.as_deref(), d.text.as_deref())
        {
            self.sha = Some(pushed.commit);
        }
        let mut content = typed_artifact(d);
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
            ev,
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

    /// The thread has a new title (a person renamed it). The title is part of the thread in every
    /// `STATE_SNAPSHOT`, so the event is said as one. Inside a run: a `STATE_SNAPSHOT` with the new
    /// title. Outside any run, with the thread idle or finished: a producer-initiated run of its
    /// own that holds that snapshot and ends at once (`RUN_STARTED`, `STATE_SNAPSHOT`,
    /// `RUN_FINISHED` or what the thread's state closes a run with), which a client that has
    /// nothing else to show for it drops. Outside a run with the thread active (its run closed
    /// early): the run opens, as it does for any event of an active thread.
    fn on_thread_titled(&mut self, ev: &Event, d: &ThreadTitledData, out: &mut Vec<agui::Event>) {
        self.meta.title.clone_from(&d.title);
        if self.run.is_some() {
            out.push(self.state_snapshot());
        } else if is_active(self.state) {
            self.open_run(format!("run-{}", ev.seq), true, out);
        } else {
            self.open_run(format!("run-{}", ev.seq), false, out);
            // the closing snapshot says the new title
            self.settle(ev, out);
        }
    }

    /// The thread began as a copy of another (ADR 0029): the copied events came first, and this is
    /// where the copy ends. A fork is a finished job with its own context, so:
    ///
    /// * a run the copy left open (a cut before a message sent mid-run) is closed as a cancelled
    ///   one, with its invocations, as a `thread_state{cancelled}` would close it;
    /// * the finished job is forgotten as `job_started` forgets one (what the copy still held of
    ///   the interrupt, the surfaces, the steps, the attempt and the commit), and so is the UI
    ///   catalog: the fork's agent has been sent none (the next message that carries one sends
    ///   it in full);
    /// * the title is the parent's as it was when the fork was made, and the fork's origin joins
    ///   every `STATE_SNAPSHOT` from here on (`thread.forkedFrom`);
    /// * a producer-initiated run of its own says it: `ACTIVITY_SNAPSHOT{messageId:"fork-<seq>",
    ///   activityType:"vymalo.fork"}`, then the closing `STATE_SNAPSHOT{done}` and `RUN_FINISHED`.
    ///
    /// The job number stays: it is the newest job the copy started, and the next message of the
    /// fork starts the one after it.
    fn on_thread_forked(&mut self, ev: &Event, d: &ThreadForkedData, out: &mut Vec<agui::Event>) {
        self.state = ThreadState::Done;
        self.meta.title.clone_from(&d.title);
        self.forked_from = Some(ForkedFrom {
            thread_id: Some(d.from.thread_id),
            seq: d.from.seq,
            kind: d.kind,
        });
        if self.run.is_some() {
            self.close_run(RunClose::Cancelled, ev.seq, out);
        }
        self.forget_job();
        self.catalog = UiCatalogLedger::default();
        self.last_final = None;
        self.open_run(format!("run-{}", ev.seq), false, out);
        let mut content = Metadata::new();
        content.insert(
            "from".to_owned(),
            json!({"threadId": d.from.thread_id, "seq": d.from.seq}),
        );
        content.insert(
            "kind".to_owned(),
            serde_json::to_value(d.kind).unwrap_or(Value::Null),
        );
        content.insert("title".to_owned(), Value::from(d.title.clone()));
        content.insert(
            "target".to_owned(),
            serde_json::to_value(&d.target).unwrap_or(Value::Null),
        );
        out.push(self.activity(
            format!("fork-{}", ev.seq),
            ACTIVITY_FORK,
            content,
            ev,
            false,
        ));
        self.settle(ev, out);
    }

    /// The thread's next job started (ADR 0020). Normally the `user_message` that caused it has
    /// begun it already; a redelivered message has no such event, and the boundary alone opens
    /// the run.
    fn on_job_started(&mut self, ev: &Event, d: &JobStartedData, out: &mut Vec<agui::Event>) {
        let begun = self.job_number == d.job;
        if !begun {
            self.begin_job(d.job);
        }
        let opened = self.ensure_run(ev, out);
        let mut content = Metadata::new();
        content.insert("job".to_owned(), Value::from(d.job));
        out.push(self.activity(format!("job-{}", d.job), ACTIVITY_JOB, content, ev, false));
        if !begun && !opened {
            out.push(self.state_snapshot());
        }
    }

    /// Forgets the finished job: the new one is queued, on attempt 1, with nothing pushed and no
    /// surface of the old one to act on (an action on an old card is a 422). The verification
    /// count goes on, as the core's does.
    fn begin_job(&mut self, number: u32) {
        self.job_number = number;
        self.state = ThreadState::Queued;
        self.forget_job();
    }

    /// What a finished job leaves behind and the next one must not inherit (see
    /// [`begin_job`](Self::begin_job)); the job number and the state are the caller's.
    fn forget_job(&mut self) {
        self.attempt = 1;
        self.sha = None;
        self.checks_failed = false;
        self.interrupt = None;
        self.failure = None;
        self.suspended = None;
        self.pending_error = None;
        self.surfaces.clear();
        self.steps.clear();
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
            ev,
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
        let mut content = match serde_json::to_value(d) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => Metadata::new(),
        };
        content.insert(AT_KEY.to_owned(), at_of(ev));
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

    /// A CI report (ADR 0017): a `vymalo.ci` activity, for every report, counted or not. **Every
    /// report is a card of its own**: the id is `ci-<provider>-<sha>-<name>-<seq>`, `seq` being the
    /// report's place in the log (one per report, since the inbox stores a delivery once), and
    /// the snapshot does not `replace`. A later report about the same commit and check, however
    /// it came, cannot overwrite the evidence of an earlier one, and a forged one cannot hide a
    /// red. The same log projects to the same ids whoever reads it. The gate's verdict is not
    /// this card: it is the `vymalo.check` card of source `ci`, replaced in place as the gate
    /// decides. The report changes neither the state nor the attempt (the `check_result` that
    /// follows it does, when it counts). One that arrives after the job ended opens a run of
    /// its own, like any late event, and closes it.
    fn on_ci_result(&mut self, ev: &Event, d: &CiReport, out: &mut Vec<agui::Event>) {
        let opened = self.ensure_run(ev, out);
        let provider = serde_json::to_value(d.provider)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let id = format!("ci-{provider}-{}-{}-{}", d.sha, d.name, ev.seq);
        let mut content = Metadata::new();
        content.insert("name".to_owned(), Value::from(d.name.as_str()));
        content.insert("conclusion".to_owned(), Value::from(d.conclusion.as_str()));
        content.insert("passed".to_owned(), Value::Bool(d.conclusion.passes()));
        content.insert("sha".to_owned(), Value::from(d.sha.as_str()));
        content.insert("shortSha".to_owned(), Value::from(short_sha(&d.sha)));
        content.insert(
            "provider".to_owned(),
            serde_json::to_value(d.provider).unwrap_or(Value::Null),
        );
        content.insert("repository".to_owned(), Value::from(d.repository.as_str()));
        // A link is passed on only when it is `http` or `https`: a card is something to click.
        // (The webhook keeps only those already; the log is data, so this is checked again.)
        let link = d.url.as_deref().filter(|u| {
            // The scheme is case-insensitive (`HTTPS://` is a link, `javascript:` is not).
            let scheme_is = |scheme: &str| {
                u.get(..scheme.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
            };
            scheme_is("http://") || scheme_is("https://")
        });
        for (key, value) in [
            ("branch", d.branch.as_deref()),
            ("url", link),
            ("summary", d.summary.as_deref()),
        ] {
            if let Some(value) = value {
                content.insert(key.to_owned(), Value::from(value));
            }
        }
        content.insert(AT_KEY.to_owned(), at_of(ev));
        let mut snapshot = ActivitySnapshotEvent::new(id.clone(), ACTIVITY_CI, content);
        snapshot.replace = Some(false);
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
        // Two jobs can each be sent back for attempt 2: from job 2 the id says which job.
        let id = if self.job_number > 1 {
            format!("rework-j{}-{}", self.job_number, d.attempt)
        } else {
            format!("rework-{}", d.attempt)
        };
        let mut content = match serde_json::to_value(d) {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => Metadata::new(),
        };
        content.insert(AT_KEY.to_owned(), at_of(ev));
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
            self.last_final = None;
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
            ev,
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
            ev,
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

    // ---- steps -------------------------------------------------------------------------

    /// A step of the agent's work (ADR 0025, `steps/v1`): its `vymalo.step` activity, said again
    /// for every event of the step with the same id, and, for a sub-agent step, a subagent of its
    /// own that its children are attributed to.
    ///
    /// The enclosing subagent of a step is the nearest ancestor of its path whose subagent is
    /// open, else the agent's invocation. A sub-agent step's activity is attributed to its
    /// *enclosing* subagent (it is the step of the one that runs it); its children carry the
    /// step's own.
    fn on_agent_step(&mut self, ev: &Event, d: &AgentStepData, out: &mut Vec<agui::Event>) {
        // A step is the sign that the agent works.
        let moved_to_working = self.state != ThreadState::Working;
        self.state = ThreadState::Working;
        let opened = self.ensure_run(ev, out);
        self.ensure_invocation(ev, out);

        let continues = d.phase != StepPhase::Start && self.steps.contains_key(&d.id);
        if continues {
            if let Some(step) = self.steps.get_mut(&d.id) {
                // The icon is what the step first said when a later report leaves it out.
                let icon = d.icon.clone().or_else(|| step.data.icon.clone());
                step.data = AgentStepData { icon, ..d.clone() };
                step.actor = ev.actor.clone();
            }
        } else {
            // A step that starts again, or one the projection never saw start (it ended in one
            // event, or its start was in a run this projection did not keep): a new run of it.
            if let Some(stale) = self.steps.remove(&d.id) {
                self.finish_step_subagent(&stale, out);
            }
            let sub = (d.kind == StepKind::Subagent && !d.state.is_end())
                .then(|| SubagentRunId::new(format!("sub-step-{}", ev.seq)));
            let mut step = StepView {
                seq: ev.seq,
                data: d.clone(),
                started_at: ev.at.to_string(),
                actor: ev.actor.clone(),
                sub,
                parent: None,
                sub_open: false,
            };
            if step.sub.is_some() {
                step.parent = self.enclosing_run(&d.path);
                out.push(self.step_subagent_started(&step).into());
                step.sub_open = true;
            }
            self.steps.insert(d.id.clone(), step);
        }
        out.extend(self.step_activity(&d.id));

        if d.state.is_end()
            && let Some(step) = self.steps.get(&d.id).cloned()
        {
            if step.sub_open {
                // Whatever still runs under it ends first, deepest first: nesting stays whole.
                for id in self.open_step_subagents_under(&d.id) {
                    if let Some(child) = self.steps.get_mut(&id) {
                        child.sub_open = false;
                        if let Some(sub) = child.sub.clone() {
                            out.push(canceled_subagent(sub).into());
                        }
                    }
                }
                if let Some(sub) = step.sub.clone() {
                    out.push(match d.state {
                        orch_core::StepState::Failed => SubagentErrorEvent::new(
                            sub,
                            d.detail
                                .clone()
                                .unwrap_or_else(|| format!("{} failed", d.label)),
                            Some(CODE_STEP_FAILED.to_owned()),
                        )
                        .into(),
                        orch_core::StepState::Canceled => canceled_subagent(sub).into(),
                        orch_core::StepState::Running
                        | orch_core::StepState::Waiting
                        | orch_core::StepState::Completed => {
                            SubagentFinishedEvent::new(sub, None).into()
                        }
                    });
                }
            }
            self.steps.remove(&d.id);
        }
        if moved_to_working && !opened {
            out.push(self.state_snapshot());
        }
    }

    /// The subagent a step with `path` is attributed to: the nearest ancestor whose subagent is
    /// open, else the agent's invocation.
    fn enclosing_run(&self, path: &[String]) -> Option<SubagentRunId> {
        path.iter()
            .rev()
            .filter_map(|id| self.steps.get(id))
            .find(|step| step.sub_open)
            .and_then(|step| step.sub.clone())
            .or_else(|| self.invocation.as_ref().map(|i| i.id.clone()))
    }

    /// The `vymalo.step` snapshot of the step `id` as it stands, at the time of the event being
    /// applied.
    fn step_activity(&mut self, id: &str) -> Option<agui::Event> {
        let enclosing = self.step_attribution(id);
        let step = self.steps.get(id)?;
        let d = &step.data;
        let mut content = Metadata::new();
        content.insert("id".to_owned(), Value::from(d.id.clone()));
        content.insert("path".to_owned(), json!(d.path));
        content.insert("kind".to_owned(), Value::from(d.kind.as_str()));
        content.insert("label".to_owned(), Value::from(d.label.clone()));
        content.insert("state".to_owned(), Value::from(d.state.as_str()));
        if let Some(icon) = &d.icon {
            content.insert("icon".to_owned(), Value::from(icon.clone()));
        }
        if let Some(detail) = &d.detail {
            content.insert("detail".to_owned(), Value::from(detail.clone()));
        }
        content.insert("startedAt".to_owned(), Value::from(step.started_at.clone()));
        content.insert(AT_KEY.to_owned(), Value::from(self.now.clone()));
        let message_id = format!("step-{}", step.seq);
        let mut snapshot = ActivitySnapshotEvent::new(message_id.clone(), ACTIVITY_STEP, content);
        snapshot.replace = Some(true);
        snapshot.subagent_run_id = enclosing;
        snapshot.base.metadata = Some(actor_metadata(&step.actor));
        self.message_ids.insert(message_id);
        Some(snapshot.into())
    }

    /// The subagent the activity of the step `id` is attributed to.
    fn step_attribution(&self, id: &str) -> Option<SubagentRunId> {
        let path = self.steps.get(id).map(|s| s.data.path.clone())?;
        self.enclosing_run(&path)
    }

    /// `SUBAGENT_STARTED` of a step's subagent, under the subagent that encloses it.
    fn step_subagent_started(&self, step: &StepView) -> SubagentStartedEvent {
        let sub = step
            .sub
            .clone()
            .unwrap_or_else(|| SubagentRunId::new(format!("sub-step-{}", step.seq)));
        let mut started = SubagentStartedEvent::new(sub, step.data.label.clone());
        started.parent_subagent_run_id = step.parent.clone();
        started.base.metadata = Some(actor_metadata(&step.actor));
        started
    }

    /// The ids of the steps whose subagent is open, deepest first (`parents_first` false) or
    /// outermost first, in the order they started.
    fn open_step_subagents(&self, parents_first: bool) -> Vec<String> {
        let mut open: Vec<&StepView> = self.steps.values().filter(|s| s.sub_open).collect();
        open.sort_by_key(|s| (s.data.path.len(), s.seq));
        if !parents_first {
            open.reverse();
        }
        open.into_iter().map(|s| s.data.id.clone()).collect()
    }

    /// The open step subagents that run under the step `id`, deepest first.
    fn open_step_subagents_under(&self, id: &str) -> Vec<String> {
        self.open_step_subagents(false)
            .into_iter()
            .filter(|other| {
                self.steps
                    .get(other)
                    .is_some_and(|s| s.data.path.iter().any(|p| p == id))
            })
            .collect()
    }

    /// Ends the subagent of a step that is replaced by a new run of itself.
    fn finish_step_subagent(&self, step: &StepView, out: &mut Vec<agui::Event>) {
        if step.sub_open
            && let Some(sub) = step.sub.clone()
        {
            out.push(canceled_subagent(sub).into());
        }
    }

    /// The invocation is closing: what it has open ends first, deepest first. When it suspends,
    /// the step subagents suspend with it (and are not opened again: what the steps say after the
    /// answer only says their activities again). Otherwise every step that has not ended is
    /// canceled, as a snapshot (so no spinner stays) and, for a sub-agent step, as the end of its
    /// subagent; the core forgot them when the task ended.
    fn close_steps(&mut self, how: &InvocationClose, out: &mut Vec<agui::Event>) {
        match how {
            InvocationClose::Suspended(_) => {
                for id in self.open_step_subagents(false) {
                    if let Some(step) = self.steps.get_mut(&id) {
                        step.sub_open = false;
                        if let Some(sub) = step.sub.clone() {
                            out.push(
                                SubagentFinishedEvent::new(
                                    sub,
                                    Some(SubagentFinishedOutcome::Suspended {
                                        interrupt_ids: None,
                                    }),
                                )
                                .into(),
                            );
                        }
                    }
                }
            }
            InvocationClose::Finished | InvocationClose::Canceled | InvocationClose::Error(_) => {
                let mut ids: Vec<&StepView> = self.steps.values().collect();
                ids.sort_by_key(|s| (s.data.path.len(), s.seq));
                let ids: Vec<String> = ids.into_iter().rev().map(|s| s.data.id.clone()).collect();
                for id in ids {
                    let Some(step) = self.steps.get_mut(&id) else {
                        continue;
                    };
                    step.data.state = orch_core::StepState::Canceled;
                    out.extend(self.step_activity(&id));
                    if let Some(step) = self.steps.get_mut(&id) {
                        let was_open = std::mem::replace(&mut step.sub_open, false);
                        if let (true, Some(sub)) = (was_open, step.sub.clone()) {
                            out.push(canceled_subagent(sub).into());
                        }
                    }
                }
                self.steps.clear();
            }
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
        self.last_final = None;
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
        // What the invocation still has open ends with it, before it does.
        if self.invocation.is_some() {
            self.close_steps(&how, out);
        }
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
        // The first job says nothing more than before; a later one says which it is.
        if self.job_number > 1 {
            snapshot["thread"]["jobNumber"] = Value::from(self.job_number);
        }
        // A thread that was forked says where from (ADR 0029), in the shape of `Thread.forkedFrom`.
        if let Some(from) = &self.forked_from {
            snapshot["thread"]["forkedFrom"] = serde_json::to_value(from).unwrap_or(Value::Null);
        }
        // The catalog the agent is told to use, when the thread has one (ADR 0023): a screen
        // compares it with its own to decide whether to send its catalog with the next run.
        if let Some(current) = self.catalog.current() {
            snapshot["thread"]["uiCatalog"] = serde_json::to_value(current).unwrap_or(Value::Null);
        }
        // A job with a gate says where it stands; one without says nothing more than before.
        if self.meta.gate.is_active() {
            let job = JobView {
                number: self.job_number,
                ..JobView::new(&self.meta.gate, self.attempt, self.sha.clone())
            };
            snapshot["job"] = serde_json::to_value(job).unwrap_or(Value::Null);
        }
        StateSnapshotEvent::new(snapshot).into()
    }

    /// An `ACTIVITY_SNAPSHOT`, attributed to the open invocation when `attribute`. The activity
    /// is a message of its own, so its id joins the ids the thread already holds.
    ///
    /// Every `vymalo.*` activity says when its event happened (`at`, the event's time), so a
    /// client can show it without keeping the log.
    fn activity(
        &mut self,
        id: String,
        activity_type: &str,
        mut content: Metadata,
        ev: &Event,
        attribute: bool,
    ) -> agui::Event {
        self.message_ids.insert(id.clone());
        content.insert(AT_KEY.to_owned(), at_of(ev));
        let mut snapshot = ActivitySnapshotEvent::new(id, activity_type, content);
        if attribute {
            snapshot.subagent_run_id = self.invocation.as_ref().map(|i| i.id.clone());
        }
        snapshot.base.metadata = Some(actor_metadata(&ev.actor));
        snapshot.into()
    }
}
