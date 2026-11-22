//! What a live card says under `child-sessions/v1` (ADR 0072, `docs/api/child-sessions-v1.md`):
//! the helpers a session can be started with, and how many may be open at once in one thread.
//!
//! A card lists the extension with `params: {agents, maxOpen}`, read here as a
//! [`ChildSessionsOffer`]. Only well-formed `params` are an offer: a card that lists the URI badly
//! offers nothing (ADR 0008, fail closed).
//!
//! The rest of the module is what the pure core decides about a session once an adapter has read
//! it into a [`SessionReport`] (ADR 0072, `docs/orchestrator.md` "Child sessions"): the parent's
//! three events and the ledger that makes them once each ([`SessionLedger`]), the child thread's
//! own ledger ([`ChildLedger`]) and the observed round ([`ObservedRound`]) it follows. Reading the
//! wire (JSON, protobuf state words) is the adapter's; the core only checks the bounds again.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::access_tier::AccessTier;
use crate::agent::AgentTaskState;
use crate::description::{DescriptionLedger, DescriptionSource};
use crate::event::{
    Actor, ErrorData, EventBody, JobRound, JobStartedData, Origin, UserMessageData,
};
use crate::gate::{Job, Snapshot, truncate_to};
use crate::ids::{ThreadId, UserId};
use crate::thread::ThreadState;
use crate::title::{TitleLedger, TitleSource, check_title};
use crate::transition::{Command, TransitionError, append};

/// The URI of `child-sessions/v1`: helpers an agent runs as sessions a client can open, talk to
/// and close (ADR 0072).
pub const CHILD_SESSIONS_EXTENSION: &str =
    "https://agents.vymalo.com/a2a/extensions/child-sessions/v1";

/// One helper a session can be started with: the agent's own name and description (untrusted
/// card text, drawn as text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildAgent {
    /// The helper's name, as a `start` names it; never empty.
    pub name: String,
    /// What the helper does, for the person.
    pub description: String,
}

/// The `params` of a card's `child-sessions/v1` entry, well formed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionsOffer {
    agents: Vec<ChildAgent>,
    max_open: u32,
}

impl ChildSessionsOffer {
    /// Reads the `params` of the card's entry: `agents`, a non-empty array of `{name,
    /// description}` (both strings, `name` not blank and each name once), and `maxOpen`, a whole
    /// number of at least 1. Unknown members are ignored. `None` for anything else.
    pub fn from_params(params: &Value) -> Option<ChildSessionsOffer> {
        let listed = params.get("agents")?.as_array()?;
        let mut agents: Vec<ChildAgent> = Vec::with_capacity(listed.len());
        for entry in listed {
            let name = entry.get("name")?.as_str()?;
            let description = entry.get("description")?.as_str()?;
            if name.trim().is_empty() || agents.iter().any(|a| a.name == name) {
                return None;
            }
            agents.push(ChildAgent {
                name: name.to_owned(),
                description: description.to_owned(),
            });
        }
        if agents.is_empty() {
            return None;
        }
        let max_open = whole_at_least_one(params.get("maxOpen")?)?;
        Some(ChildSessionsOffer { agents, max_open })
    }

    /// The helpers, in the card's order.
    pub fn agents(&self) -> &[ChildAgent] {
        &self.agents
    }

    /// The most sessions open at once in one parent context.
    pub fn max_open(&self) -> u32 {
        self.max_open
    }

    /// Whether a session can be started with the helper `name`.
    pub fn offers_agent(&self, name: &str) -> bool {
        self.agents.iter().any(|a| a.name == name)
    }
}

/// A JSON number that is a whole number in `1..=u32::MAX`; a card's numbers may come back as
/// doubles from an SDK, so `16.0` counts as 16.
fn whole_at_least_one(value: &Value) -> Option<u32> {
    let n = match value.as_u64() {
        Some(n) => n,
        None => {
            let f = value.as_f64()?;
            if f.fract() != 0.0 || f < 1.0 || f > f64::from(u32::MAX) {
                return None;
            }
            // whole and in range, checked above
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = f as u64;
            n
        }
    };
    u32::try_from(n).ok().filter(|n| *n >= 1)
}

/// The most sessions one parent thread notes, counting every session in the thread's life: a
/// closed session keeps its note (it is what makes the parent's events once each), so it still
/// counts. A session past it is refused
/// ([`TransitionError::ChildSessionRefused`]) and stays a step of the parent's task.
pub const MAX_CHILD_SESSIONS: usize = 256;
/// The longest session id, in bytes.
pub const MAX_SESSION_ID_BYTES: usize = 128;
/// The longest child context id, in bytes.
pub const MAX_SESSION_CONTEXT_BYTES: usize = 512;
/// The longest helper name a report carries, in bytes.
pub const MAX_SESSION_AGENT_BYTES: usize = 128;
/// The longest description a report carries, in characters (an adapter cuts to it).
pub const MAX_SESSION_DESCRIPTION_CHARS: usize = 256;
/// The longest round task id, in bytes.
pub const MAX_SESSION_TASK_BYTES: usize = 256;
/// The most sessions one `sessions` object may list.
pub const MAX_SESSIONS_LISTED: usize = 512;

/// Who started a session or a round (`by` in the contract). Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionBy {
    /// The parent's model.
    Agent,
    /// A person.
    Person,
}

