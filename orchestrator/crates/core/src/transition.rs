//! The pure state machine: `(state, input) -> (next state, commands)`.
//!
//! Every `match` over [`ThreadState`], [`Input`], [`AgentUpdate`] and
//! [`AgentTaskState`] is exhaustive with no wildcard arm, so adding a variant
//! makes the compiler point at every decision that must be revisited (ADR 0004).
//!
//! Rule for `thread_state` events: one is appended only when the thread *enters*
//! `blocked`, `done`, `failed` or `cancelled`. Entering `queued`/`working` is implied
//! by `user_message` / `agent_status` and visible through `Thread.state`; entering
//! `verifying` by the `check_result` events that follow the agent's `completed`.
//!
//! The function takes and returns a [`Snapshot`]: the state and the job ledger. Under a gate
//! with no required source (the default) the job is never touched and every decision is the one
//! made before the gate existed. Under an active gate the agent finishing is not enough: see
//! [`crate::verify`] for the rules and ADR 0018 for the reasoning.

use jiff::SignedDuration;

use crate::agent::{AgentTaskState, AgentUpdate};
use crate::error::{Classify, ErrorClass};
use crate::event::{
    Actor, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData, EventBody,
    JobStartedData, Origin, ThreadStateData, UserMessageData,
};
use crate::gate::{
    CheckResult, CheckSource, CheckStatus, CiReport, Hold, Job, MAX_SUMMARY_BYTES, PushedRef,
    Recognised, Snapshot, Timer, Verdict, WatchKey, add_task_message, cap_findings,
    recognise_artifact, repo_key, truncate_to,
};
use crate::ids::{AgentId, UserId};
use crate::step::{StepReport, StepSource, record_step};
use crate::thread::ThreadState;
use crate::ui::{UiActionData, UiSurfaceData, check_operation_list};
use crate::ui_catalog::{UiCatalogData, UiDelivery};
use crate::verify;

/// Everything that can happen to a thread, already translated to protocol-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The user wrote a message (first message, follow-up, answer to a blocked job, or the next
    /// request on a finished thread, which starts the thread's next job: ADR 0020).
    UserMessage {
        /// Author.
        user: UserId,
        /// Text.
        text: String,
        /// The id the surface gave the message (an AG-UI message id), recorded in the log.
        message_id: Option<String>,
        /// The id of the run the surface started or continued with it, recorded in the log.
        run_id: Option<String>,
        /// The surface the message came in through, recorded in the log (ADR 0019).
        origin: Origin,
        /// The UI catalog the person's screen sent with it, when it sent one (ADR 0023): the
        /// caller has checked it ([`UiCatalogData::from_json`]). The thread records a digest it
        /// has not seen, and the agent is sent it when it becomes the current catalog.
        catalog: Option<UiCatalogData>,
    },
    /// The user acted on an A2UI surface (a button with an event action). Like a message, it
    /// answers a blocked thread and is delegated to the agent; unlike one it carries no text.
    /// The caller has checked that the thread has the surface ([`UiActionData::check`] checks
    /// the sizes).
    UiAction {
        /// Who acted.
        user: UserId,
        /// What they did.
        action: UiActionData,
        /// The UI catalog the person's screen sent with it, when it sent one (as for
        /// [`Input::UserMessage`]).
        catalog: Option<UiCatalogData>,
    },
    /// A user message that is in the log already but whose delegation never reached the agent
    /// (its outbox row was claimed after the job ended). The dispatcher builds it so that the
    /// message is not lost (ADR 0020): on a `done` or `failed` thread it starts the next job for
    /// `text` (the `user_message` event is in the log, so none is appended); on a `cancelled` one
    /// it changes nothing, because the person asked to stop; on an open thread it delegates.
    Redeliver {
        /// The message text.
        text: String,
    },
    /// The user asked to cancel.
    Cancel {
        /// Who asked.
        user: UserId,
    },
    /// The delegated agent reported something.
    Agent {
        /// Which agent.
        agent: AgentId,
        /// The revision that produced it, when known.
        revision: Option<String>,
        /// What it reported.
        update: AgentUpdate,
    },
    /// A step of the work that does not come from the delegated agent's own report: a tool call
    /// the orchestrator relays for it (ADR 0024), an agent it asked for (ADR 0026). It goes
    /// through the same rules as an agent's step ([`record_step`]: coalesced, bounded, nested
    /// under its parent), by `actor`, and may name an MCP server as its icon. A thread that is
    /// finished takes none ([`TransitionError::InvalidInState`]).
    Step {
        /// Who the step is attributed to (the agent whose call it is, or the orchestrator).
        actor: Actor,
        /// What happened.
        report: StepReport,
    },
    /// A delegation could not be delivered (dead-lettered outbox row).
    DeliveryFailed {
        /// Why.
        reason: String,
        /// Whether trying again (a new message) can help.
        retryable: bool,
    },
    /// A cancel arrived before any delegation reached the agent.
    CancelledBeforeStart,
    /// The agent refused to cancel (or cancelling failed for good).
    CancelRejected {
        /// Why.
        reason: String,
        /// Whether retrying can help.
        retryable: bool,
    },
    /// A CI provider reported a completed check on some commit (a webhook, already normalised
    /// and matched to this thread by its watch key). The core decides whether it is about the
    /// commit the agent pushed.
    CiReported(CiReport),
    /// The verifier answered the request made for `attempt` in `verification`. The dispatcher builds it
    /// from the verifier's `verdict` artifact, or from its absence (`passed: false`, the
    /// finding "no verdict").
    VerifierReported {
        /// The attempt the verification was requested in.
        attempt: u32,
        /// The verification it was requested in.
        verification: u32,
        /// The verdict.
        verdict: Verdict,
    },
    /// The verifier could not be used for the request made for `attempt` in `verification`: its
    /// agent is gone from the configuration, cannot be reached, refused the request or ended its
    /// task without answering. The thread waits for the user ([`Hold::VerifierFailed`]) and no
    /// attempt is spent: the verifier being down is not the code's fault. The dispatcher builds
    /// it when it gives up on a `verify` row; one for a verification that is over changes
    /// nothing.
    VerifierFailed {
        /// The attempt the verification was requested in.
        attempt: u32,
        /// The verification it was requested in.
        verification: u32,
        /// Why, worded for the people who see the thread (no transport detail, no secret).
        reason: String,
    },
    /// A deadline armed by [`Command::Schedule`] passed.
    TimerFired(Timer),
}

