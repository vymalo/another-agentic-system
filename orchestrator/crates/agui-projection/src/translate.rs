//! `RunAgentInput` → core inputs (`docs/api/agui.md`, "Inbound").
//!
//! The consumer sends the transcript it holds and what it wants done; the orchestrator owns the
//! history. [`translate`] reconciles the two by message id, works out what is new, and says what
//! to do about it: apply one input, attach to a run that already exists, or refuse before the
//! stream starts ([`InputError`], which knows the HTTP status the surface answers with).
//!
//! It is a pure function of the request and a [`ThreadView`]. Deciding whether the thread exists
//! and whether the caller owns it (404) is the surface's job, before it builds the view.

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_proto::{ContentPart, Message, MessageContent, ResumeStatus, RunAgentInput};
use orch_core::{
    AgentId, Delivery, Input, Origin, ThreadId, ThreadState, UiActionData, UiVersion, UserId,
};
use serde_json::{Map, Value};

use crate::vocab::{RELEASE_CHANNELS_URI, SEND_KEY};

/// What [`translate`] needs to know about the thread the request names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadView {
    /// No thread has this id. The consumer minted it, and this request creates the thread.
    New {
        /// The identity the edge vouched for: the owner-to-be.
        user: UserId,
    },
    /// The thread exists and belongs to the caller.
    Known(KnownThread),
}

/// An existing thread, as far as the projection sees it. Build it with
/// [`Projector::view`](crate::Projector::view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownThread {
    /// The caller.
    pub user: UserId,
    /// The agent the thread targets.
    pub agent: AgentId,
    /// The state the log implies.
    pub state: ThreadState,
    /// A run is open (the thread is `queued` or `working`).
    pub run_open: bool,
    /// The interrupts a `resume` can answer: one while the thread is blocked on the agent.
    pub open_interrupts: Vec<String>,
    /// Every message id the log already holds (user, agent and activity messages).
    pub message_ids: BTreeSet<String>,
    /// The run each user message of the log opened, by message id (ADR 0082): a resend of that
    /// message, whatever `runId` it carries, attaches to the run the log says it belongs to.
    pub message_runs: BTreeMap<String, String>,
    /// The words each user message of the log was sent with, by message id (ADR 0082): an id is
    /// bound to its text, so a request that repeats the id with other words is refused.
    pub message_texts: BTreeMap<String, String>,
    /// Every run id the log already holds.
    pub run_ids: BTreeSet<String>,
    /// The A2UI surfaces the thread has now, with the dialect each one speaks. An action is
    /// accepted only for one of these, and only when it is [`SurfaceDialect::V1_0`].
    pub surfaces: BTreeMap<String, SurfaceDialect>,
}

/// The A2UI dialect the operations of a surface are written in, as far as the orchestrator cares
/// (ADR 0075, a hard cutover).
///
/// A thread from before the cutover keeps its `v0.9.1` surfaces in the log, and nothing is
/// migrated: they are relayed to a viewer as they were sent (which draws them as made by an
/// earlier version), and an action on one is refused with 422 before anything is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDialect {
    /// A2UI `"1.0"`, the only dialect this build speaks.
    V1_0,
    /// An operation of any other `version` — a surface of a thread from before the cutover.
    Earlier,
}

impl ThreadView {
    /// The view of a thread that does not exist yet.
    pub fn new_thread(user: UserId) -> Self {
        ThreadView::New { user }
    }

    fn user(&self) -> &UserId {
        match self {
            ThreadView::New { user } => user,
            ThreadView::Known(k) => &k.user,
        }
    }

    /// Refuses a request whose URL names another agent than the thread's target (409). A new
    /// thread targets whatever the URL names.
    pub fn ensure_agent(&self, requested: &AgentId) -> Result<(), InputError> {
        match self {
            ThreadView::Known(k) if &k.agent != requested => Err(InputError::WrongAgent {
                thread: k.agent.to_string(),
                requested: requested.to_string(),
            }),
            ThreadView::Known(_) | ThreadView::New { .. } => Ok(()),
        }
    }
}