/// A session's state as the agent says it (`working`, `idle`, `closed`): the contract's, never the
/// thread's. Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// A round is open.
    Working,
    /// Kept open, no round is open.
    Idle,
    /// Over: never reopened.
    Closed,
}

/// How a round ended, as the parent's log says it (`report` of `child_session_reported`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOutcome {
    /// The round completed.
    Done,
    /// It failed or was rejected.
    Failed,
    /// It was canceled.
    Canceled,
    /// It waits for the person (input or authentication).
    Blocked,
}

/// What a round's final task state says to the parent: `None` while the round has not ended.
pub fn report_of(state: AgentTaskState) -> Option<SessionOutcome> {
    match state {
        AgentTaskState::Completed => Some(SessionOutcome::Done),
        AgentTaskState::Failed | AgentTaskState::Rejected => Some(SessionOutcome::Failed),
        AgentTaskState::Canceled => Some(SessionOutcome::Canceled),
        AgentTaskState::InputRequired | AgentTaskState::AuthRequired => {
            Some(SessionOutcome::Blocked)
        }
        AgentTaskState::Submitted | AgentTaskState::Working => None,
    }
}

/// A round of a session as an agent reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundReport {
    /// The round's number, from 1.
    pub number: u32,
    /// The round's A2A task; absent when the agent did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The round task's state; `None` when the agent's word for it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<AgentTaskState>,
}

/// One session as an agent reported it, through the adapter's strict read ([`Self::check`] is the
/// core's door).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionReport {
    /// The session's id (opaque), 1 to [`MAX_SESSION_ID_BYTES`] bytes.
    pub session: String,
    /// The child A2A context the thread binds to (opaque), 1 to [`MAX_SESSION_CONTEXT_BYTES`].
    pub context_id: String,
    /// The helper's name, at most [`MAX_SESSION_AGENT_BYTES`] bytes.
    pub agent: String,
    /// What the helper was asked to do (the agent's text, untrusted).
    pub description: String,
    /// Whether the session goes idle after a round instead of closing.
    pub keep_open: bool,
    /// Who started it, when the report says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<SessionBy>,
    /// The session's state now.
    pub state: SessionState,
    /// A close waits for the open round to end.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closing: bool,
    /// The newest round the report names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<RoundReport>,
    /// The A2A context of the parent task the session belongs to, when the report says (only a
    /// round's object does): what a person's start is checked against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_context_id: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Why a [`SessionReport`] failed the core's door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionInvalid {
    /// The session id is empty, too long or has a control character.
    #[error("the session id is not 1 to {MAX_SESSION_ID_BYTES} printable bytes")]
    BadSession,
    /// The context id is empty, too long or has a control character.
    #[error("the context id is not 1 to {MAX_SESSION_CONTEXT_BYTES} printable bytes")]
    BadContext,
    /// The helper name is too long or has a control character.
    #[error("the helper name is not at most {MAX_SESSION_AGENT_BYTES} printable bytes")]
    BadAgent,
    /// The description is longer than [`MAX_SESSION_DESCRIPTION_CHARS`] characters.
    #[error("the description is longer than {MAX_SESSION_DESCRIPTION_CHARS} characters")]
    BadDescription,
    /// The round's number is 0 or its task id is not printable text of a bounded size.
    #[error("the round is not a number from 1 and a printable task id")]
    BadRound,
}

fn printable(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

impl SessionReport {
    /// The bounds an adapter reads the wire under, checked again at the core's door.
    ///
    /// # Errors
    /// [`SessionInvalid`] for the first member that is out of bounds.
    pub fn check(&self) -> Result<(), SessionInvalid> {
        if !printable(&self.session, MAX_SESSION_ID_BYTES) {
            return Err(SessionInvalid::BadSession);
        }
        if !printable(&self.context_id, MAX_SESSION_CONTEXT_BYTES) {
            return Err(SessionInvalid::BadContext);
        }
        if self.agent.len() > MAX_SESSION_AGENT_BYTES || self.agent.chars().any(char::is_control) {
            return Err(SessionInvalid::BadAgent);
        }
        if self.description.chars().count() > MAX_SESSION_DESCRIPTION_CHARS {
            return Err(SessionInvalid::BadDescription);
        }
        if let Some(round) = &self.round {
            let bad_task = round
                .task_id
                .as_deref()
                .is_some_and(|t| !printable(t, MAX_SESSION_TASK_BYTES));
            if round.number == 0 || bad_task {
                return Err(SessionInvalid::BadRound);
            }
        }
        Ok(())
    }

    fn ended_round(&self) -> Option<(u32, SessionOutcome)> {
        let round = self.round.as_ref()?;
        Some((round.number, report_of(round.state?)?))
    }
}

/// The kind of a `child-sessions/v1` status event (`event` in the contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEvent {
    /// A session was started.
    Opened,
    /// A round other than the first started.
    Round,
    /// A round ended.
    Reported,
    /// The session closed.
    Closed,
}

/// What an adapter learned about sessions, in one piece: the three places the contract says it
/// (`docs/api/child-sessions-v1.md` sections 3 and 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionNews {
    /// The sessions object of a parent task: the truth.
    Listed(Vec<SessionReport>),
    /// One event of a parent task's status update, with the session it names.
    Event(SessionEvent, SessionReport),
    /// The session a round task says it belongs to.
    OfRound(SessionReport),
}