impl Input {
    /// A short static name, for errors and logs.
    pub fn name(&self) -> &'static str {
        match self {
            Input::UserMessage { .. } => "user message",
            Input::UiAction { .. } => "ui action",
            Input::Redeliver { .. } => "redelivery",
            Input::Cancel { .. } => "cancel",
            Input::Agent { .. } => "agent update",
            Input::Step { .. } => "step",
            Input::DeliveryFailed { .. } => "delivery failure",
            Input::CancelledBeforeStart => "cancelled before start",
            Input::CancelRejected { .. } => "cancel rejection",
            Input::CiReported(_) => "ci report",
            Input::VerifierReported { .. } => "verifier report",
            Input::VerifierFailed { .. } => "verifier failure",
            Input::TimerFired(_) => "timer",
        }
    }
}

/// An event to append; the application stamps `at`, the store assigns `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    /// Who produced it.
    pub actor: Actor,
    /// The payload.
    pub body: EventBody,
}

/// What the application must do as a consequence of a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Append an event to the thread's log.
    Append(EventDraft),
    /// Delegate this text to the target agent (outbox kind `delegate`).
    Delegate {
        /// The user's text.
        text: String,
        /// What to tell the agent of the person's UI catalog, if the thread has one (ADR 0023).
        catalog: Option<UiDelivery>,
    },
    /// Delegate a user's action on an A2UI surface to the target agent (outbox kind `delegate`).
    DelegateAction {
        /// The action.
        action: UiActionData,
        /// What to tell the agent of the person's UI catalog, if the thread has one (ADR 0023).
        catalog: Option<UiDelivery>,
    },
    /// Ask the agent to cancel the running task (outbox kind `cancel`). `job` is the number of
    /// the job the person asked to stop: a row claimed after that job ended and the next began
    /// is finished without touching the agent (ADR 0020).
    RequestCancel {
        /// The job to cancel.
        job: u32,
    },
    /// Route inbound CI reports for this key to the thread (a row of the `watches` table,
    /// ADR 0016). Idempotent.
    ///
    /// The application does not execute it yet: the inbox arrives with slice 5 of the MVP plan.
    Watch {
        /// `ci:<repo-key>@<sha>`.
        key: WatchKey,
    },
    /// Feed `timer` back as [`Input::TimerFired`] once `after` has passed (an inbox row with
    /// `source = 'timer'`, ADR 0016).
    ///
    /// The application does not execute it yet: timers arrive with slice 5 of the MVP plan.
    Schedule {
        /// How long from now.
        after: SignedDuration,
        /// What to feed back.
        timer: Timer,
    },
    /// Ask `verifier` to review `pushed` (outbox kind `verify`, ADR 0018). The dispatcher
    /// answers with exactly one [`Input::VerifierReported`] for this `attempt` and
    /// `verification`, or with [`Input::VerifierFailed`] when it cannot get an answer.
    RequestVerification {
        /// The attempt it belongs to.
        attempt: u32,
        /// The verification it belongs to.
        verification: u32,
        /// The agent that reviews.
        verifier: AgentId,
        /// What to review.
        pushed: PushedRef,
        /// The prompt, written by the core.
        text: String,
    },
}