/// Why a request is refused before the stream starts. Each maps to an RFC 9457 problem; nothing
/// has been streamed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// `threadId` is not a UUID (400).
    #[error("threadId must be a UUID, got {got:?}")]
    ThreadIdNotUuid {
        /// What was sent.
        got: String,
    },
    /// `protocolVersion` names another major than 1, or is not `MAJOR.MINOR` (400).
    #[error("unsupported protocolVersion {got:?}: this server speaks AG-UI 1.x")]
    ProtocolVersion {
        /// What was sent.
        got: String,
    },
    /// The request asks for nothing: no new message, no answer, and a run that was never
    /// started (422).
    #[error(
        "nothing to run: no new message, no resume, and runId {run_id:?} is not a run of this thread"
    )]
    NothingToRun {
        /// The request's `runId`.
        run_id: String,
    },
    /// A message id the log holds was sent with other words (422, `message_id_reused`). An id is
    /// bound to the text it was first written with (ADR 0082, C4): a client that edits a draft
    /// mints a new id, because a resend repeats the same bytes. Accepting another text would make
    /// the id meaningless.
    #[error(
        "message {message_id:?} is already in this thread with other words; a message id is bound \
         to the text it was sent with, so an edited message needs a new id"
    )]
    MessageIdReused {
        /// The id the request repeated.
        message_id: String,
    },
    /// More than one message is new (422): the orchestrator owns the history, one message at a
    /// time.
    #[error("{count} messages are new; send one at a time")]
    TooManyNewMessages {
        /// How many.
        count: usize,
    },
    /// A message that is not a user message is new (422): only the orchestrator writes the rest.
    #[error(
        "message {id:?} has role {role:?} and is not in this thread; only user messages can be sent"
    )]
    NewNonUserMessage {
        /// The message's id.
        id: String,
        /// Its role.
        role: &'static str,
    },
    /// A new user message has no text (422).
    #[error("message {id:?} has no text")]
    EmptyMessage {
        /// The message's id.
        id: String,
    },
    /// A resolved `resume` entry carries no usable answer (422): expected `{"text": "…"}` or a
    /// string.
    #[error(
        "resume entry for interrupt {interrupt_id:?} needs a payload of the form {{\"text\": \"…\"}}"
    )]
    InvalidResumePayload {
        /// The interrupt.
        interrupt_id: String,
    },
    /// An interrupt is answered by `resume` and a new user message says something else (422).
    #[error(
        "the request answers an interrupt with `resume` and also sends a new message; send one"
    )]
    AmbiguousAnswer,
    /// The request starts a new run under a `runId` the thread already used (422): a run id is
    /// never reused on a thread.
    #[error("runId {run_id:?} was already used on this thread; a run id is never reused")]
    RunIdReused {
        /// The request's `runId`.
        run_id: String,
    },
    /// A run is already open on the thread (409), and the request does not say how its message
    /// is to be delivered (`forwardedProps["vymalo.send"]`, ADR 0036), or it is not a message.
    #[error(
        "a run is already open on this thread; wait for it to finish, or send a message with \
         forwardedProps[\"vymalo.send\"] set to \"steer\" or \"interrupt\""
    )]
    RunInProgress,
    /// `forwardedProps["vymalo.send"]` is not `"steer"` or `"interrupt"` (400).
    #[error("forwardedProps[\"vymalo.send\"] must be \"steer\" or \"interrupt\", got {got}")]
    InvalidSend {
        /// What was sent, as JSON, cut to a short excerpt.
        got: String,
    },
    /// An action on a finished thread (409): the card belongs to a request that ended. A message
    /// is not refused: it starts the thread's next job (ADR 0020). (A stop has nothing to stop on
    /// a finished thread, which holds no interrupt: that is `NothingToRun`.)
    #[error(
        "this card belongs to a finished request ({state:?}); write a message to start the next one"
    )]
    ThreadFinished {
        /// The terminal state.
        state: ThreadState,
    },
    /// `forwardedProps.a2uiAction` is not an action: no `userAction` object, or a member of the
    /// wrong type (422).
    #[error("forwardedProps.a2uiAction is not a valid action: {why}")]
    InvalidAction {
        /// What is wrong.
        why: String,
    },
    /// The action is larger than the limits allow (413).
    #[error("the action is too large: {why}")]
    ActionTooLarge {
        /// Which limit.
        why: String,
    },
    /// The action names a surface the thread does not have (422): it never had it, or the agent
    /// deleted it. Nothing is sent to the agent.
    #[error("this thread has no surface {surface_id:?}")]
    UnknownSurface {
        /// The surface the action named, cut to a short excerpt.
        surface_id: String,
    },
    /// The action names a surface whose operations are not A2UI `"1.0"` (422): a surface of a
    /// thread from before the cutover, shown by the client as made by an earlier version. Nothing
    /// is written and nothing is sent to the agent (ADR 0075, decision 4).
    #[error(
        "the surface {surface_id:?} was made by an earlier version of A2UI and cannot be acted on; \
         ask the agent again"
    )]
    EarlierSurface {
        /// The surface the action named, cut to a short excerpt.
        surface_id: String,
    },
    /// The request carries an action and also a new message, an answer or a cancellation (422):
    /// one thing at a time.
    #[error(
        "the request carries an A2UI action and also a message, an answer or a cancellation; send one"
    )]
    AmbiguousAction,
    /// The URL names another agent than the thread's target (409).
    #[error("the thread targets agent {thread:?}, not {requested:?}")]
    WrongAgent {
        /// The thread's agent.
        thread: String,
        /// The URL's agent.
        requested: String,
    },
}