/// A session the parent thread has noted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNote {
    /// The session's id.
    pub session: String,
    /// The child thread made for it.
    pub child: ThreadId,
    /// The newest round whose end was reported to the parent's log (0: none).
    pub reported: u32,
    /// `child_session_closed` is in the parent's log.
    pub closed: bool,
    /// How the round `reported` ended when it was reported: a round that was `blocked` and goes on
    /// (the person answered within the same round) is reported again when it ends otherwise, and
    /// the same `(round, outcome)` is never reported twice. `None` in a note stored before the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SessionOutcome>,
}

/// The sessions a parent thread has noted (`Job.sessions`): what makes the parent's three events
/// once each. Belongs to the conversation: [`Job::next`] keeps it. At most
/// [`MAX_CHILD_SESSIONS`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionLedger {
    notes: Vec<SessionNote>,
}

impl SessionLedger {
    /// A ledger with these notes (what a store reads back).
    pub fn of(notes: Vec<SessionNote>) -> Self {
        SessionLedger { notes }
    }

    /// The parent has noted no session (what the log leaves out).
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// How many sessions are noted.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// The notes, in the order the sessions were first seen.
    pub fn notes(&self) -> &[SessionNote] {
        &self.notes
    }

    /// The note of `session`.
    pub fn note(&self, session: &str) -> Option<&SessionNote> {
        self.notes.iter().find(|n| n.session == session)
    }
}

/// What a child thread knows of its session (`Job.child`). Belongs to the conversation:
/// [`Job::next`] keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildLedger {
    /// The session's id.
    pub session: String,
    /// The helper's name.
    pub agent: String,
    /// Whether the session goes idle after a round.
    pub keep_open: bool,
    /// The session's state as last reported.
    pub state: SessionState,
    /// A close waits for the open round to end.
    #[serde(default, skip_serializing_if = "is_false")]
    pub closing: bool,
    /// The newest round the thread was told to follow (0: none).
    #[serde(default)]
    pub followed: u32,
    /// The highest round the thread started a job for (0: none). A start at or below it is a
    /// replay or a reordering and changes nothing.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub started: u32,
    /// The round the thread was blocked on has gone on within the same round number (the person
    /// answered it) and the thread was told to follow it again, once; cleared when the thread is
    /// heard of again and is no longer blocked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub refollow: bool,
    /// A person asked to close the session (`child_session_close_asked`): the closing round that
    /// follows is never a job of this thread.
    #[serde(default, skip_serializing_if = "is_false")]
    pub close_asked: bool,
    /// The close that was asked cancels the working round first.
    #[serde(default, skip_serializing_if = "is_false")]
    pub close_cancel: bool,
    /// Which round is whose, and how new the state is.
    #[serde(default, skip_serializing_if = "RoundTrack::is_empty")]
    pub track: RoundTrack,
}

/// What a child thread remembers to tell the rounds it did not start from the ones it did, and to
/// keep a stale report from moving its state back. Round 0 is "none".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundTrack {
    /// The newest round known when the close was asked: the close makes the next one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub close_after: u32,
    /// The round the answer to the close named: the closing round, once known.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub close_round: u32,
    /// The newest round a report that moved the state was about.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub state_round: u32,
}

impl RoundTrack {
    /// Nothing remembered.
    pub fn is_empty(&self) -> bool {
        *self == RoundTrack::default()
    }
}

/// The round of a session this thread's current job follows without having sent it (`Job.observed`,
/// ADR 0072 decision 3). Belongs to the job: [`Job::next`] forgets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedRound {
    /// The round's number.
    pub number: u32,
    /// The round's A2A task.
    pub task_id: String,
    /// Who started it, when the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<SessionBy>,
}

impl ObservedRound {
    /// Whether the tokens of this round are counted in this thread: only a round a person started
    /// is. A round the parent's model started is in the parent's totals (ADR 0072, amended
    /// 2026-11-18), and so is a round whose `by` the agent did not say (`None`): it is taken for
    /// the agent's and left uncounted here. A blocked child that a person answers within the same
    /// job keeps this round's `by` (ADR 0072, S3: the round keeps its starter's `by`), so the
    /// parent counts that answer.
    pub fn counts_usage(&self) -> bool {
        self.by == Some(SessionBy::Person)
    }
}

/// The thread a child thread belongs to and the session it is (`child_of`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildOf {
    /// The parent thread.
    pub thread: ThreadId,
    /// The session's id.
    pub session: String,
}

/// `data` of a `child_session_opened`, in the parent's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionOpenedData {
    /// The session's id.
    pub session: String,
    /// The child thread.
    pub thread: ThreadId,
    /// The helper's name.
    pub agent: String,
    /// What the helper was asked to do.
    pub description: String,
    /// Whether the session goes idle after a round.
    pub keep_open: bool,
    /// Who started it, when the agent said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<SessionBy>,
    /// The number of the round the report named when the session was first seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u32>,
}

/// `data` of a `child_session_reported`, in the parent's log: a round ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionReportedData {
    /// The session's id.
    pub session: String,
    /// The child thread.
    pub thread: ThreadId,
    /// The round that ended.
    pub round: u32,
    /// How it ended.
    pub report: SessionOutcome,
}

/// `data` of a `child_session_closed`, in the parent's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionClosedData {
    /// The session's id.
    pub session: String,
    /// The child thread.
    pub thread: ThreadId,
}