/// An input that is not valid in the current state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransitionError {
    /// An action on a finished job: its surface belongs to a request that ended, and a person who
    /// wants something else writes a message (which starts the next job, ADR 0020).
    #[error("thread is finished ({state:?})")]
    Finished {
        /// The terminal state.
        state: ThreadState,
    },
    /// A late or replayed input for a state that cannot take it.
    #[error("{input} is not valid in state {state:?}")]
    InvalidInState {
        /// Current state.
        state: ThreadState,
        /// The input's name.
        input: &'static str,
    },
}

impl Classify for TransitionError {
    fn class(&self) -> ErrorClass {
        match self {
            TransitionError::Finished { .. } | TransitionError::InvalidInState { .. } => {
                ErrorClass::Rejected
            }
        }
    }
}

/// The `error` event for an A2UI part that was refused.
fn refused_ui(reason: &str) -> EventBody {
    EventBody::Error(ErrorData {
        message: format!("an A2UI part from the agent was refused: {reason}"),
        retryable: false,
    })
}

pub(crate) fn append(actor: Actor, body: EventBody) -> Command {
    Command::Append(EventDraft { actor, body })
}

pub(crate) fn entered(state: ThreadState) -> Command {
    append(
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

fn error_event(message: &str, retryable: bool) -> Command {
    append(
        Actor::system(),
        EventBody::Error(ErrorData {
            message: message.to_owned(),
            retryable,
        }),
    )
}

fn agent_status(actor: Actor, status: AgentStatus, detail: Option<String>) -> Command {
    append(
        actor,
        EventBody::AgentStatus(AgentStatusData { status, detail }),
    )
}

fn prefixed(prefix: &str, detail: &Option<String>) -> String {
    match detail {
        Some(d) if !d.is_empty() => format!("{prefix}: {d}"),
        Some(_) | None => prefix.to_owned(),
    }
}

/// The commands that record a catalog the input carried (first in the commit, before the message
/// or the action it came with) and say what the agent is sent of it, as the thread's ledger
/// decides ([`UiCatalogLedger::accept`]). One rule for every branch that delegates.
fn deliver(
    job: &mut Job,
    user: &UserId,
    carried: Option<&UiCatalogData>,
) -> (Option<Command>, Option<UiDelivery>) {
    let accepted = job.catalog.accept(carried);
    let record = carried
        .filter(|_| accepted.record)
        .map(|catalog| append(Actor::user(user), EventBody::UiCatalog(catalog.clone())));
    (record, accepted.delivery)
}

fn user_message(
    job: &mut Job,
    user: &UserId,
    text: &str,
    message_id: &Option<String>,
    run_id: &Option<String>,
    origin: Origin,
    catalog: &Option<UiCatalogData>,
) -> Vec<Command> {
    let (record, delivery) = deliver(job, user, catalog.as_ref());
    let mut cmds: Vec<Command> = record.into_iter().collect();
    cmds.push(append(
        Actor::user(user),
        EventBody::UserMessage(UserMessageData {
            text: text.to_owned(),
            message_id: message_id.clone(),
            run_id: run_id.clone(),
            origin,
        }),
    ));
    cmds.push(Command::Delegate {
        text: text.to_owned(),
        catalog: delivery,
    });
    cmds
}

fn job_started(job: &Job) -> Command {
    append(
        Actor::system(),
        EventBody::JobStarted(JobStartedData { job: job.number }),
    )
}

/// The delegation of a message that is in the log already: the screen's catalog is the thread's
/// current one, since the message carries none of its own.
fn redelegate(job: &Job, text: &str) -> Command {
    Command::Delegate {
        text: text.to_owned(),
        catalog: job.catalog.redelivery(),
    }
}

fn ui_action(
    job: &mut Job,
    user: &UserId,
    action: &UiActionData,
    catalog: &Option<UiCatalogData>,
) -> Vec<Command> {
    let (record, delivery) = deliver(job, user, catalog.as_ref());
    let mut cmds: Vec<Command> = record.into_iter().collect();
    cmds.push(append(
        Actor::user(user),
        EventBody::UiAction(action.clone()),
    ));
    cmds.push(Command::DelegateAction {
        action: action.clone(),
        catalog: delivery,
    });
    cmds
}

/// Decides the next state, job and commands for `input` in `snapshot`. Pure: no I/O, no clock.
pub fn transition(
    snapshot: &Snapshot,
    input: &Input,
) -> Result<(Snapshot, Vec<Command>), TransitionError> {
    let mut job = snapshot.job.clone();
    let (state, commands) = decide(snapshot.state, &mut job, input)?;
    // A hold explains a `blocked` thread; a thread that is anything else has none.
    match state {
        ThreadState::Blocked => {}
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Verifying
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => job.hold = None,
    }
    Ok((Snapshot { state, job }, commands))
}

/// Most bytes of a verifier failure's reason that reach the log.
const MAX_HOLD_REASON_BYTES: usize = 512;

/// Keeps what the person wrote, for the prompts the core writes (the rework and the verifier's):
/// every user message of the job in order, capped (`add_task_message`). Under an active gate
/// only: a job with no gate never has a ledger to keep it in, and never needs one.
fn note_task(job: &mut Job, text: &str) {
    if job.gate.is_active() {
        job.task = add_task_message(job.task.as_deref(), text);
    }
}

/// Keeps what the agent said about its work, for the verifier's prompt: only under a gate that
/// requires the verifier (nothing else reads it), capped, the latest word replacing the earlier.
fn note_summary(job: &mut Job, text: &str) {
    let text = text.trim();
    if job.gate.requires(CheckSource::Verifier) && !text.is_empty() {
        job.summary = Some(truncate_to(text, MAX_SUMMARY_BYTES).to_owned());
    }
}

fn decide(
    state: ThreadState,
    job: &mut Job,
    input: &Input,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    match input {
        Input::UserMessage {
            user,
            text,
            message_id,
            run_id,
            origin,
            catalog,
        } => match state {
            ThreadState::Queued | ThreadState::Working => {
                note_task(job, text);
                Ok((
                    state,
                    user_message(job, user, text, message_id, run_id, *origin, catalog),
                ))
            }
            // Blocked, or being verified: the user's message re-delegates. It does not use an
            // attempt: an attempt is used only when the gate fails.
            ThreadState::Blocked | ThreadState::Verifying => {
                note_task(job, text);
                job.hold = None;
                Ok((
                    ThreadState::Queued,
                    user_message(job, user, text, message_id, run_id, *origin, catalog),
                ))
            }
            // The thread is a conversation (ADR 0020): the next message is the next job, on the
            // same agent and under the same gate, whatever state the last one ended in. The next
            // job keeps the catalogs the conversation has seen.
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                *job = job.next();
                note_task(job, text);
                let mut cmds = user_message(job, user, text, message_id, run_id, *origin, catalog);
                // the boundary comes right after the message, before its delegation
                cmds.insert(cmds.len().saturating_sub(1), job_started(job));
                Ok((ThreadState::Queued, cmds))
            }
        },
        Input::Redeliver { text } => match state {
            // The thread moved on while the message waited (an earlier redelivery started the next
            // job): the message joins that job, as one written during it would, and is sent
            // after what that job has been told, so it may reach the agent out of the order it
            // was written in (open question 33).
            ThreadState::Queued | ThreadState::Working => {
                note_task(job, text);
                Ok((state, vec![redelegate(job, text)]))
            }
            ThreadState::Blocked | ThreadState::Verifying => {
                note_task(job, text);
                job.hold = None;
                Ok((ThreadState::Queued, vec![redelegate(job, text)]))
            }
            ThreadState::Done | ThreadState::Failed => {
                *job = job.next();
                note_task(job, text);
                Ok((
                    ThreadState::Queued,
                    vec![job_started(job), redelegate(job, text)],
                ))
            }
            // The person asked to stop: a message they wrote before that stays undelivered.
            ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::UiAction {
            user,
            action,
            catalog,
        } => match state {
            ThreadState::Queued | ThreadState::Working => {
                Ok((state, ui_action(job, user, action, catalog)))
            }
            ThreadState::Blocked | ThreadState::Verifying => {
                job.hold = None;
                Ok((ThreadState::Queued, ui_action(job, user, action, catalog)))
            }
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                Err(TransitionError::Finished { state })
            }
        },
        Input::Cancel { .. } => match state {
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                Ok((state, vec![Command::RequestCancel { job: job.number }]))
            }
            // The agent's task is over, so there is nothing to ask it to cancel: the thread is
            // cancelled at once.
            ThreadState::Verifying => Ok((
                ThreadState::Cancelled,
                vec![entered(ThreadState::Cancelled)],
            )),
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::Agent {
            agent,
            revision,
            update,
        } => agent_input(
            state,
            job,
            Actor::agent(agent, revision.clone()),
            update,
            input,
        ),
        Input::Step { actor, report } => match state {
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                Err(TransitionError::InvalidInState {
                    state,
                    input: input.name(),
                })
            }
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Blocked
            | ThreadState::Verifying => Ok(record_step(
                state,
                job,
                actor.clone(),
                report,
                StepSource::Orchestrator,
            )),
        },
        Input::DeliveryFailed { reason, retryable } => match state {
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Blocked
            | ThreadState::Verifying => {
                if *retryable {
                    let mut cmds = vec![error_event(reason, true)];
                    match state {
                        ThreadState::Blocked => {}
                        ThreadState::Queued | ThreadState::Working => {
                            cmds.push(entered(ThreadState::Blocked));
                        }
                        // The verifier could not be reached.
                        ThreadState::Verifying => {
                            job.hold = Some(Hold::VerifierFailed);
                            cmds.push(entered(ThreadState::Blocked));
                        }
                        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {}
                    }
                    Ok((ThreadState::Blocked, cmds))
                } else {
                    Ok((
                        ThreadState::Failed,
                        vec![error_event(reason, false), entered(ThreadState::Failed)],
                    ))
                }
            }
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                Ok((state, vec![error_event(reason, false)]))
            }
        },
        Input::CancelledBeforeStart => match state {
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Blocked
            | ThreadState::Verifying => Ok((
                ThreadState::Cancelled,
                vec![entered(ThreadState::Cancelled)],
            )),
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::CancelRejected { reason, retryable } => match state {
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Blocked
            | ThreadState::Verifying => Ok((state, vec![error_event(reason, *retryable)])),
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::CiReported(report) => Ok(ci_reported(state, job, report)),
        Input::VerifierReported {
            attempt,
            verification,
            verdict,
        } => Ok(verifier_reported(
            state,
            job,
            *attempt,
            *verification,
            verdict,
        )),
        Input::VerifierFailed {
            attempt,
            verification,
            reason,
        } => Ok(verifier_failed(state, job, *attempt, *verification, reason)),
        Input::TimerFired(timer) => Ok(timer_fired(state, job, *timer)),
    }
}

fn agent_input(
    state: ThreadState,
    job: &mut Job,
    actor: Actor,
    update: &AgentUpdate,
    input: &Input,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    match state {
        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
            return Err(TransitionError::InvalidInState {
                state,
                input: input.name(),
            });
        }
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Verifying
        | ThreadState::Blocked => {}
    }
    match update {
        AgentUpdate::Artifact {
            name,
            mime_type,
            uri,
            text,
        } => {
            let mut cmds = vec![append(
                actor,
                EventBody::Artifact(ArtifactData {
                    name: name.clone(),
                    mime_type: mime_type.clone(),
                    uri: uri.clone(),
                    text: text.clone(),
                }),
            )];
            // While the work is being verified the ledger is frozen: what is checked is what
            // the agent had pushed when it finished.
            match state {
                ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                    if job.gate.is_active() {
                        cmds.extend(note_artifact(job, name, uri.as_deref(), text.as_deref()));
                    }
                }
                ThreadState::Verifying
                | ThreadState::Done
                | ThreadState::Failed
                | ThreadState::Cancelled => {}
            }
            Ok((state, cmds))
        }
        // The adapter has checked the payload; the door is checked again here, so nothing
        // unchecked reaches the log whatever adapter sent it (ADR 0013).
        AgentUpdate::Ui { operations } => Ok((
            state,
            vec![append(
                actor,
                match check_operation_list(operations) {
                    Ok(()) => EventBody::UiSurface(UiSurfaceData {
                        operations: operations.clone(),
                    }),
                    Err(rejection) => refused_ui(&rejection.to_string()),
                },
            )],
        )),
        AgentUpdate::UiRejected { reason } => Ok((state, vec![append(actor, refused_ui(reason))])),
        AgentUpdate::Step(report) => Ok(record_step(state, job, actor, report, StepSource::Agent)),
        AgentUpdate::Message {
            message_id,
            text,
            is_final,
        } => {
            // What the agent says about its work is what the verifier is shown (as data), until
            // the work is being verified: the ledger is frozen then.
            match state {
                ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                    if *is_final {
                        note_summary(job, text);
                    }
                }
                ThreadState::Verifying
                | ThreadState::Done
                | ThreadState::Failed
                | ThreadState::Cancelled => {}
            }
            Ok((
                state,
                vec![append(
                    actor,
                    EventBody::AgentMessage(AgentMessageData {
                        text: text.clone(),
                        message_id: message_id.clone(),
                        is_final: *is_final,
                    }),
                )],
            ))
        }
        AgentUpdate::Status {
            state: task,
            detail,
        } => status_input(state, job, actor, *task, detail),
    }
}