impl InputError {
    /// The HTTP status of the problem response: 400, 409 or 422.
    pub fn http_status(&self) -> u16 {
        match self {
            InputError::ThreadIdNotUuid { .. }
            | InputError::ProtocolVersion { .. }
            | InputError::InvalidSend { .. } => 400,
            InputError::RunInProgress
            | InputError::ThreadFinished { .. }
            | InputError::WrongAgent { .. } => 409,
            InputError::NothingToRun { .. }
            | InputError::TooManyNewMessages { .. }
            | InputError::NewNonUserMessage { .. }
            | InputError::EmptyMessage { .. }
            | InputError::MessageIdReused { .. }
            | InputError::InvalidResumePayload { .. }
            | InputError::RunIdReused { .. }
            | InputError::InvalidAction { .. }
            | InputError::UnknownSurface { .. }
            | InputError::EarlierSurface { .. }
            | InputError::AmbiguousAction
            | InputError::AmbiguousAnswer => 422,
            InputError::ActionTooLarge { .. } => 413,
        }
    }

    /// The stable, machine-readable `code` of the problem the surface answers with, where a client
    /// acts on it (the RFC 9457 extension member `code` that `orch_api::Problem` carries). `None`
    /// for the refusals that are only a status and a sentence: their bodies are what they always
    /// were.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            InputError::MessageIdReused { .. } => Some("message_id_reused"),
            InputError::ThreadIdNotUuid { .. }
            | InputError::ProtocolVersion { .. }
            | InputError::NothingToRun { .. }
            | InputError::TooManyNewMessages { .. }
            | InputError::NewNonUserMessage { .. }
            | InputError::EmptyMessage { .. }
            | InputError::InvalidResumePayload { .. }
            | InputError::AmbiguousAnswer
            | InputError::RunIdReused { .. }
            | InputError::RunInProgress
            | InputError::InvalidSend { .. }
            | InputError::ThreadFinished { .. }
            | InputError::InvalidAction { .. }
            | InputError::ActionTooLarge { .. }
            | InputError::UnknownSurface { .. }
            | InputError::EarlierSurface { .. }
            | InputError::AmbiguousAction
            | InputError::WrongAgent { .. } => None,
        }
    }
}