/// The most sessions a person may have asked one parent thread to start whose outcome is not
/// known yet ([`Job::session_starts`]); a fifth is refused ([`SessionRefusal::TooManyStarts`]).
pub const MAX_PENDING_STARTS: usize = 4;
/// The longest start request id, in bytes.
pub const MAX_REQUEST_BYTES: usize = 128;
/// The longest description of a person's start, in characters (`child-sessions/v1` section 4).
pub const MAX_START_DESCRIPTION_CHARS: usize = 80;
/// The most bytes of an agent's words that `child_session_start_ended` keeps.
pub const MAX_START_REASON_BYTES: usize = 512;

/// A start a person asked for whose outcome is not in the parent's log yet (`Job.session_starts`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingStart {
    /// The id the person's request carried.
    pub request: String,
}

/// How a start a person asked for ended (`Input::SessionStartEnded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStartOutcome {
    /// The agent opened the session and its child thread exists.
    Opened {
        /// The session's id.
        session: String,
        /// The child thread made for it.
        child: ThreadId,
    },
    /// The agent refused, or could not be reached: the words are for the person (untrusted).
    Failed {
        /// What the agent said, or the orchestrator's own words.
        reason: String,
    },
}

/// `outcome` of a `child_session_start_ended`. Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartOutcomeKind {
    /// The session was opened.
    Opened,
    /// The start did not happen.
    Failed,
}

/// `data` of a `child_session_start_asked`, in the parent's log. It has no place for the person's
/// text: the log keeps what was asked of whom, never the words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionStartAskedData {
    /// The id of the person's request.
    pub request: String,
    /// The helper asked for.
    pub agent: String,
    /// What the person said the helper is for (at most [`MAX_START_DESCRIPTION_CHARS`]).
    pub description: String,
    /// Whether the session goes idle after a round.
    pub keep_open: bool,
    /// The access tier the first round was asked under, when one applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<AccessTier>,
}

/// `data` of a `child_session_start_ended`, in the parent's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionStartEndedData {
    /// The id of the person's request.
    pub request: String,
    /// How it ended.
    pub outcome: StartOutcomeKind,
    /// The session's id, when it opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The child thread, when it opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
    /// Why it did not, in the agent's words (untrusted, at most [`MAX_START_REASON_BYTES`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `data` of a `child_session_close_asked`, in the child's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionCloseAskedData {
    /// The session's id.
    pub session: String,
    /// The close cancels the working round first.
    pub cancel: bool,
}

/// `data` of a `child_session_state`, in the child's log: the session's state changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionStateData {
    /// The session's id.
    pub session: String,
    /// The session's state now.
    pub state: SessionState,
    /// A close waits for the open round to end.
    pub closing: bool,
    /// A person's close is asked and not answered yet. Said (and absent when false, as it is by
    /// default) so a refused close, which changes neither `state` nor `closing`, clears it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close_asked: bool,
}

/// What a person's first message to a session they start is: written in the child's first commit,
/// before the round it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonPrompt {
    /// The person.
    pub user: UserId,
    /// What they asked the helper.
    pub text: String,
    /// The id the request gave the message, recorded in the log.
    pub message_id: Option<String>,
    /// The access tier the first round was asked under.
    pub tier: Option<AccessTier>,
}

/// Why a report about a session was refused ([`TransitionError::ChildSessionRefused`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionRefusal {
    /// The parent notes [`MAX_CHILD_SESSIONS`] sessions already.
    #[error("this thread already has {MAX_CHILD_SESSIONS} child sessions")]
    LedgerFull,
    /// The report, or a person's request, fails its bounds.
    #[error("the session report is malformed")]
    Malformed,
    /// A person asked to start a session while [`MAX_PENDING_STARTS`] starts of this thread were
    /// still waiting for their outcome.
    #[error("{MAX_PENDING_STARTS} sessions are already starting in this thread")]
    TooManyStarts,
    /// A child thread cannot start sessions: only a parent can.
    #[error("a helper session cannot start another")]
    ChildThread,
}

fn malformed(_: SessionInvalid) -> TransitionError {
    TransitionError::ChildSessionRefused(SessionRefusal::Malformed)
}

/// The parent's decision on a session (`Input::ChildSession`): the events it owes the log, each
/// once whatever the order or number of the reports (the ledger decides, not the report), and the
/// note that remembers it. Never moves the parent's state.
pub(crate) fn learn(
    job: &mut Job,
    actor: &Actor,
    report: &SessionReport,
    child: ThreadId,
) -> Result<Vec<Command>, TransitionError> {
    report.check().map_err(malformed)?;
    let mut cmds = Vec::new();
    let at = match job
        .sessions
        .notes
        .iter()
        .position(|n| n.session == report.session)
    {
        Some(at) => at,
        None => {
            if job.sessions.notes.len() >= MAX_CHILD_SESSIONS {
                return Err(TransitionError::ChildSessionRefused(
                    SessionRefusal::LedgerFull,
                ));
            }
            cmds.push(append(
                actor.clone(),
                EventBody::ChildSessionOpened(ChildSessionOpenedData {
                    session: report.session.clone(),
                    thread: child,
                    agent: report.agent.clone(),
                    description: report.description.clone(),
                    keep_open: report.keep_open,
                    by: report.by,
                    round: report.round.as_ref().map(|r| r.number),
                }),
            ));
            job.sessions.notes.push(SessionNote {
                session: report.session.clone(),
                child,
                reported: 0,
                closed: false,
                outcome: None,
            });
            job.sessions.notes.len() - 1
        }
    };
    let note = &mut job.sessions.notes[at];
    if let Some((round, outcome)) = report.ended_round()
        && !note.closed
        && (round > note.reported
            // a round reported blocked that goes on in the same task and ends otherwise
            || (round == note.reported
                && note.outcome == Some(SessionOutcome::Blocked)
                && outcome != SessionOutcome::Blocked))
    {
        note.reported = round;
        note.outcome = Some(outcome);
        cmds.push(append(
            actor.clone(),
            EventBody::ChildSessionReported(ChildSessionReportedData {
                session: note.session.clone(),
                thread: note.child,
                round,
                report: outcome,
            }),
        ));
    }
    if report.state == SessionState::Closed && !note.closed {
        note.closed = true;
        cmds.push(append(
            actor.clone(),
            EventBody::ChildSessionClosed(ChildSessionClosedData {
                session: note.session.clone(),
                thread: note.child,
            }),
        ));
    }
    Ok(cmds)
}