/// The agent waits for the user (input or authentication): the thread blocks, and a repeat of
/// the same wait only shows again when it has something new to say.
fn blocked(
    state: ThreadState,
    status: Command,
    detail: &Option<String>,
) -> (ThreadState, Vec<Command>) {
    match state {
        ThreadState::Queued | ThreadState::Working => (
            ThreadState::Blocked,
            vec![status, entered(ThreadState::Blocked)],
        ),
        ThreadState::Blocked => match detail {
            Some(_) => (ThreadState::Blocked, vec![status]),
            None => (ThreadState::Blocked, vec![]),
        },
        // The agent's task is over; a late wait changes nothing.
        ThreadState::Verifying
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => (state, vec![]),
    }
}

fn status_input(
    state: ThreadState,
    job: &mut Job,
    actor: Actor,
    task: AgentTaskState,
    detail: &Option<String>,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    // The task is over: whatever step it left open is not going on (the projection closes what it
    // shows; the log gets no event for it).
    if task.is_terminal() {
        job.steps.close_all();
    }
    match task {
        AgentTaskState::Submitted => Ok((state, vec![])),
        AgentTaskState::Working => {
            let cmd = agent_status(actor, AgentStatus::Working, detail.clone());
            match state {
                ThreadState::Queued | ThreadState::Blocked => Ok((ThreadState::Working, vec![cmd])),
                ThreadState::Working => match detail {
                    Some(_) => Ok((ThreadState::Working, vec![cmd])),
                    None => Ok((ThreadState::Working, vec![])),
                },
                // A late update of a task that is over: the verification goes on.
                ThreadState::Verifying
                | ThreadState::Done
                | ThreadState::Failed
                | ThreadState::Cancelled => Ok((state, vec![])),
            }
        }
        AgentTaskState::InputRequired => Ok(blocked(
            state,
            agent_status(actor, AgentStatus::InputRequired, detail.clone()),
            detail,
        )),
        AgentTaskState::AuthRequired => Ok(blocked(
            state,
            agent_status(actor, AgentStatus::AuthRequired, detail.clone()),
            detail,
        )),
        AgentTaskState::Completed => match state {
            // A repeat of the completion that started the verification.
            ThreadState::Verifying => Ok((state, vec![])),
            ThreadState::Queued
            | ThreadState::Working
            | ThreadState::Blocked
            | ThreadState::Done
            | ThreadState::Failed
            | ThreadState::Cancelled => Ok(completed(state, job, actor, detail)),
        },
        AgentTaskState::Failed | AgentTaskState::Rejected => {
            let detail = match task {
                AgentTaskState::Rejected => Some(prefixed("rejected", detail)),
                AgentTaskState::Submitted
                | AgentTaskState::Working
                | AgentTaskState::InputRequired
                | AgentTaskState::AuthRequired
                | AgentTaskState::Completed
                | AgentTaskState::Failed
                | AgentTaskState::Canceled => detail.clone(),
            };
            Ok((
                ThreadState::Failed,
                vec![
                    agent_status(actor, AgentStatus::Failed, detail),
                    entered(ThreadState::Failed),
                ],
            ))
        }
        AgentTaskState::Canceled => Ok((
            ThreadState::Cancelled,
            vec![
                agent_status(actor, AgentStatus::Canceled, detail.clone()),
                entered(ThreadState::Cancelled),
            ],
        )),
    }
}

