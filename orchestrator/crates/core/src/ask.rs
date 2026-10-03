//! Asked agents in the job ledger (ADR 0026, `docs/api/thread-tools-v1.md` `ask_agent`).
//!
//! While a job runs, the agent the thread is addressed to may **ask** one of the agents the person
//! mentioned to do part of the work and wait for its answer. The asked agent runs as a child task
//! of the same thread. This module holds what the core decides about that: the ledger of the
//! job's asks ([`Ask`], in [`Job::asks`](crate::Job::asks)), the limits an ask is checked against
//! ([`AskLimits`]), the two events that tell the story (`ask_started`, `ask_finished`) and the
//! rules, which [`transition`](crate::transition) applies:
//!
//! * an ask is **refused** with a reason ([`AskRefusal`]) when the job is not running, the asker
//!   is not a running ask, the agent was not mentioned in this job, it is the asker or one of the
//!   agents waiting for the asker, or a limit is reached (nesting depth, asks in the job, asks
//!   running at once);
//! * an ask that is accepted logs `ask_started`, writes the `ask` outbox row that sends it
//!   ([`Command::Ask`](crate::Command::Ask)) and arms its deadline
//!   ([`Timer::AskDeadline`](crate::Timer::AskDeadline));
//! * **each ask ends exactly once**, as one `ask_finished`: the asked agent answered
//!   ([`Input::AskFinished`](crate::Input::AskFinished)), the orchestrator could not reach it
//!   ([`Input::AskFailed`](crate::Input::AskFailed)), its deadline passed (`timed_out`), the person
//!   stopped the job, or the task that asked ended (`canceled`, which cascades to the asks it had
//!   asked in turn). An input about an ask that has ended is dropped, like any late input;
//! * **no ask runs after its job's task has ended**: the thread leaving `queued`/`working`/`blocked`
//!   for `verifying`, `done`, `failed` or `cancelled` ends them all.
//!
//! The ledger keeps **who asked whom and how it stands**, not what was said: the text and the
//! result are in the events, and the two commands carry what the dispatcher needs. The core has no
//! clock, so the ledger has no timestamps: the events carry the time the application stamps, and
//! the deadline is a timer that comes back as an input.

use std::time::Duration;

use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::event::{Actor, EventBody};
use crate::gate::{Job, Timer, truncate_to};
use crate::ids::{AgentId, ThreadId};
use crate::thread::ThreadState;
use crate::thread_tools::Caller;
use crate::transition::{Command, EventDraft, TransitionError, append};

/// The deepest chain of asks a deployment allows by default: the addressed agent asks one (depth
/// 1), which may ask one in turn (depth 2), which may not ask.
pub const DEFAULT_ASK_DEPTH: u8 = 2;

/// The most asks one job may make, by default. Asks that ended count.
pub const DEFAULT_MAX_ASKS_PER_JOB: u32 = 16;

/// The most asks of one job that may run at once, by default.
pub const DEFAULT_MAX_RUNNING_ASKS: u32 = 4;

/// How long an ask may run before it ends `timed_out`, by default, in seconds.
pub const DEFAULT_ASK_TIMEOUT_SECS: i64 = 1800;

/// The most bytes of the question an ask may carry (`ask_started.text`). A longer one is refused.
pub const MAX_ASK_TEXT_BYTES: usize = 16 * 1024;

/// The most bytes of an answer that reach the log (`ask_finished.text`); the rest is cut.
pub const MAX_ASK_ANSWER_BYTES: usize = 64 * 1024;

/// The most bytes of a question an asked agent asks back, or of an error, that reach the log.
pub const MAX_ASK_NOTE_BYTES: usize = 4 * 1024;

/// The most artifacts of an answer that reach the log; the rest are dropped.
pub const MAX_ASK_ARTIFACTS: usize = 20;

/// The most bytes of the key a caller names a call by. A longer one is refused.
pub const MAX_CALL_KEY_BYTES: usize = 256;

/// The most earlier tasks of the same agent an ask refers to ([`Command::Ask`]'s
/// `reference_task_ids`): the latest ones.
pub const MAX_ASK_REFERENCES: usize = 8;

/// What an ask is checked against. The application passes it with the input, so the core stays
/// pure and a deployment sets the numbers (`orch-config`, with the tool that asks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskLimits {
    /// The deepest chain of asks: the addressed agent's ask is depth 1, one asked in turn is 2.
    /// An ask that would be deeper is refused.
    pub depth: u8,
    /// The most asks of one job, those that ended included.
    pub per_job: u32,
    /// The most asks of the job running at once.
    pub running: u32,
    /// How long this ask may run before it ends `timed_out`.
    pub timeout: SignedDuration,
}