/// The title a child thread has: its description when it is a usable title, else the helper's name
/// (the agent's own, untrusted), else a plain word.
pub fn child_title(report: &SessionReport) -> String {
    check_title(&report.description)
        .or_else(|_| check_title(&report.agent))
        .unwrap_or_else(|_| "Helper".to_owned())
}

/// The first decision of a child thread: a `queued` thread with no event, whose job knows its
/// session, and the order to follow the round the report names (a round with a task id), if any.
/// The thread's title is [`child_title`]; the model is never asked to title or describe it.
pub fn start_child(report: &SessionReport) -> (Snapshot, Vec<Command>) {
    start_child_for(report, None)
}

/// [`start_child`] for a session a person started: with their `prompt`, the child's first commit
/// holds their `user_message` before the order to follow round 1 (which the report says the person
/// started, so the round is counted in this thread).
pub fn start_child_for(
    report: &SessionReport,
    prompt: Option<PersonPrompt>,
) -> (Snapshot, Vec<Command>) {
    let round = report
        .round
        .as_ref()
        .and_then(|r| r.task_id.as_ref().map(|t| (r.number, t.clone())));
    let job = Job {
        child: Some(ChildLedger {
            session: report.session.clone(),
            agent: report.agent.clone(),
            keep_open: report.keep_open,
            state: report.state,
            closing: report.closing,
            followed: round.as_ref().map_or(0, |(n, _)| *n),
            started: 0,
            refollow: false,
            close_asked: false,
            close_cancel: false,
            track: RoundTrack::default(),
        }),
        title: TitleLedger::of(TitleSource::Model),
        // the helper's words are the agent's, fixed: nothing replaces them
        description: DescriptionLedger::of(DescriptionSource::User),
        ..Job::default()
    };
    let mut cmds: Vec<Command> = prompt
        .map(|p| {
            append(
                Actor::user(&p.user),
                EventBody::UserMessage(UserMessageData {
                    text: p.text,
                    message_id: p.message_id,
                    run_id: None,
                    origin: Origin::Agui,
                    delivery: None,
                    mentions: Vec::new(),
                    tier: p.tier,
                    tier_cap: None,
                    // a person's start of a child session names no interface locale: its request
                    // has no member for one (ADR 0085)
                    locale: None,
                }),
            )
        })
        .into_iter()
        .collect();
    let follows = round.is_some();
    cmds.extend(round.map(|(round, task_id)| Command::Observe {
        session: report.session.clone(),
        round,
        task_id,
        by: report.by,
    }));
    // A thread with nothing to follow is at rest, in a state a person can delete it in (`queued`
    // would wait for a round that never comes): idle, or closed.
    let state = if follows {
        ThreadState::Queued
    } else {
        ThreadState::Done
    };
    (Snapshot { state, job }, cmds)
}

/// Whether `report` is older news than what the ledger holds, which a stale listing can be: a
/// report of an earlier round, or of the same round with the session further back than it is
/// (working after idle or closed, a close under way undone). A report that names no round cannot
/// be told from the newest, and is not stale.
fn stale(child: &ChildLedger, report: &SessionReport) -> bool {
    let Some(r) = &report.round else {
        return false;
    };
    let rank = |s: SessionState| match s {
        SessionState::Working => 0,
        SessionState::Idle => 1,
        SessionState::Closed => 2,
    };
    match r.number.cmp(&child.track.state_round) {
        std::cmp::Ordering::Less => child.track.state_round != 0,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => {
            rank(report.state) < rank(child.state)
                || (report.state == SessionState::Working
                    && child.state == SessionState::Working
                    && child.closing
                    && !report.closing)
        }
    }
}