/// The agent finished. With no gate that is the end; under a gate it is where verification
/// starts.
fn completed(
    state: ThreadState,
    job: &mut Job,
    actor: Actor,
    detail: &Option<String>,
) -> (ThreadState, Vec<Command>) {
    match state {
        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => return (state, vec![]),
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Verifying => {}
    }
    let status = agent_status(actor, AgentStatus::Completed, detail.clone());
    if !job.gate.is_active() {
        return (ThreadState::Done, vec![status, entered(ThreadState::Done)]);
    }
    if let Some(detail) = detail {
        note_summary(job, detail);
    }
    job.verification += 1;
    job.hold = None;
    let (next, more) = verify::conclude(job, true, &[]);
    let mut cmds = vec![status];
    cmds.extend(more);
    (next, cmds)
}

// ---- the gate ------------------------------------------------------------------------------

fn same_repository(a: &str, b: &str) -> bool {
    match (repo_key(a), repo_key(b)) {
        (Some(x), Some(y)) => x == y,
        (Some(_), None) | (None, Some(_)) | (None, None) => false,
    }
}

/// What the gate makes of an artifact of the agent. Only called under an active gate.
fn note_artifact(job: &mut Job, name: &str, uri: Option<&str>, text: Option<&str>) -> Vec<Command> {
    match recognise_artifact(name, uri, text) {
        Recognised::Branch(pushed) => {
            if job.pushed.as_ref() == Some(&pushed) {
                return Vec::new();
            }
            // Facts about another commit no longer count.
            job.results
                .retain(|r| r.commit.as_deref().is_none_or(|c| c == pushed.commit));
            let watch = job.gate.requires(CheckSource::Ci).then(|| Command::Watch {
                key: WatchKey::ci(&pushed.repository, &pushed.commit),
            });
            job.pushed = Some(pushed);
            job.branch_problem = None;
            watch.into_iter().collect()
        }
        Recognised::Checks(report) => {
            if job.gate.requires(CheckSource::AgentChecks) {
                let entry = CheckResult {
                    source: CheckSource::AgentChecks,
                    name: None,
                    attempt: job.attempt,
                    commit: Some(report.commit),
                    status: if report.passed {
                        CheckStatus::Passed
                    } else {
                        CheckStatus::Failed
                    },
                    summary: report.summary,
                    stale: false,
                    findings: report.findings,
                };
                replace_agent_checks(job, entry);
            }
            Vec::new()
        }
        Recognised::Malformed {
            artifact: crate::gate::KnownArtifact::Checks,
            reason,
        } => {
            if job.gate.requires(CheckSource::AgentChecks) {
                let entry = CheckResult {
                    source: CheckSource::AgentChecks,
                    name: None,
                    attempt: job.attempt,
                    commit: None,
                    status: CheckStatus::Failed,
                    summary: None,
                    stale: false,
                    findings: cap_findings([format!(
                        "the agent's `checks` artifact cannot be used: {reason}"
                    )]),
                };
                replace_agent_checks(job, entry);
            }
            Vec::new()
        }
        Recognised::Malformed {
            artifact: crate::gate::KnownArtifact::Branch,
            reason,
        } => {
            // Nothing was pushed as far as the gate can tell, and the agent should hear why
            // (a source with no pushed commit says this instead of "no pushed commit"). An earlier
            // usable `branch` stays the pushed commit.
            job.branch_problem = Some(truncate_to(&reason, 512).to_owned());
            Vec::new()
        }
        // The gate has no opinion on a pull request (the checks decide, not the agent opening one).
        Recognised::PullRequest(_) | Recognised::Other => Vec::new(),
    }
}