impl Default for AskLimits {
    fn default() -> Self {
        AskLimits {
            depth: DEFAULT_ASK_DEPTH,
            per_job: DEFAULT_MAX_ASKS_PER_JOB,
            running: DEFAULT_MAX_RUNNING_ASKS,
            timeout: SignedDuration::from_secs(DEFAULT_ASK_TIMEOUT_SECS),
        }
    }
}

impl AskLimits {
    /// The same limits with another deadline, for a caller that asks for a shorter one.
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        AskLimits {
            timeout: SignedDuration::try_from(timeout).unwrap_or(SignedDuration::MAX),
            ..self
        }
    }
}

/// How an ask ended (`ask_finished.state`). Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskOutcome {
    /// The asked agent answered.
    Completed,
    /// The asked agent asked a question back: the ask ends, and asking the same agent again
    /// continues its task (the question is answered by the next ask).
    InputRequired,
    /// The asked agent needs the person to authenticate somewhere; continued like
    /// [`AskOutcome::InputRequired`].
    AuthRequired,
    /// The asked agent failed, or could not be reached.
    Failed,
    /// The asked agent refused the request.
    Rejected,
    /// The ask was cancelled: by the person's stop, because the task that asked ended, or because
    /// the asked agent cancelled its own task.
    Canceled,
    /// The ask's deadline passed.
    TimedOut,
}

impl AskOutcome {
    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            AskOutcome::Completed => "completed",
            AskOutcome::InputRequired => "input_required",
            AskOutcome::AuthRequired => "auth_required",
            AskOutcome::Failed => "failed",
            AskOutcome::Rejected => "rejected",
            AskOutcome::Canceled => "canceled",
            AskOutcome::TimedOut => "timed_out",
        }
    }

    /// Whether asking the same agent again continues its task: it is waiting for something.
    pub const fn is_continuable(self) -> bool {
        match self {
            AskOutcome::InputRequired | AskOutcome::AuthRequired => true,
            AskOutcome::Completed
            | AskOutcome::Failed
            | AskOutcome::Rejected
            | AskOutcome::Canceled
            | AskOutcome::TimedOut => false,
        }
    }
}

/// One ask of the job, as the ledger keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// The ask's number in the job, from 1, in the order the asks were accepted. Children are
    /// always numbered after the ask that asked them.
    pub n: u32,
    /// Who asked: the addressed agent, or an earlier ask of this job.
    pub by: Caller,
    /// The agent asked.
    pub agent: AgentId,
    /// How deep in a chain of asks: 1 for the addressed agent's ask.
    pub depth: u8,
    /// The caller's name for the call that asked (`ask:<thread>:<caller>:<callId>`), when it gave
    /// one. A second ask by the same caller with the same key is the same ask.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_key: Option<String>,
    /// The asked agent's A2A task, once the dispatcher has told the core
    /// ([`Input::AskSent`](crate::Input::AskSent)). Kept so that a later ask of the same agent
    /// can continue it or refer to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// How it ended; `None` while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AskOutcome>,
}

impl Ask {
    /// Whether the ask has not ended.
    pub const fn is_running(&self) -> bool {
        self.outcome.is_none()
    }
}

/// The step an ask is shown as (`ask_started.stepId`): `ask-<n>`. The asked agent's own steps are
/// nested under it.
pub fn ask_step_id(n: u32) -> String {
    format!("ask-{n}")
}

/// The A2A context an agent is asked in: `<thread>-ask-<agent>`. It is the asked agent's own,
/// separate from the worker's (the thread's context) and from the verifier's, and the same for
/// every ask of that agent in the thread, so that asking it again continues the conversation
/// (ADR 0021).
pub fn ask_context(thread: ThreadId, agent: &AgentId) -> String {
    format!("{thread}-ask-{agent}")
}

/// `data` of an `ask_started`: the job's agent asked another one (actor: the asking agent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskStartedData {
    /// The ask's number in the job.
    pub ask: u32,
    /// The agent asked.
    pub agent: AgentId,
    /// Who asked.
    pub by: Caller,
    /// How deep in a chain of asks: 1 for the addressed agent's ask.
    pub depth: u8,
    /// What it was asked: untrusted text from an agent, at most [`MAX_ASK_TEXT_BYTES`] bytes.
    pub text: String,
    /// The step the ask is shown as ([`ask_step_id`]).
    pub step_id: String,
    /// The step of the asking agent that the ask belongs under (its `ask_agent` call), when the
    /// caller named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_step_id: Option<String>,
}

