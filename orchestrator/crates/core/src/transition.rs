//! The pure state machine: `(state, input) -> (next state, commands)`.
//!
//! Every `match` over [`ThreadState`], [`Input`], [`AgentUpdate`] and
//! [`AgentTaskState`] is exhaustive with no wildcard arm, so adding a variant
//! makes the compiler point at every decision that must be revisited (ADR 0004).
//!
//! Rule for `thread_state` events: one is appended only when the thread *enters*
//! `blocked`, `done`, `failed` or `cancelled`. Entering `queued`/`working` is implied
//! by `user_message` / `agent_status` and visible through `Thread.state`.

use crate::agent::{AgentTaskState, AgentUpdate};
use crate::error::{Classify, ErrorClass};
use crate::event::{
    Actor, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData, EventBody,
    ThreadStateData, UserMessageData,
};
use crate::ids::{AgentId, UserId};
use crate::thread::ThreadState;

/// Everything that can happen to a thread, already translated to protocol-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The user wrote a message (first message, follow-up, or answer to a blocked job).
    UserMessage {
        /// Author.
        user: UserId,
        /// Text.
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
}

impl Input {
    /// A short static name, for errors and logs.
    pub fn name(&self) -> &'static str {
        match self {
            Input::UserMessage { .. } => "user message",
            Input::Cancel { .. } => "cancel",
            Input::Agent { .. } => "agent update",
            Input::DeliveryFailed { .. } => "delivery failure",
            Input::CancelledBeforeStart => "cancelled before start",
            Input::CancelRejected { .. } => "cancel rejection",
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
    },
    /// Ask the agent to cancel the running task (outbox kind `cancel`).
    RequestCancel,
}

/// An input that is not valid in the current state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransitionError {
    /// The thread is finished; the user must start a new one.
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

fn append(actor: Actor, body: EventBody) -> Command {
    Command::Append(EventDraft { actor, body })
}

fn entered(state: ThreadState) -> Command {
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

fn user_message(user: &UserId, text: &str) -> Vec<Command> {
    vec![
        append(
            Actor::user(user),
            EventBody::UserMessage(UserMessageData {
                text: text.to_owned(),
            }),
        ),
        Command::Delegate {
            text: text.to_owned(),
        },
    ]
}

/// Decides the next state and the commands for `input` in `state`. Pure: no I/O, no clock.
pub fn transition(
    state: &ThreadState,
    input: &Input,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
    let state = *state;
    match input {
        Input::UserMessage { user, text } => match state {
            ThreadState::Queued | ThreadState::Working => Ok((state, user_message(user, text))),
            ThreadState::Blocked => Ok((ThreadState::Queued, user_message(user, text))),
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                Err(TransitionError::Finished { state })
            }
        },
        Input::Cancel { .. } => match state {
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                Ok((state, vec![Command::RequestCancel]))
            }
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::Agent {
            agent,
            revision,
            update,
        } => agent_input(state, Actor::agent(agent, revision.clone()), update, input),
        Input::DeliveryFailed { reason, retryable } => match state {
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                if *retryable {
                    let mut cmds = vec![error_event(reason, true)];
                    match state {
                        ThreadState::Blocked => {}
                        ThreadState::Queued | ThreadState::Working => {
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
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => Ok((
                ThreadState::Cancelled,
                vec![entered(ThreadState::Cancelled)],
            )),
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
        Input::CancelRejected { reason, retryable } => match state {
            ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {
                Ok((state, vec![error_event(reason, *retryable)]))
            }
            ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Ok((state, vec![])),
        },
    }
}

fn agent_input(
    state: ThreadState,
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
        ThreadState::Queued | ThreadState::Working | ThreadState::Blocked => {}
    }
    match update {
        AgentUpdate::Artifact {
            name,
            mime_type,
            uri,
            text,
        } => Ok((
            state,
            vec![append(
                actor,
                EventBody::Artifact(ArtifactData {
                    name: name.clone(),
                    mime_type: mime_type.clone(),
                    uri: uri.clone(),
                    text: text.clone(),
                }),
            )],
        )),
        AgentUpdate::Message {
            message_id,
            text,
            is_final,
        } => Ok((
            state,
            vec![append(
                actor,
                EventBody::AgentMessage(AgentMessageData {
                    text: text.clone(),
                    message_id: message_id.clone(),
                    is_final: *is_final,
                }),
            )],
        )),
        AgentUpdate::Status {
            state: task,
            detail,
        } => status_input(state, actor, *task, detail),
    }
}

fn status_input(
    state: ThreadState,
    actor: Actor,
    task: AgentTaskState,
    detail: &Option<String>,
) -> Result<(ThreadState, Vec<Command>), TransitionError> {
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
                ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                    Ok((state, vec![]))
                }
            }
        }
        AgentTaskState::InputRequired | AgentTaskState::AuthRequired => {
            let detail = match task {
                AgentTaskState::AuthRequired => Some(prefixed("authentication required", detail)),
                AgentTaskState::Submitted
                | AgentTaskState::Working
                | AgentTaskState::InputRequired
                | AgentTaskState::Completed
                | AgentTaskState::Failed
                | AgentTaskState::Canceled
                | AgentTaskState::Rejected => detail.clone(),
            };
            let cmd = agent_status(actor, AgentStatus::InputRequired, detail.clone());
            match state {
                ThreadState::Queued | ThreadState::Working => Ok((
                    ThreadState::Blocked,
                    vec![cmd, entered(ThreadState::Blocked)],
                )),
                ThreadState::Blocked => match detail {
                    Some(_) => Ok((ThreadState::Blocked, vec![cmd])),
                    None => Ok((ThreadState::Blocked, vec![])),
                },
                ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
                    Ok((state, vec![]))
                }
            }
        }
        AgentTaskState::Completed => Ok((
            ThreadState::Done,
            vec![
                agent_status(actor, AgentStatus::Completed, detail.clone()),
                entered(ThreadState::Done),
            ],
        )),
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