fn replace_agent_checks(job: &mut Job, entry: CheckResult) {
    job.results.retain(|r| r.source != CheckSource::AgentChecks);
    job.results.push(entry);
}

/// The ledger entry for a CI report.
fn ci_entry(job: &Job, report: &CiReport) -> CheckResult {
    let passed = report.conclusion.passes();
    let summary = report
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_to(s, 2048).to_owned());
    let findings = if passed {
        Vec::new()
    } else {
        let mut line = format!("{}: {}", report.name, report.conclusion.as_str());
        if let Some(summary) = &summary {
            line.push_str(" - ");
            line.push_str(summary);
        }
        if let Some(url) = &report.url {
            line.push_str(&format!(" ({url})"));
        }
        cap_findings([line])
    };
    CheckResult {
        source: CheckSource::Ci,
        name: Some(report.name.clone()),
        attempt: job.attempt,
        commit: Some(report.sha.to_lowercase()),
        status: if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        summary,
        stale: false,
        findings,
    }
}

/// A CI report. It always leaves its card in the log. It changes the job only when it is about
/// the commit the agent pushed, CI is required and the report counts; then it decides at once
/// if the thread is being verified, otherwise it waits in the ledger for the agent to finish.
fn ci_reported(
    state: ThreadState,
    job: &mut Job,
    report: &CiReport,
) -> (ThreadState, Vec<Command>) {
    let card = append(Actor::system(), EventBody::CiResult(report.clone()));
    match state {
        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
            return (state, vec![card]);
        }
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Verifying
        | ThreadState::Blocked => {}
    }
    let about_the_push = job.gate.requires(CheckSource::Ci)
        && job.pushed.as_ref().is_some_and(|p| {
            p.commit == report.sha.to_lowercase()
                && same_repository(&p.repository, &report.repository)
        });
    if !about_the_push {
        return (state, vec![card]);
    }
    // Only a check the gate names counts; a gate that names none never passes (configuration
    // refuses it, see `orch_app::GateError::CiWithoutChecks`).
    if !job.gate.ci.required.contains(&report.name) {
        return (state, vec![card]);
    }
    let entry = ci_entry(job, report);
    job.results.retain(|r| {
        !(r.source == CheckSource::Ci && r.name == entry.name && r.commit == entry.commit)
    });
    job.results.push(entry);
    match state {
        ThreadState::Verifying => {
            let (next, more) = verify::conclude(job, false, &[CheckSource::Ci]);
            let mut cmds = vec![card];
            cmds.extend(more);
            (next, cmds)
        }
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => (state, vec![card]),
    }
}