/// An artifact an asked agent handed back, as `ask_finished` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskArtifact {
    /// Artifact name.
    pub name: String,
    /// Where it is, when it has a location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// What an asked agent's task ended with, as the dispatcher reports it
/// ([`Input::AskFinished`](crate::Input::AskFinished)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskResult {
    /// How it ended.
    pub outcome: AskOutcome,
    /// What it said (its answer), when it said anything. Cut at [`MAX_ASK_ANSWER_BYTES`].
    pub text: Option<String>,
    /// The question it asks back, for [`AskOutcome::InputRequired`] and
    /// [`AskOutcome::AuthRequired`]. Cut at [`MAX_ASK_NOTE_BYTES`].
    pub question: Option<String>,
    /// What it handed back. At most [`MAX_ASK_ARTIFACTS`] are kept.
    pub artifacts: Vec<AskArtifact>,
    /// Why it failed, when it did. Cut at [`MAX_ASK_NOTE_BYTES`].
    pub error: Option<String>,
}

impl AskResult {
    /// A result that says only how the ask ended.
    pub fn of(outcome: AskOutcome) -> Self {
        AskResult {
            outcome,
            text: None,
            question: None,
            artifacts: Vec::new(),
            error: None,
        }
    }
}

/// `data` of an `ask_finished`: the ask ended, once (actor: the asked agent for what it said; the
/// orchestrator for a deadline, a cancel, or an agent it could not reach).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskFinishedData {
    /// The ask's number in the job.
    pub ask: u32,
    /// How it ended.
    pub state: AskOutcome,
    /// What the asked agent said: untrusted text, at most [`MAX_ASK_ANSWER_BYTES`] bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The question it asked back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// What it handed back, at most [`MAX_ASK_ARTIFACTS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<AskArtifact>,
    /// Why it did not complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Why an ask was refused. Nothing was written. Closed (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AskRefusal {
    /// The job is not running: it has ended, waits for the person or for its verification, or is
    /// being stopped.
    #[error("the task that asks is over")]
    TaskOver,
    /// The asker is an ask this job does not have.
    #[error("there is no ask {n} in this job")]
    UnknownCaller {
        /// The ask that was named as the asker.
        n: u32,
    },
    /// The agent is not one the person mentioned in this job.
    #[error(
        "you can ask only the agents the person mentioned in this job ({}), not '{agent}'",
        names(.mentioned)
    )]
    NotMentioned {
        /// The agent that was asked.
        agent: AgentId,
        /// The agents that may be asked.
        mentioned: Vec<AgentId>,
    },
    /// The agent is the asker, or one of the agents waiting for the asker: asking it would be a
    /// cycle.
    #[error("'{agent}' is the asking agent or waits for it: asking it would go in a circle")]
    Cycle {
        /// The agent that was asked.
        agent: AgentId,
    },
    /// The ask would be nested deeper than the limit.
    #[error("asks may be nested {max} deep at most")]
    DepthReached {
        /// The limit.
        max: u8,
    },
    /// The job has made as many asks as it may.
    #[error("a job may make {max} asks at most")]
    TooManyInJob {
        /// The limit.
        max: u32,
    },
    /// As many asks as may run at once run already.
    #[error("{max} asks are running already; wait for one to end")]
    TooManyRunning {
        /// The limit.
        max: u32,
    },
    /// The question has no text.
    #[error("the question is empty")]
    EmptyText,
    /// The question is longer than [`MAX_ASK_TEXT_BYTES`].
    #[error("the question may hold {max} bytes at most")]
    TextTooLong {
        /// The limit.
        max: usize,
    },
    /// The call key is longer than [`MAX_CALL_KEY_BYTES`].
    #[error("the call key may hold {max} bytes at most")]
    CallKeyTooLong {
        /// The limit.
        max: usize,
    },
}

fn names(agents: &[AgentId]) -> String {
    agents
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// An ask as the input carries it, borrowed.
pub(crate) struct AskRequest<'a> {
    pub actor: &'a Actor,
    pub caller: Caller,
    pub agent: &'a AgentId,
    pub text: &'a str,
    pub call_key: Option<&'a str>,
    pub parent_step: Option<&'a str>,
    pub limits: &'a AskLimits,
}