/// Something in the request that was ignored, or served with a caveat. The run is not refused;
/// the surface logs each.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Warning {
    /// The consumer speaks a newer minor version of AG-UI 1 (served as 1.0).
    #[error("protocolVersion {got:?} is newer than 1.0; served as 1.0")]
    NewerMinorVersion {
        /// What was sent.
        got: String,
    },
    /// Frontend tools are ignored: the orchestrator runs no model in the browser's loop.
    #[error("{count} frontend tools ignored")]
    ToolsIgnored {
        /// How many were sent.
        count: usize,
    },
    /// Context entries are ignored for now (open question 18).
    #[error("{count} context entries ignored")]
    ContextIgnored {
        /// How many were sent.
        count: usize,
    },
    /// Parts of a user message that are not text were skipped.
    #[error("message {id:?}: {count} non-text parts skipped")]
    NonTextPartsSkipped {
        /// The message.
        id: String,
        /// How many.
        count: usize,
    },
    /// A `resume` entry was ignored: the thread is not blocked, or the id is not open.
    #[error("resume entry for interrupt {interrupt_id:?} ignored: {why}")]
    ResumeIgnored {
        /// The interrupt named.
        interrupt_id: String,
        /// Why.
        why: &'static str,
    },
}

/// The result of translating a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    /// What to apply, in order. Empty means **attach**: nothing is new and the run named by the
    /// request already exists, so stream it from its start (an idempotent retry).
    pub inputs: Vec<Input>,
    /// What was ignored on the way.
    pub warnings: Vec<Warning>,
    /// The run to attach to, when `inputs` is empty and it is not the request's own `runId`: the run
    /// the log says the resent message opened (ADR 0082, decision 5). `None` means the request's
    /// `runId`, which is the run of a retry that repeats it.
    pub attach: Option<String>,
}

/// The thread id of a request, as a core id. Not a UUID is a 400.
pub fn thread_id_of(input: &RunAgentInput) -> Result<ThreadId, InputError> {
    input
        .thread_id
        .as_str()
        .parse::<ThreadId>()
        .map_err(|_| InputError::ThreadIdNotUuid {
            got: input.thread_id.to_string(),
        })
}

/// The release the consumer selects for a new thread:
/// `forwardedProps[<release-channels URI>].release` (ADR 0008). The surface validates it against
/// the live agent card and fails closed.
pub fn release_selector(input: &RunAgentInput) -> Option<&str> {
    input
        .forwarded_props
        .as_ref()?
        .get(RELEASE_CHANNELS_URI)?
        .get("release")?
        .as_str()
}

/// The ids of the messages in the request: what the consumer already holds, so the requester's
/// projection does not send them back.
pub fn held_message_ids(input: &RunAgentInput) -> BTreeSet<String> {
    input
        .messages
        .iter()
        .map(|m| m.id().as_str().to_owned())
        .collect()
}

/// [`translate_with_warnings`], without the warnings.
pub fn translate(input: &RunAgentInput, thread: &ThreadView) -> Result<Vec<Input>, InputError> {
    translate_with_warnings(input, thread).map(|t| t.inputs)
}