/// A child thread is told what its session looks like now (`Input::ChildSessionSaid`): the ledger
/// follows it and a round not yet followed is ordered followed, once. Writes no event. A thread
/// that is not this session's child changes nothing.
pub(crate) fn said(
    state: ThreadState,
    job: &mut Job,
    report: &SessionReport,
) -> Result<Vec<Command>, TransitionError> {
    report.check().map_err(malformed)?;
    let Some(child) = job.child.as_mut().filter(|c| c.session == report.session) else {
        return Ok(Vec::new());
    };
    let before = (child.state, child.closing);
    // closed is final: a late or reordered report does not reopen the session
    if child.state != SessionState::Closed && !stale(child, report) {
        child.state = report.state;
        child.closing = report.closing;
        if let Some(r) = &report.round {
            child.track.state_round = child.track.state_round.max(r.number);
        }
    }
    child.keep_open = report.keep_open;
    if !report.agent.is_empty() {
        child.agent.clone_from(&report.agent);
    }
    if state != ThreadState::Blocked {
        child.refollow = false;
    }
    let mut cmds = Vec::new();
    if (child.state, child.closing) != before {
        cmds.push(append(
            Actor::system(),
            EventBody::ChildSessionState(ChildSessionStateData {
                session: child.session.clone(),
                state: child.state,
                closing: child.closing,
                close_asked: child.close_asked,
            }),
        ));
    }
    // The round a person's close made ends the session and has nothing to show: it is marked
    // followed and never becomes a job, whatever it looks like when it is reported (working, with
    // the close under way, or ended), and also when the report comes before the answer to the
    // close. Only the round the close made is it: the answer names it, and until then it is the
    // one after the newest round known when the close was asked.
    let closing_round = child.close_asked
        && report.round.as_ref().is_some_and(|r| {
            let made = match child.track.close_round {
                0 => child.track.close_after + 1,
                named => named,
            };
            r.number == made
                && r.state
                    .and_then(report_of)
                    .is_none_or(|outcome| outcome != SessionOutcome::Blocked)
        });
    if closing_round && let Some(r) = &report.round {
        child.followed = child.followed.max(r.number);
        return Ok(cmds);
    }
    // A thread running a job of its own (a message of the person's) is waiting for the answer
    // that names the round: the report of that round is not a round to follow.
    let own_round_unnamed = matches!(state, ThreadState::Queued | ThreadState::Working)
        && job.observed.is_none()
        && !(job.number == 1 && child.started == 0);
    if own_round_unnamed {
        return Ok(cmds);
    }
    let order = report
        .round
        .as_ref()
        .filter(|r| r.number > child.followed)
        .and_then(|r| r.task_id.as_ref().map(|t| (r.number, t.clone())))
        .or_else(|| {
            // the round the thread is blocked on goes on in the same task: followed again, once
            let r = report.round.as_ref()?;
            let goes_on = state == ThreadState::Blocked
                && !child.refollow
                && r.number == child.followed
                && matches!(
                    r.state,
                    Some(AgentTaskState::Working | AgentTaskState::Submitted)
                );
            let task = r.task_id.as_ref().filter(|_| goes_on)?;
            child.refollow = true;
            Some((r.number, task.clone()))
        });
    if let Some((round, task_id)) = order {
        child.followed = child.followed.max(round);
        cmds.push(Command::Observe {
            session: report.session.clone(),
            round,
            task_id,
            by: report.by,
        });
    }
    Ok(cmds)
}

/// What a person's message to a session's thread is allowed by the session (`UserMessage`,
/// `StopAndSend`, `UiAction`): a closed session takes none, and a session that closes when its
/// round ends cannot be stopped to send (`stop`), because the stop is its end. A helper cannot ask
/// other agents, so a message that `mentions` any is refused (decision 5).
pub(crate) fn admits_message(job: &Job, stop: bool, mentions: bool) -> Result<(), TransitionError> {
    let Some(child) = &job.child else {
        return Ok(());
    };
    if child.state == SessionState::Closed {
        return Err(TransitionError::SessionClosed);
    }
    if mentions {
        return Err(TransitionError::ChildMentions);
    }
    if stop && !child.keep_open {
        return Err(TransitionError::SessionClosesOnStop);
    }
    Ok(())
}

/// What a person asked of a parent thread to start (`Input::StartSession`), borrowed.
pub(crate) struct Start<'a> {
    pub user: &'a UserId,
    pub request: &'a str,
    pub agent: &'a str,
    pub description: &'a str,
    pub text: &'a str,
    pub keep_open: bool,
    pub tier: Option<AccessTier>,
    pub tier_cap: Option<AccessTier>,
}

/// A person asks the parent thread to start a session (`Input::StartSession`): valid on a parent
/// in any state, never moves it. Writes `child_session_start_asked` (never the text) and orders
/// the start; the same request again changes nothing. Refused for a child, for a request or a
/// description out of bounds, and when [`MAX_PENDING_STARTS`] starts wait already.
pub(crate) fn start_session(
    job: &mut Job,
    start: &Start<'_>,
) -> Result<Vec<Command>, TransitionError> {
    let refuse = |why| TransitionError::ChildSessionRefused(why);
    if job.child.is_some() {
        return Err(refuse(SessionRefusal::ChildThread));
    }
    let description_ok = !start.description.trim().is_empty()
        && start.description.chars().count() <= MAX_START_DESCRIPTION_CHARS
        && !start.description.chars().any(char::is_control);
    if !printable(start.request, MAX_REQUEST_BYTES) || !description_ok {
        return Err(refuse(SessionRefusal::Malformed));
    }
    if job
        .session_starts
        .iter()
        .any(|p| p.request == start.request)
    {
        return Ok(Vec::new());
    }
    if job.session_starts.len() >= MAX_PENDING_STARTS {
        return Err(refuse(SessionRefusal::TooManyStarts));
    }
    job.session_starts.push(PendingStart {
        request: start.request.to_owned(),
    });
    Ok(vec![
        append(
            Actor::user(start.user),
            EventBody::ChildSessionStartAsked(ChildSessionStartAskedData {
                request: start.request.to_owned(),
                agent: start.agent.to_owned(),
                description: start.description.to_owned(),
                keep_open: start.keep_open,
                tier: start.tier,
            }),
        ),
        Command::StartSession {
            request: start.request.to_owned(),
            agent: start.agent.to_owned(),
            description: start.description.to_owned(),
            text: start.text.to_owned(),
            keep_open: start.keep_open,
            tier: start.tier,
            tier_cap: start.tier_cap,
        },
    ])
}