/// The verifier's answer. Only the answer to the verification in progress decides. Any other
/// (an abandoned verification, a repeat, a finished thread) is recorded as a `check_result`
/// marked `stale` and changes nothing else.
fn verifier_reported(
    state: ThreadState,
    job: &mut Job,
    attempt: u32,
    verification: u32,
    verdict: &Verdict,
) -> (ThreadState, Vec<Command>) {
    let findings = if verdict.passed || !verdict.findings.is_empty() {
        cap_findings(verdict.findings.iter().cloned())
    } else {
        cap_findings(["the verifier rejected the work without saying why"])
    };
    let status = if verdict.passed {
        CheckStatus::Passed
    } else {
        CheckStatus::Failed
    };
    let entry = CheckResult {
        source: CheckSource::Verifier,
        name: None,
        attempt,
        commit: job.pushed.as_ref().map(|p| p.commit.clone()),
        status,
        summary: None,
        stale: false,
        findings,
    };
    let stale = |entry: CheckResult| {
        (
            state,
            vec![append(
                Actor::system(),
                EventBody::CheckResult(CheckResult {
                    stale: true,
                    ..entry
                }),
            )],
        )
    };
    let current = match state {
        ThreadState::Verifying => {
            attempt == job.attempt
                && verification == job.verification
                && job.gate.requires(CheckSource::Verifier)
                && verify::status_of(job, CheckSource::Verifier) == CheckStatus::Pending
        }
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => false,
    };
    if !current {
        return stale(entry);
    }
    job.results.push(entry);
    verify::conclude(job, false, &[CheckSource::Verifier])
}