/// Works out what a request asks of the thread.
///
/// | Request | Result |
/// |---|---|
/// | one new user message | `[UserMessage]`, with the message's id and the run id recorded |
/// | messages already in the thread | ignored (reconciliation by id) |
/// | `resume` resolved on a blocked thread | `[UserMessage{payload.text}]`, run id recorded |
/// | `resume` cancelled, no new message | `[Cancel]` |
/// | `resume` cancelled and a new user message | `[UserMessage]` |
/// | nothing new, `runId` recorded | `[]`: attach |
/// | nothing new, a resent message the log holds | `[]`: attach to the run that message opened, whatever `runId` the request carries (ADR 0082) |
/// | one new user message and `vymalo.send: "interrupt"`, thread exists | `[StopAndSend]` |
///
/// and the refusals of [`InputError`]. A thread that is finished takes only a message (it starts
/// the next job), and one that has a run open takes only a message that says how it is delivered,
/// `forwardedProps["vymalo.send"]` (ADR 0036): `"steer"` is the plain message.
pub fn translate_with_warnings(
    input: &RunAgentInput,
    thread: &ThreadView,
) -> Result<Translation, InputError> {
    let mut warnings = Vec::new();
    check_version(input, &mut warnings)?;
    if !input.tools().is_empty() {
        warnings.push(Warning::ToolsIgnored {
            count: input.tools().len(),
        });
    }
    if !input.context().is_empty() {
        warnings.push(Warning::ContextIgnored {
            count: input.context().len(),
        });
    }
    let action = user_action(input)?;
    // Read on every run, so a malformed one is refused every time, whatever the thread is doing.
    let send = send_mode(input)?;

    let known = match thread {
        ThreadView::Known(k) => Some(k),
        ThreadView::New { .. } => None,
    };

    // An id is bound to the text it was first written with (ADR 0082, C4): a resend repeats the
    // same bytes, so a known id sent with other words is a second message wearing the first one's
    // id, refused before anything is written. Both ways this function reconciles by id — the drop
    // below and an attach — pass through here, so neither discards the new words silently.
    if let Some(k) = known {
        for message in &input.messages {
            let Message::User(u) = message else {
                continue;
            };
            let Some(logged) = k.message_texts.get(u.id.as_str()) else {
                continue;
            };
            let (text, _) = user_text(&u.content);
            if text != *logged {
                return Err(InputError::MessageIdReused {
                    message_id: u.id.to_string(),
                });
            }
        }
    }

    // Reconcile by id: what the thread already holds is not new, whatever its role.
    let mut new_users = Vec::new();
    for message in &input.messages {
        if known.is_some_and(|k| k.message_ids.contains(message.id().as_str())) {
            continue;
        }
        match message {
            Message::User(u) => new_users.push(u),
            other => {
                return Err(InputError::NewNonUserMessage {
                    id: other.id().to_string(),
                    role: other.role(),
                });
            }
        }
    }
    if new_users.len() > 1 {
        return Err(InputError::TooManyNewMessages {
            count: new_users.len(),
        });
    }
    let new_user = match new_users.first() {
        Some(u) => {
            let (text, skipped) = user_text(&u.content);
            if skipped > 0 {
                warnings.push(Warning::NonTextPartsSkipped {
                    id: u.id.to_string(),
                    count: skipped,
                });
            }
            if text.trim().is_empty() {
                return Err(InputError::EmptyMessage {
                    id: u.id.to_string(),
                });
            }
            Some((u.id.to_string(), text))
        }
        None => None,
    };

    // Resume: answers to the interrupt the thread is blocked on.
    let mut answer: Option<String> = None;
    let mut cancelled = false;
    for entry in input.resume() {
        let id = entry.interrupt_id.as_str();
        let why = match known {
            None => Some("the thread does not exist yet"),
            Some(k) if k.state != ThreadState::Blocked => Some("the thread is not blocked"),
            Some(k) if !k.open_interrupts.iter().any(|open| open == id) => {
                Some("no such open interrupt")
            }
            Some(_) => None,
        };
        if let Some(why) = why {
            warnings.push(Warning::ResumeIgnored {
                interrupt_id: id.to_owned(),
                why,
            });
            continue;
        }
        match entry.status {
            ResumeStatus::Cancelled => cancelled = true,
            ResumeStatus::Resolved => {
                let text = entry
                    .payload
                    .as_ref()
                    .and_then(payload_text)
                    .ok_or_else(|| InputError::InvalidResumePayload {
                        interrupt_id: id.to_owned(),
                    })?;
                answer.get_or_insert(text);
            }
        }
    }

    let user = thread.user().clone();
    let run_id = Some(input.run_id.to_string());
    if let Some(mut action) = action {
        if answer.is_some() || new_user.is_some() || cancelled {
            return Err(InputError::AmbiguousAction);
        }
        // Only a surface the thread has: nothing an agent did not send can be acted on.
        let dialect = known
            .and_then(|k| k.surfaces.get(&action.surface_id))
            .ok_or_else(|| InputError::UnknownSurface {
                surface_id: action.surface_id.chars().take(64).collect(),
            })?;
        // And only one of this build's dialect: a surface from before the cutover is shown, never
        // acted on (ADR 0075, decision 4).
        if *dialect == SurfaceDialect::Earlier {
            return Err(InputError::EarlierSurface {
                surface_id: action.surface_id.chars().take(64).collect(),
            });
        }
        action.version = UiVersion::V1_0;
        action.run_id = run_id;
        return finish(
            vec![Input::UiAction {
                tier_cap: None,
                user,
                action,
                catalog: None,
                tier: None,
            }],
            input,
            known,
            None,
            warnings,
        );
    }
    let inputs = match (answer, new_user, cancelled) {
        (Some(_), Some(_), _) => return Err(InputError::AmbiguousAnswer),
        (Some(text), None, _) => vec![Input::UserMessage {
            tier_cap: None,
            user,
            text,
            message_id: None,
            run_id,
            origin: Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
            tier: None,
        }],
        // Stop & send (ADR 0036): on a thread that exists, where the core knows what to stop
        // (and treats it as a plain message when nothing runs). A new thread has nothing to stop.
        (None, Some((id, text)), _) if send == Some(Delivery::Interrupt) && known.is_some() => {
            vec![Input::StopAndSend {
                tier_cap: None,
                user,
                text,
                message_id: Some(id),
                run_id,
                origin: Origin::Agui,
                catalog: None,
                mentions: Vec::new(),
                tier: None,
            }]
        }
        (None, Some((id, text)), _) => vec![Input::UserMessage {
            tier_cap: None,
            user,
            text,
            message_id: Some(id),
            run_id,
            origin: Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
            tier: None,
        }],
        (None, None, true) => vec![Input::Cancel { user }],
        (None, None, false) => {
            return match known {
                Some(k) if k.run_ids.contains(input.run_id.as_str()) => Ok(Translation {
                    inputs: Vec::new(),
                    warnings,
                    attach: None,
                }),
                // A resend of a message the log holds, under a `runId` of its own: it attaches to
                // the run that message opened (ADR 0082, decision 5). Five sends of one message are
                // one message, one run and one charge, and the person is shown the answer instead
                // of a refusal. The message is the last of the request that the log has: the one
                // the client is sending again.
                Some(k) => match last_opened_run(input, k) {
                    Some(run) => Ok(Translation {
                        inputs: Vec::new(),
                        warnings,
                        attach: Some(run),
                    }),
                    None => Err(InputError::NothingToRun {
                        run_id: input.run_id.to_string(),
                    }),
                },
                None => Err(InputError::NothingToRun {
                    run_id: input.run_id.to_string(),
                }),
            };
        }
    };

    finish(inputs, input, known, send, warnings)
}