const TASK_ENDED: &str = "the asking task ended";
/// Why asks end when the person stops the job.
pub(crate) const PERSON_STOPPED: &str = "the person stopped the job";
/// Why asks end when the task that asked is over.
pub(crate) const ASKER_ENDED: &str = TASK_ENDED;
const DEADLINE_PASSED: &str = "the asked agent did not answer in time";

fn find(job: &Job, n: u32) -> Option<&Ask> {
    job.asks.iter().find(|a| a.n == n)
}

/// An ask of the job, accepted or refused (the rules in the module documentation).
///
/// On success the ask is in the ledger and the commands are: `ask_started`, the `ask` row, and the
/// deadline. A call that repeats one the ledger has (same caller, same key) is the same ask and
/// writes nothing.
pub(crate) fn request(
    state: ThreadState,
    job: &mut Job,
    ask: &AskRequest<'_>,
) -> Result<Vec<Command>, TransitionError> {
    let refuse = |why: AskRefusal| Err(TransitionError::AskRefused(why));
    match state {
        ThreadState::Queued | ThreadState::Working => {}
        ThreadState::Blocked
        | ThreadState::Verifying
        | ThreadState::Done
        | ThreadState::Failed
        | ThreadState::Cancelled => return refuse(AskRefusal::TaskOver),
    }
    // A job that is being stopped starts its replacement as soon as its task ends; nothing it
    // asks now would be waited for.
    if job.after_stop.is_some() {
        return refuse(AskRefusal::TaskOver);
    }
    if ask.text.trim().is_empty() {
        return refuse(AskRefusal::EmptyText);
    }
    if ask.text.len() > MAX_ASK_TEXT_BYTES {
        return refuse(AskRefusal::TextTooLong {
            max: MAX_ASK_TEXT_BYTES,
        });
    }
    if ask.call_key.is_some_and(|k| k.len() > MAX_CALL_KEY_BYTES) {
        return refuse(AskRefusal::CallKeyTooLong {
            max: MAX_CALL_KEY_BYTES,
        });
    }
    // how deep the asker is, and the agents waiting for it
    let (asker_depth, waiting) = match ask.caller {
        Caller::Main => (0, Vec::new()),
        Caller::Ask(m) => match find(job, m) {
            Some(parent) if parent.is_running() => (parent.depth, chain(job, parent)),
            Some(_) => return refuse(AskRefusal::TaskOver),
            None => return refuse(AskRefusal::UnknownCaller { n: m }),
        },
    };
    // The same call again is the same ask, whatever has become of it since: the caller reads its
    // result from the ledger and the log.
    if let Some(key) = ask.call_key
        && job
            .asks
            .iter()
            .any(|a| a.by == ask.caller && a.call_key.as_deref() == Some(key))
    {
        return Ok(Vec::new());
    }
    if !job.mentioned.contains(ask.agent) {
        return refuse(AskRefusal::NotMentioned {
            agent: ask.agent.clone(),
            mentioned: job.mentioned.iter().cloned().collect(),
        });
    }
    if waiting.contains(ask.agent) {
        return refuse(AskRefusal::Cycle {
            agent: ask.agent.clone(),
        });
    }
    let depth = asker_depth.saturating_add(1);
    if depth > ask.limits.depth {
        return refuse(AskRefusal::DepthReached {
            max: ask.limits.depth,
        });
    }
    let asked = u32::try_from(job.asks.len()).unwrap_or(u32::MAX);
    if asked >= ask.limits.per_job {
        return refuse(AskRefusal::TooManyInJob {
            max: ask.limits.per_job,
        });
    }
    let running =
        u32::try_from(job.asks.iter().filter(|a| a.is_running()).count()).unwrap_or(u32::MAX);
    if running >= ask.limits.running {
        return refuse(AskRefusal::TooManyRunning {
            max: ask.limits.running,
        });
    }

    let n = job
        .asks
        .iter()
        .map(|a| a.n)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let (continue_task, reference_task_ids) = continuation(job, ask.agent);
    job.asks.push(Ask {
        n,
        by: ask.caller,
        agent: ask.agent.clone(),
        depth,
        call_key: ask.call_key.map(str::to_owned),
        task_id: None,
        outcome: None,
    });
    Ok(vec![
        append(
            ask.actor.clone(),
            EventBody::AskStarted(AskStartedData {
                ask: n,
                agent: ask.agent.clone(),
                by: ask.caller,
                depth,
                text: ask.text.to_owned(),
                step_id: ask_step_id(n),
                parent_step_id: ask.parent_step.map(str::to_owned),
            }),
        ),
        Command::Ask {
            job: job.number,
            ask: n,
            agent: ask.agent.clone(),
            depth,
            text: ask.text.to_owned(),
            continue_task,
            reference_task_ids,
        },
        Command::Schedule {
            after: ask.limits.timeout,
            timer: Timer::AskDeadline {
                job: job.number,
                ask: n,
            },
        },
    ])
}