/// The verifier could not be used. Only the verification in progress, still waiting for the
/// verifier, is held; a failure reported for any other (an abandoned verification, one already
/// answered, a finished thread) changes nothing. Holding does not use an attempt.
fn verifier_failed(
    state: ThreadState,
    job: &mut Job,
    attempt: u32,
    verification: u32,
    reason: &str,
) -> (ThreadState, Vec<Command>) {
    let waiting = match state {
        ThreadState::Verifying => {
            attempt == job.attempt
                && verification == job.verification
                && job.gate.requires(CheckSource::Verifier)
                && verify::status_of(job, CheckSource::Verifier) == CheckStatus::Pending
        }
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => false,
    };
    if !waiting {
        return (state, vec![]);
    }
    let reason = truncate_to(reason.trim(), MAX_HOLD_REASON_BYTES);
    let message = if reason.is_empty() {
        "the verifier could not be used".to_owned()
    } else {
        format!("the verifier could not be used: {reason}")
    };
    verify::hold(job, Hold::VerifierFailed, &message)
}

/// A deadline. It blocks the thread only if the verification it was armed for is still waiting
/// for the source it guards; a timer of anything else is stale and changes nothing. Blocking
/// does not use an attempt.
fn timer_fired(state: ThreadState, job: &mut Job, timer: Timer) -> (ThreadState, Vec<Command>) {
    match state {
        ThreadState::Verifying => {}
        ThreadState::Queued
        | ThreadState::Working
        | ThreadState::Blocked
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => return (state, vec![]),
    }
    match timer {
        Timer::CiDeadline {
            attempt,
            verification,
        } => {
            let waiting = attempt == job.attempt
                && verification == job.verification
                && job.gate.requires(CheckSource::Ci)
                && verify::status_of(job, CheckSource::Ci) == CheckStatus::Pending;
            if waiting {
                verify::hold(job, Hold::CiTimeout, "CI did not report in time")
            } else {
                (state, vec![])
            }
        }
        Timer::VerifierDeadline {
            attempt,
            verification,
        } => {
            let waiting = attempt == job.attempt
                && verification == job.verification
                && job.gate.requires(CheckSource::Verifier)
                && verify::status_of(job, CheckSource::Verifier) == CheckStatus::Pending;
            if waiting {
                verify::hold(
                    job,
                    Hold::VerifierTimeout,
                    "the verifier did not answer in time",
                )
            } else {
                (state, vec![])
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn class_table() {
        let errors = [
            TransitionError::Finished {
                state: ThreadState::Done,
            },
            TransitionError::InvalidInState {
                state: ThreadState::Queued,
                input: "cancel",
            },
        ];
        for e in errors {
            // Exhaustive: a new variant forces a class decision.
            let expected = match e {
                TransitionError::Finished { .. } | TransitionError::InvalidInState { .. } => {
                    ErrorClass::Rejected
                }
            };
            assert_eq!(e.class(), expected);
            assert!(!e.is_retryable());
        }
    }
}
