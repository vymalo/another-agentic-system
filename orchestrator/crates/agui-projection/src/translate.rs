//! `RunAgentInput` → core inputs (`docs/api/agui.md`, "Inbound").
//!
//! The consumer sends the transcript it holds and what it wants done; the orchestrator owns the
//! history. [`translate`] reconciles the two by message id, works out what is new, and says what
//! to do about it: apply one input, attach to a run that already exists, or refuse before the
//! stream starts ([`InputError`], which knows the HTTP status the surface answers with).
//!
//! It is a pure function of the request and a [`ThreadView`]. Deciding whether the thread exists
//! and whether the caller owns it (404) is the surface's job, before it builds the view.

use std::collections::BTreeSet;

use orch_agui_proto::{ContentPart, Message, MessageContent, ResumeStatus, RunAgentInput};
use orch_core::{AgentId, Input, ThreadId, ThreadState, UserId};
use serde_json::Value;

use crate::vocab::RELEASE_CHANNELS_URI;

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
    /// Every run id the log already holds.
    pub run_ids: BTreeSet<String>,
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
    /// A run is already open on the thread (409).
    #[error("a run is already open on this thread; wait for it to finish")]
    RunInProgress,
    /// The thread is finished (409).
    #[error("the thread is finished ({state:?}); start a new thread")]
    ThreadFinished {
        /// The terminal state.
        state: ThreadState,
    },
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
            InputError::ThreadIdNotUuid { .. } | InputError::ProtocolVersion { .. } => 400,
            InputError::RunInProgress
            | InputError::ThreadFinished { .. }
            | InputError::WrongAgent { .. } => 409,
            InputError::NothingToRun { .. }
            | InputError::TooManyNewMessages { .. }
            | InputError::NewNonUserMessage { .. }
            | InputError::EmptyMessage { .. }
            | InputError::InvalidResumePayload { .. }
            | InputError::RunIdReused { .. }
            | InputError::AmbiguousAnswer => 422,
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
    /// `forwardedProps.a2uiAction` is not handled yet (generative UI arrives with ADR 0013).
    #[error("forwardedProps.a2uiAction ignored: generative UI actions are not supported yet")]
    A2uiActionIgnored,
}

/// The result of translating a request.
#[derive(Debug, Clone, PartialEq)]
pub struct Translation {
    /// What to apply, in order. Empty means **attach**: nothing is new and the run named by the
    /// request already exists, so stream it from its start (an idempotent retry).
    pub inputs: Vec<Input>,
    /// What was ignored on the way.
    pub warnings: Vec<Warning>,
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
///
/// and the refusals of [`InputError`]. A thread that is finished, or has a run open, takes no new
/// input.
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
    if input
        .forwarded_props
        .as_ref()
        .is_some_and(|p| p.get("a2uiAction").is_some())
    {
        warnings.push(Warning::A2uiActionIgnored);
    }

    let known = match thread {
        ThreadView::Known(k) => Some(k),
        ThreadView::New { .. } => None,
    };

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
    let inputs = match (answer, new_user, cancelled) {
        (Some(_), Some(_), _) => return Err(InputError::AmbiguousAnswer),
        (Some(text), None, _) => vec![Input::UserMessage {
            user,
            text,
            message_id: None,
            run_id,
        }],
        (None, Some((id, text)), _) => vec![Input::UserMessage {
            user,
            text,
            message_id: Some(id),
            run_id,
        }],
        (None, None, true) => vec![Input::Cancel { user }],
        (None, None, false) => {
            return match known {
                Some(k) if k.run_ids.contains(input.run_id.as_str()) => Ok(Translation {
                    inputs: Vec::new(),
                    warnings,
                }),
                Some(_) | None => Err(InputError::NothingToRun {
                    run_id: input.run_id.to_string(),
                }),
            };
        }
    };

    // Something is to be applied: a thread that cannot take it says so before the stream.
    if let Some(k) = known {
        let starts_a_run = inputs
            .iter()
            .any(|i| matches!(i, Input::UserMessage { .. }));
        if starts_a_run && k.run_ids.contains(input.run_id.as_str()) {
            return Err(InputError::RunIdReused {
                run_id: input.run_id.to_string(),
            });
        }
        if k.state.is_terminal() {
            return Err(InputError::ThreadFinished { state: k.state });
        }
        if k.run_open {
            return Err(InputError::RunInProgress);
        }
    }
    Ok(Translation { inputs, warnings })
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