/// A start a person asked for ended (`Input::SessionStartEnded`): the pending start is dropped and
/// the outcome written once; a request nobody is waiting for changes nothing.
pub(crate) fn start_ended(
    job: &mut Job,
    request: &str,
    outcome: &SessionStartOutcome,
) -> Vec<Command> {
    let Some(at) = job.session_starts.iter().position(|p| p.request == request) else {
        return Vec::new();
    };
    job.session_starts.remove(at);
    let data = match outcome {
        SessionStartOutcome::Opened { session, child } => ChildSessionStartEndedData {
            request: request.to_owned(),
            outcome: StartOutcomeKind::Opened,
            session: Some(session.clone()),
            thread: Some(*child),
            reason: None,
        },
        SessionStartOutcome::Failed { reason } => ChildSessionStartEndedData {
            request: request.to_owned(),
            outcome: StartOutcomeKind::Failed,
            session: None,
            thread: None,
            reason: Some(truncate_to(reason, MAX_START_REASON_BYTES).to_owned()),
        },
    };
    vec![append(
        Actor::system(),
        EventBody::ChildSessionStartEnded(data),
    )]
}

/// A person asks to close the session this thread is (`Input::CloseSession`): valid in every
/// state of a child and never moves it. Nothing for a session that is closed, or for a close of
/// the same strength already asked; refused for a thread that is no child.
pub(crate) fn close_session(
    job: &mut Job,
    user: &UserId,
    cancel: bool,
) -> Result<Vec<Command>, TransitionError> {
    let child = job.child.as_mut().ok_or(TransitionError::NotAChild)?;
    if child.state == SessionState::Closed || (child.close_asked && (child.close_cancel || !cancel))
    {
        return Ok(Vec::new());
    }
    if !child.close_asked {
        child.track.close_after = child.followed.max(child.started);
    }
    child.close_asked = true;
    child.close_cancel |= cancel;
    Ok(vec![
        append(
            Actor::user(user),
            EventBody::ChildSessionCloseAsked(ChildSessionCloseAskedData {
                session: child.session.clone(),
                cancel,
            }),
        ),
        Command::CloseSession { cancel },
    ])
}

/// The agent refused the close (`Input::SessionCloseFailed`): the ask is cleared, so a person may
/// ask again, the agent's words are the child's `error` and the state is said again with no close
/// asked. Nothing when no close was asked.
pub(crate) fn close_failed(job: &mut Job, reason: &str) -> Result<Vec<Command>, TransitionError> {
    let child = job.child.as_mut().ok_or(TransitionError::NotAChild)?;
    if !child.close_asked {
        return Ok(Vec::new());
    }
    child.close_asked = false;
    child.close_cancel = false;
    Ok(vec![
        append(
            Actor::system(),
            EventBody::Error(ErrorData {
                message: truncate_to(reason, MAX_START_REASON_BYTES).to_owned(),
                retryable: false,
            }),
        ),
        // what a card reads: the session as it was, with no close asked
        append(
            Actor::system(),
            EventBody::ChildSessionState(ChildSessionStateData {
                session: child.session.clone(),
                state: child.state,
                closing: child.closing,
                close_asked: false,
            }),
        ),
    ])
}

/// The answer to a message this thread sent names round `number` (`Input::RoundSent`): the round
/// is followed, so the agent's report of it orders no `Observe`. While a job of this thread runs
/// it is that job's round (a replay of its `RoundStarted` changes nothing); otherwise it is the
/// round a close made, which is never a job. Another session's, or a thread that is no child,
/// changes nothing or is refused.
pub(crate) fn round_sent(
    state: ThreadState,
    job: &mut Job,
    session: &str,
    number: u32,
) -> Result<Vec<Command>, TransitionError> {
    let child = job.child.as_mut().ok_or(TransitionError::NotAChild)?;
    if child.session != session {
        return Ok(Vec::new());
    }
    child.followed = child.followed.max(number);
    if matches!(state, ThreadState::Queued | ThreadState::Working) {
        child.started = child.started.max(number);
    } else if child.close_asked {
        // no job of this thread is running: the round the close made
        child.track.close_round = number;
    }
    Ok(Vec::new())
}