/// The run the last message of `input` that the log holds opened (ADR 0082): `None` when the request
/// carries no message the log's `user_message` events name a run for.
fn last_opened_run(input: &RunAgentInput, known: &KnownThread) -> Option<String> {
    input
        .messages
        .iter()
        .filter_map(|message| known.message_runs.get(message.id().as_str()))
        .next_back()
        .cloned()
}

/// Something is to be applied: a thread that cannot take it says so before the stream.
///
/// A thread with a run open takes a **message** that says how it is delivered (`send`, ADR
/// 0036); anything else is refused until the run is over.
fn finish(
    inputs: Vec<Input>,
    input: &RunAgentInput,
    known: Option<&KnownThread>,
    send: Option<Delivery>,
    warnings: Vec<Warning>,
) -> Result<Translation, InputError> {
    if let Some(k) = known {
        let is_message =
            |i: &Input| matches!(i, Input::UserMessage { .. } | Input::StopAndSend { .. });
        let starts_a_run = inputs
            .iter()
            .any(|i| is_message(i) || matches!(i, Input::UiAction { .. }));
        if starts_a_run && k.run_ids.contains(input.run_id.as_str()) {
            return Err(InputError::RunIdReused {
                run_id: input.run_id.to_string(),
            });
        }
        // A message on a finished thread starts its next job (ADR 0020); an action belongs to
        // the finished job.
        if k.state.is_terminal() && !inputs.iter().all(is_message) {
            return Err(InputError::ThreadFinished { state: k.state });
        }
        if k.run_open
            && !(send.is_some() && matches!(inputs.as_slice(), [only] if is_message(only)))
        {
            return Err(InputError::RunInProgress);
        }
    }
    Ok(Translation {
        inputs,
        warnings,
        attach: None,
    })
}