/// The agents of `ask` and of the asks above it, the asker's side of the chain.
fn chain(job: &Job, ask: &Ask) -> Vec<AgentId> {
    let mut agents = vec![ask.agent.clone()];
    let mut by = ask.by;
    while let Caller::Ask(m) = by {
        let Some(parent) = find(job, m) else { break };
        agents.push(parent.agent.clone());
        by = parent.by;
    }
    agents
}

/// Whether an ask of `agent` continues the task of the last one (it ended waiting for something
/// and the dispatcher recorded the task), else which earlier tasks of the agent in this job it
/// refers to (ADR 0021).
fn continuation(job: &Job, agent: &AgentId) -> (Option<String>, Vec<String>) {
    let earlier: Vec<&Ask> = job.asks.iter().filter(|a| &a.agent == agent).collect();
    if let Some(last) = earlier.last()
        && last.outcome.is_some_and(AskOutcome::is_continuable)
        && let Some(task) = &last.task_id
    {
        return (Some(task.clone()), Vec::new());
    }
    // A task that several asks continued is one task: it is referred to once.
    let mut tasks: Vec<String> = Vec::new();
    for task in earlier.iter().filter_map(|a| a.task_id.clone()) {
        if !tasks.contains(&task) {
            tasks.push(task);
        }
    }
    let skip = tasks.len().saturating_sub(MAX_ASK_REFERENCES);
    (None, tasks.into_iter().skip(skip).collect())
}

/// The dispatcher's report that the asked agent took the message and its task is `task_id`.
/// Recorded once, while the ask runs; any other report is dropped. Writes no event.
pub(crate) fn sent(job: &mut Job, n: u32, task_id: &str) {
    if let Some(ask) = job.asks.iter_mut().find(|a| a.n == n)
        && ask.is_running()
        && ask.task_id.is_none()
    {
        ask.task_id = Some(task_id.to_owned());
    }
}

/// What the asked agent ended its task with. An ask that has ended already, or that the job does
/// not have, is a late input: dropped.
pub(crate) fn finished(job: &mut Job, n: u32, actor: Actor, result: &AskResult) -> Vec<Command> {
    let data = AskFinishedData {
        ask: n,
        state: result.outcome,
        text: result
            .text
            .as_deref()
            .map(|t| truncate_to(t, MAX_ASK_ANSWER_BYTES).to_owned()),
        question: result
            .question
            .as_deref()
            .map(|t| truncate_to(t, MAX_ASK_NOTE_BYTES).to_owned()),
        artifacts: result
            .artifacts
            .iter()
            .take(MAX_ASK_ARTIFACTS)
            .cloned()
            .collect(),
        error: result
            .error
            .as_deref()
            .map(|t| truncate_to(t, MAX_ASK_NOTE_BYTES).to_owned()),
    };
    end(job, n, actor, data)
}

/// The orchestrator could not get an answer from the asked agent (its agent is not listed any
/// more, cannot be reached, refused the request): the ask failed.
pub(crate) fn failed(job: &mut Job, n: u32, reason: &str) -> Vec<Command> {
    end(
        job,
        n,
        Actor::system(),
        AskFinishedData {
            ask: n,
            state: AskOutcome::Failed,
            text: None,
            question: None,
            artifacts: Vec::new(),
            error: Some(truncate_to(reason.trim(), MAX_ASK_NOTE_BYTES).to_owned()),
        },
    )
}

/// The deadline armed for ask `n` of job `of` passed. One of an earlier job, or of an ask that
/// has ended, is stale and changes nothing.
pub(crate) fn deadline(job: &mut Job, of: u32, n: u32) -> Vec<Command> {
    if of != job.number {
        return Vec::new();
    }
    end(
        job,
        n,
        Actor::system(),
        AskFinishedData {
            ask: n,
            state: AskOutcome::TimedOut,
            text: None,
            question: None,
            artifacts: Vec::new(),
            error: Some(DEADLINE_PASSED.to_owned()),
        },
    )
}