/// The orchestrator began to follow a round of this child thread (`Input::RoundStarted`): the job
/// is the round's, under the number the agent gave it. A fresh child thread starts its first job
/// in place; one that is blocked or over starts the next job; the same round again changes nothing;
/// a thread that is busy refuses, and the dispatcher tries again later. A round at or below the
/// highest one started is a replay and changes nothing. A thread that is not `session`'s child, or
/// round 0, is refused ([`TransitionError::InvalidInState`]).
pub(crate) fn round_started(
    state: ThreadState,
    job: &mut Job,
    session: &str,
    number: u32,
    task_id: &str,
    by: Option<SessionBy>,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    let refuse = TransitionError::InvalidInState {
        state,
        input: "round started",
    };
    let Some(child) = job.child.as_ref().filter(|c| c.session == session) else {
        return Err(refuse);
    };
    if number == 0 {
        return Err(refuse);
    }
    if number <= child.started || job.observed.as_ref().is_some_and(|o| o.number == number) {
        return Ok((state, Vec::new()));
    }
    // the closing round is never a job; before the answer names it, a round that may be it waits
    // (the dispatcher asks again)
    if child.close_asked && child.track.close_round == number {
        return Ok((state, Vec::new()));
    }
    if child.close_asked && child.track.close_round == 0 && number == child.track.close_after + 1 {
        return Err(refuse);
    }
    let fresh = job.observed.is_none() && job.number == 1 && child.started == 0;
    match state {
        // a thread made at rest (nothing to follow when it was made) starts its first job in place
        ThreadState::Queued | ThreadState::Done if fresh => {}
        ThreadState::Blocked | ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
            *job = job.next();
        }
        ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => {
            return Err(TransitionError::InvalidInState {
                state,
                input: "round started",
            });
        }
    }
    job.observed = Some(ObservedRound {
        number,
        task_id: task_id.to_owned(),
        by,
    });
    if let Some(child) = job.child.as_mut() {
        child.followed = child.followed.max(number);
        child.started = number;
    }
    Ok((
        ThreadState::Working,
        vec![append(
            Actor::system(),
            EventBody::JobStarted(JobStartedData {
                job: job.number,
                round: Some(JobRound { number, by }),
            }),
        )],
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn good() -> Value {
        json!({
            "agents": [
                {"name": "planner", "description": "Breaks a request into steps."},
                {"name": "researcher", "description": ""}
            ],
            "maxOpen": 16,
            "later": "ignored"
        })
    }

    #[test]
    fn well_formed_params_are_an_offer() {
        let offer = ChildSessionsOffer::from_params(&good()).unwrap();
        assert_eq!(offer.max_open(), 16);
        let names: Vec<_> = offer.agents().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["planner", "researcher"]);
        assert_eq!(
            offer.agents()[0].description,
            "Breaks a request into steps."
        );
        assert!(offer.offers_agent("planner"));
        assert!(!offer.offers_agent("Planner"));
        assert!(!offer.offers_agent("writer"));
        // a name is matched exactly: no trimming, no case folding
        assert!(!offer.offers_agent(" planner"));
    }

    #[test]
    fn it_serialises_as_the_capabilities_document_has_it() {
        let offer = ChildSessionsOffer::from_params(&good()).unwrap();
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({
                "agents": [
                    {"name": "planner", "description": "Breaks a request into steps."},
                    {"name": "researcher", "description": ""}
                ],
                "maxOpen": 16
            })
        );
    }

    #[test]
    fn names_and_descriptions_are_kept_as_written() {
        // a name is the agent's own: not trimmed, and names that differ only in case are two names
        let offer = ChildSessionsOffer::from_params(&json!({
            "agents": [
                {"name": " planner ", "description": "  spaced  "},
                {"name": "Planner", "description": ""},
                {"name": "planner", "description": ""}
            ],
            "maxOpen": 1
        }))
        .unwrap();
        let names: Vec<_> = offer.agents().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, [" planner ", "Planner", "planner"]);
        assert_eq!(offer.agents()[0].description, "  spaced  ");
        assert_eq!(offer.max_open(), 1);
    }

    #[test]
    fn a_whole_double_is_a_whole_number() {
        let mut params = good();
        params["maxOpen"] = json!(4.0);
        assert_eq!(
            ChildSessionsOffer::from_params(&params).unwrap().max_open(),
            4
        );
        params["maxOpen"] = json!(1);
        assert_eq!(
            ChildSessionsOffer::from_params(&params).unwrap().max_open(),
            1
        );
    }

    #[test]
    fn bad_params_are_no_offer() {
        let bad: Vec<(&str, Value)> = vec![
            ("not an object", json!([])),
            ("no agents", json!({"maxOpen": 16})),
            ("agents not a list", json!({"agents": {}, "maxOpen": 16})),
            ("no agent", json!({"agents": [], "maxOpen": 16})),
            (
                "a name twice",
                json!({"agents": [{"name": "a", "description": ""}, {"name": "a", "description": "x"}], "maxOpen": 16}),
            ),
            (
                "a blank name",
                json!({"agents": [{"name": " ", "description": ""}], "maxOpen": 16}),
            ),
            (
                "a name that is not text",
                json!({"agents": [{"name": 3, "description": ""}], "maxOpen": 16}),
            ),
            (
                "no description",
                json!({"agents": [{"name": "a"}], "maxOpen": 16}),
            ),
            (
                "an entry that is not an object",
                json!({"agents": ["a"], "maxOpen": 16}),
            ),
            (
                "no maxOpen",
                json!({"agents": [{"name": "a", "description": ""}]}),
            ),
            (
                "maxOpen zero",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 0}),
            ),
            (
                "maxOpen negative",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": -1}),
            ),
            (
                "maxOpen a fraction",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 1.5}),
            ),
            (
                "maxOpen as text",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": "16"}),
            ),
            (
                "maxOpen too large",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 4_294_967_296_u64}),
            ),
        ];
        for (why, params) in bad {
            assert_eq!(ChildSessionsOffer::from_params(&params), None, "{why}");
        }
    }
}