/// How the run says its message is delivered, `forwardedProps["vymalo.send"]` (ADR 0036):
/// `"steer"` (Send) or `"interrupt"` (Stop & send). `null` and no member are none; anything
/// else is a 400, whatever the thread is doing.
fn send_mode(input: &RunAgentInput) -> Result<Option<Delivery>, InputError> {
    let Some(value) = input
        .forwarded_props
        .as_ref()
        .and_then(|props| props.get(SEND_KEY))
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    match value.as_str() {
        Some("steer") => Ok(Some(Delivery::Steer)),
        Some("interrupt") => Ok(Some(Delivery::Interrupt)),
        _ => Err(InputError::InvalidSend {
            got: value.to_string().chars().take(64).collect(),
        }),
    }
}

/// The action a request carries, `forwardedProps.a2uiAction.userAction` (the convention of
/// `@ag-ui/a2ui-middleware` and `useAgUiSendA2uiAction`, *verified 2026-09-29*), checked for
/// shape and size. Whether the surface exists is decided by the caller with the thread's log.
///
/// `name`, `surfaceId` and `sourceComponentId` are required (A2UI's `action` message requires
/// them); `context` is an object and defaults to empty; every other member (`timestamp`,
/// `userMessage`, `type`) is dropped: the agent gets the log's own time, and text the agent wrote
/// is not ours to forward.
fn user_action(input: &RunAgentInput) -> Result<Option<UiActionData>, InputError> {
    let Some(props) = input.forwarded_props.as_ref() else {
        return Ok(None);
    };
    let Some(envelope) = props.get("a2uiAction") else {
        return Ok(None);
    };
    let invalid = |why: &str| InputError::InvalidAction {
        why: why.to_owned(),
    };
    let user_action = envelope
        .get("userAction")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("expected {\"userAction\": {...}}"))?;
    let text = |key: &str| -> Result<String, InputError> {
        user_action
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| invalid(&format!("{key} must be a string")))
    };
    let context = match user_action.get("context") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(invalid("context must be an object")),
    };
    let action = UiActionData {
        tier: None,
        tier_cap: None,
        surface_id: text("surfaceId")?,
        name: text("name")?,
        source_component_id: text("sourceComponentId")?,
        context,
        // Set once the surface is known: A2UI v1.0, the only dialect (ADR 0075), and the request's
        // run id.
        version: UiVersion::V1_0,
        run_id: None,
    };
    action.check().map_err(|e| {
        if e.is_oversized() {
            InputError::ActionTooLarge { why: e.to_string() }
        } else {
            InputError::InvalidAction { why: e.to_string() }
        }
    })?;
    Ok(Some(action))
}

/// The protocol version check: another major is refused, a newer minor is served with a warning.
fn check_version(input: &RunAgentInput, warnings: &mut Vec<Warning>) -> Result<(), InputError> {
    let Some(got) = input.protocol_version.as_deref() else {
        return Ok(());
    };
    let bad = || InputError::ProtocolVersion {
        got: got.to_owned(),
    };
    let (major, minor) = got.split_once('.').ok_or_else(bad)?;
    let major: u32 = major.parse().map_err(|_| bad())?;
    let minor: u32 = minor.parse().map_err(|_| bad())?;
    if major != 1 {
        return Err(bad());
    }
    if minor > 0 {
        warnings.push(Warning::NewerMinorVersion {
            got: got.to_owned(),
        });
    }
    Ok(())
}

/// The text of a user message, and how many parts that are not text were skipped.
fn user_text(content: &MessageContent) -> (String, usize) {
    match content {
        MessageContent::Text(text) => (text.clone(), 0),
        MessageContent::Parts(parts) => {
            let mut texts = Vec::new();
            let mut skipped = 0;
            for part in parts {
                match part {
                    ContentPart::Text { text, .. } => texts.push(text.as_str()),
                    ContentPart::Image { .. }
                    | ContentPart::Audio { .. }
                    | ContentPart::Video { .. }
                    | ContentPart::Document { .. } => skipped += 1,
                }
            }
            (texts.join("\n"), skipped)
        }
    }
}

/// The answer in a resume payload: `{"text": "…"}`, or a bare string.
fn payload_text(payload: &Value) -> Option<String> {
    match payload {
        Value::String(text) => Some(text.clone()),
        Value::Object(map) => map.get("text")?.as_str().map(str::to_owned),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
    }
    .filter(|text| !text.trim().is_empty())
}