/// Ends every ask that still runs, `canceled`, for `why` (the person stopped the job, or the task
/// that asked is over).
pub(crate) fn cancel_all(job: &mut Job, why: &str) -> Vec<Command> {
    let running: Vec<u32> = job
        .asks
        .iter()
        .filter(|a| a.is_running())
        .map(|a| a.n)
        .collect();
    running
        .into_iter()
        .flat_map(|n| end(job, n, Actor::system(), canceled(n, why)))
        .collect()
}

fn canceled(n: u32, why: &str) -> AskFinishedData {
    AskFinishedData {
        ask: n,
        state: AskOutcome::Canceled,
        text: None,
        question: None,
        artifacts: Vec::new(),
        error: Some(why.to_owned()),
    }
}

/// Ends ask `n` with `data`, once, and the asks it asked, `canceled`: an ask that ended is not
/// waiting for them any more. `None` of it when the ask has ended already.
fn end(job: &mut Job, n: u32, actor: Actor, data: AskFinishedData) -> Vec<Command> {
    let Some(ask) = job.asks.iter_mut().find(|a| a.n == n) else {
        return Vec::new();
    };
    if !ask.is_running() {
        return Vec::new();
    }
    ask.outcome = Some(data.state);
    let mut cmds = vec![append(actor, EventBody::AskFinished(data))];
    // children are numbered after their parent: one pass, in order, reaches every descendant
    let mut ended = vec![n];
    for child in &mut job.asks {
        let under = matches!(child.by, Caller::Ask(m) if ended.contains(&m));
        if under && child.is_running() {
            child.outcome = Some(AskOutcome::Canceled);
            ended.push(child.n);
            cmds.push(append(
                Actor::system(),
                EventBody::AskFinished(canceled(child.n, TASK_ENDED)),
            ));
        }
    }
    cmds
}

/// Puts the commands that end asks into `cmds`: before the `thread_state` event of the same
/// transition when there is one, else at the end, so that a thread's last word is its own.
pub(crate) fn place(cmds: &mut Vec<Command>, ending: Vec<Command>) {
    if ending.is_empty() {
        return;
    }
    let at = cmds
        .iter()
        .position(|c| {
            matches!(
                c,
                Command::Append(EventDraft {
                    body: EventBody::ThreadState(_),
                    ..
                })
            )
        })
        .unwrap_or(cmds.len());
    cmds.splice(at..at, ending);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn an_outcome_has_one_spelling_and_a_status() {
        for (outcome, text) in [
            (AskOutcome::Completed, "completed"),
            (AskOutcome::InputRequired, "input_required"),
            (AskOutcome::AuthRequired, "auth_required"),
            (AskOutcome::Failed, "failed"),
            (AskOutcome::Rejected, "rejected"),
            (AskOutcome::Canceled, "canceled"),
            (AskOutcome::TimedOut, "timed_out"),
        ] {
            assert_eq!(outcome.as_str(), text);
            assert_eq!(
                serde_json::to_value(outcome).unwrap(),
                serde_json::json!(text)
            );
        }
        assert!(AskOutcome::InputRequired.is_continuable());
        assert!(AskOutcome::AuthRequired.is_continuable());
        assert!(!AskOutcome::Completed.is_continuable());
        assert!(!AskOutcome::TimedOut.is_continuable());
    }

    #[test]
    fn the_defaults_are_the_ones_the_owner_chose() {
        let limits = AskLimits::default();
        assert_eq!((limits.depth, limits.per_job, limits.running), (2, 16, 4));
        assert_eq!(limits.timeout, SignedDuration::from_secs(1800));
        assert_eq!(
            limits.with_timeout(Duration::from_secs(60)).timeout,
            SignedDuration::from_secs(60)
        );
    }

    #[test]
    fn the_names_of_a_context_and_a_step() {
        let thread = ThreadId(uuid::Uuid::nil());
        assert_eq!(ask_step_id(3), "ask-3");
        assert_eq!(
            ask_context(thread, &AgentId::new("mock-researcher")),
            "00000000-0000-0000-0000-000000000000-ask-mock-researcher"
        );
    }

    #[test]
    fn a_refusal_names_the_agents_that_may_be_asked() {
        let refusal = AskRefusal::NotMentioned {
            agent: AgentId::new("c"),
            mentioned: vec![AgentId::new("a"), AgentId::new("b")],
        };
        assert_eq!(
            refusal.to_string(),
            "you can ask only the agents the person mentioned in this job (a, b), not 'c'"
        );
    }
}
