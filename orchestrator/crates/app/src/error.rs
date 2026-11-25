use std::time::Duration;

use orch_core::{
    AgentId, Classify, ErrorClass, ForkError, NotDeletable, SessionRefusal, TransitionError,
};
use orch_ports::{AgentError, ArtifactError, RegistryError, StoreError};

use crate::authz::Permission;
use crate::prompts::PromptRefusal;
use crate::sharing::SharingMode;
use crate::user_tools::UserToolRefusal;

/// Application failure. The API maps these to RFC 9457 problems by [`class`](Classify::class).
///
/// A message describes this layer only; the lower error is the `source`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// No such thread for this user (a foreign thread is indistinguishable from a missing one).
    #[error("not found")]
    NotFound,
    /// The person is who they say, and their roles do not let them do this (ADR 0033): a permission
    /// they lack, or an agent their roles do not name. Never said about a thread that is not the
    /// person's: that is [`AppError::NotFound`], whatever their roles (ADR 0039).
    #[error("{detail}")]
    Forbidden {
        /// The permission that was needed.
        permission: Permission,
        /// What was refused, fit to show the caller: it names no one else's thread or role.
        detail: String,
    },
    /// The deployment's sharing cap is `disabled` (ADR 0040): nothing can be shared or given a new
    /// link. Taking a link down is never refused for it.
    #[error("sharing is disabled in this deployment")]
    SharingDisabled,
    /// A share above the deployment's cap (ADR 0040): `public` under a cap of `internal`.
    #[error("this deployment shares at most {cap}")]
    OverCap {
        /// The cap.
        cap: SharingMode,
    },
    /// A thread that works cannot be deleted (ADR 0043): it is `queued`, `working` or `verifying`,
    /// or an agent its job asked is still running. Stop it, wait for it to end, then delete it.
    #[error("{0}; stop it and delete it when it has ended")]
    ThreadActive(NotDeletable),
    /// A new link for a thread that is not shared (ADR 0040).
    #[error("the thread is not shared")]
    NotShared,
    /// The request is invalid.
    #[error("{0}")]
    Invalid(String),
    /// An action on a card of a finished job (ADR 0020). A message is not refused: it starts the
    /// thread's next job.
    #[error("this card belongs to a finished request")]
    Finished,
    /// A thread cannot be cut where it was asked to (ADR 0029).
    #[error(transparent)]
    Fork(ForkError),
    /// The request is valid, but what it names is in the way: an id that is taken by another
    /// thread, a family of edits that has no room for another.
    #[error("{0}")]
    Refused(String),
    /// The request is well formed, but what it names cannot be done: a tool server the deployment
    /// does not list, or does not offer to the thread's agent, or more servers than a thread may
    /// have (ADR 0024). The detail names the server's id and never a URL or a credential.
    #[error("{0}")]
    Unprocessable(String),
    /// A model or a reasoning effort the person picked that the live card of the thread's agent does
    /// not offer (ADR 0086): refused before anything is written, never silently replaced by the
    /// agent's default. The refusal carries the stable code and the list the card offers.
    #[error("{0}")]
    PickNotOffered(orch_core::PickRefusal),
    /// A change of how the person's list shows a thread that cannot be made (ADR 0042): well
    /// formed, and what it names is in the way. `code` is the stable name a client acts on
    /// (`bad_anchor`, `nested_row`), and `child_thread` for the thread of a helper session that
    /// is neither forked, shared nor written to (ADR 0072).
    #[error("{detail}")]
    Arrangement {
        /// The stable name.
        code: &'static str,
        /// What is in the way, fit to show the caller.
        detail: String,
    },
    /// A request about a helper session that what it names is in the way of, with a stable `code`
    /// a client acts on (ADR 0072, S3): `session_closed`, `no_context`, `too_many_starts`.
    #[error("{detail}")]
    Blocked {
        /// The stable name.
        code: &'static str,
        /// What is in the way, fit to show the caller.
        detail: String,
    },
    /// A request about a person's own MCP servers is refused (ADR 0046): the feature is off, no such
    /// server, the limit, a stale revision, a URL the guard refuses, a deployment that keeps no
    /// credentials, a broker that cannot keep one now. See [`UserToolRefusal`].
    #[error(transparent)]
    UserTool(UserToolRefusal),
    /// A request about a person's prompt library is refused (ADR 0070): a rule of the page, a name
    /// taken, the limit, a stale revision, no such prompt, an answer that does not fit, a text that
    /// is too long. See [`PromptRefusal`].
    #[error(transparent)]
    Prompt(PromptRefusal),
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The artifact store failed (ADR 0032). A file that is not there is [`AppError::NotFound`].
    #[error(transparent)]
    Artifacts(#[from] ArtifactError),
    /// An input was not valid in the thread's state.
    #[error(transparent)]
    Transition(TransitionError),
    /// A delegated agent could not do what this request needs of it (for example its card is
    /// unreachable, so a release cannot be validated).
    #[error("agent {agent} is unavailable")]
    Upstream {
        /// The agent that failed.
        agent: AgentId,
        /// What went wrong with it.
        #[source]
        source: AgentError,
    },
    /// The agent registry could not say whether an agent exists (ADR 0022: fail closed). The
    /// caller is not wrong; it can try again once the registry answers.
    #[error("the agent registry is unreachable")]
    RegistryUnavailable {
        /// What the registry said (its source and why, never a URL).
        #[source]
        source: RegistryError,
    },
    /// The live card of an agent could not be read, and what this request needs to know of it
    /// (whether it honours the access tier, ADR 0061) is in it. Fail closed: nothing is written
    /// and nothing is assumed. The caller is not wrong; it can try again once the card is back.
    #[error("the card of the agent {agent} could not be read")]
    CardUnavailable {
        /// The agent whose card it was.
        agent: AgentId,
    },
    /// A message over the person's role's `messages_per_minute` (ADR 0081, decision 4): refused
    /// before anything is written and before any agent is asked, with the seconds until the oldest
    /// message in the window leaves it. `limit` is the name the problem body carries and `max` its
    /// value; `in_use` is how many messages the window holds.
    #[error("the limit {limit} is reached: {in_use} in the last minute, at most {max}")]
    MessageLimit {
        /// The limit's name (`messages_per_minute`).
        limit: &'static str,
        /// The messages the person sent in the window.
        in_use: u32,
        /// The role's limit.
        max: u32,
        /// How long until the oldest message in the window leaves it, in whole seconds.
        retry_after: Duration,
    },
    /// The thread kept changing under the optimistic commit loop; try again.
    #[error("the thread is being changed concurrently")]
    Contended,
    /// An invariant of the application broke: a bug.
    #[error("internal error: {detail}")]
    Internal {
        /// What broke, without secrets.
        detail: String,
    },
}

impl AppError {
    /// The roles of the person do not hold `permission`.
    pub fn missing_permission(permission: Permission) -> Self {
        AppError::Forbidden {
            permission,
            detail: format!("your roles do not grant {permission}"),
        }
    }

    /// The roles of the person do not name `agent` for `permission`.
    pub fn agent_not_allowed(permission: Permission, agent: &AgentId) -> Self {
        AppError::Forbidden {
            permission,
            detail: format!("your roles do not grant {permission} for the agent {agent}"),
        }
    }

    /// An arrangement the store refused with `code` (`bad_anchor`, `nested_row`).
    pub fn arrangement(code: &'static str) -> Self {
        let detail = match code {
            "bad_anchor" => {
                "the thread to place it by is gone, archived, nested under another or the thread itself"
            }
            "nested_row" => {
                "a thread nested under another cannot be pinned or placed; eject it from its parent first"
            }
            "child_thread" => {
                "a helper session's thread cannot be forked or shared, and a helper session cannot start another"
            }
            "child_mentions" => "a helper session cannot ask other agents",
            "session_closes_on_stop" => {
                "a session that closes when done closes when its round is stopped"
            }
            "not_child_thread" => "only a helper session's thread can be closed",
            "no_child_sessions" => "this agent does not run helpers as sessions",
            _ => "this arrangement cannot be made",
        };
        AppError::Arrangement {
            code,
            detail: detail.to_owned(),
        }
    }

    /// The thread of a helper session (a child, ADR 0072) cannot be forked or shared, and a child
    /// cannot start a session: 422 `child_thread`.
    pub fn child_thread() -> Self {
        AppError::arrangement("child_thread")
    }

    /// A refusal about a helper session with a code of its own (409).
    pub fn blocked(code: &'static str, detail: impl Into<String>) -> Self {
        AppError::Blocked {
            code,
            detail: detail.into(),
        }
    }

    /// A broken application invariant.
    pub fn internal(detail: impl Into<String>) -> Self {
        AppError::Internal {
            detail: detail.into(),
        }
    }

    /// An agent failed.
    pub fn upstream(agent: &AgentId, source: AgentError) -> Self {
        AppError::Upstream {
            agent: agent.clone(),
            source,
        }
    }
}

impl From<ForkError> for AppError {
    fn from(e: ForkError) -> Self {
        AppError::Fork(e)
    }
}

impl From<TransitionError> for AppError {
    fn from(e: TransitionError) -> Self {
        match e {
            TransitionError::Finished { .. } => AppError::Finished,
            // well formed, but what it names cannot be done: 422 (ADR 0036)
            e @ TransitionError::TextTooLong { .. } => AppError::Unprocessable(e.to_string()),
            // a helper session (ADR 0072, S3): each has the code a client acts on
            TransitionError::SessionClosed => AppError::blocked(
                "session_closed",
                "this helper session is closed and takes no more messages",
            ),
            TransitionError::SessionClosesOnStop => AppError::arrangement("session_closes_on_stop"),
            TransitionError::ChildMentions => AppError::arrangement("child_mentions"),
            TransitionError::NotAChild => AppError::arrangement("not_child_thread"),
            TransitionError::ChildSessionRefused(SessionRefusal::TooManyStarts) => {
                AppError::blocked(
                    "too_many_starts",
                    "wait for the helpers you started to open",
                )
            }
            TransitionError::ChildSessionRefused(SessionRefusal::ChildThread) => {
                AppError::child_thread()
            }
            other => AppError::Transition(other),
        }
    }
}

impl Classify for AppError {
    fn class(&self) -> ErrorClass {
        match self {
            AppError::NotFound => ErrorClass::NotFound,
            AppError::Forbidden { .. } | AppError::SharingDisabled => ErrorClass::Forbidden,
            AppError::Invalid(_) => ErrorClass::Invalid,
            AppError::Finished
            | AppError::Refused(_)
            | AppError::Unprocessable(_)
            | AppError::PickNotOffered(_)
            | AppError::Arrangement { .. }
            | AppError::Blocked { .. }
            | AppError::OverCap { .. }
            | AppError::ThreadActive(_)
            | AppError::NotShared => ErrorClass::Rejected,
            AppError::UserTool(refusal) => match refusal {
                UserToolRefusal::Off | UserToolRefusal::NotFound => ErrorClass::NotFound,
                UserToolRefusal::UrlRefused { .. } => ErrorClass::Invalid,
                UserToolRefusal::TooMany { .. }
                | UserToolRefusal::StaleRevision
                | UserToolRefusal::NoCredentials
                | UserToolRefusal::EgressRefused { .. }
                | UserToolRefusal::NotReady { .. }
                | UserToolRefusal::OAuthNotConfigured => ErrorClass::Rejected,
                UserToolRefusal::CredentialsUnavailable
                | UserToolRefusal::AuthorizationUnavailable => ErrorClass::Transient,
                UserToolRefusal::Busy => ErrorClass::Conflict,
            },
            AppError::Prompt(refusal) => match refusal {
                PromptRefusal::NotFound => ErrorClass::NotFound,
                PromptRefusal::Invalid(_) => ErrorClass::Invalid,
                PromptRefusal::NameTaken
                | PromptRefusal::TooMany { .. }
                | PromptRefusal::StaleRevision
                | PromptRefusal::InvalidAnswer(_)
                | PromptRefusal::RenderedTooLong => ErrorClass::Rejected,
            },
            AppError::Fork(e) => e.class(),
            AppError::Store(e) => e.class(),
            AppError::Artifacts(e) => e.class(),
            AppError::Transition(e) => e.class(),
            AppError::Upstream { source, .. } => source.class(),
            AppError::RegistryUnavailable { source } => source.class(),
            AppError::CardUnavailable { .. } => ErrorClass::Transient,
            AppError::MessageLimit { .. } => ErrorClass::RateLimited,
            AppError::Contended => ErrorClass::Conflict,
            AppError::Internal { .. } => ErrorClass::Internal,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            AppError::Store(e) => e.retry_after(),
            AppError::Upstream { source, .. } => source.retry_after(),
            AppError::MessageLimit { retry_after, .. } => Some(*retry_after),
            AppError::RegistryUnavailable { .. } | AppError::CardUnavailable { .. } => None,
            AppError::NotFound
            | AppError::Forbidden { .. }
            | AppError::SharingDisabled
            | AppError::OverCap { .. }
            | AppError::NotShared
            | AppError::ThreadActive(_)
            | AppError::Invalid(_)
            | AppError::Finished
            | AppError::Refused(_)
            | AppError::Unprocessable(_)
            | AppError::PickNotOffered(_)
            | AppError::Arrangement { .. }
            | AppError::Blocked { .. }
            | AppError::UserTool(_)
            | AppError::Prompt(_)
            | AppError::Fork(_)
            | AppError::Artifacts(_)
            | AppError::Transition(_)
            | AppError::Contended
            | AppError::Internal { .. } => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::ThreadState;

    use crate::user_tools::OwnReason;

    use super::*;

    #[test]
    fn what_the_core_refuses_of_a_helper_session_has_the_code_a_client_acts_on() {
        for (refusal, code) in [
            (TransitionError::SessionClosed, "session_closed"),
            (
                TransitionError::SessionClosesOnStop,
                "session_closes_on_stop",
            ),
            (TransitionError::ChildMentions, "child_mentions"),
            (TransitionError::NotAChild, "not_child_thread"),
            (
                TransitionError::ChildSessionRefused(SessionRefusal::TooManyStarts),
                "too_many_starts",
            ),
            (
                TransitionError::ChildSessionRefused(SessionRefusal::ChildThread),
                "child_thread",
            ),
        ] {
            match AppError::from(refusal.clone()) {
                AppError::Arrangement { code: got, .. } | AppError::Blocked { code: got, .. } => {
                    assert_eq!(got, code, "{refusal}");
                }
                other => panic!("{refusal}: {other:?}"),
            }
        }
    }

    fn io(msg: &str) -> std::io::Error {
        std::io::Error::other(msg.to_owned())
    }

    #[test]
    fn class_table() {
        let all = [
            AppError::NotFound,
            AppError::missing_permission(Permission::ThreadWrite),
            AppError::agent_not_allowed(Permission::AgentInvoke, &AgentId::new("coder")),
            AppError::SharingDisabled,
            AppError::OverCap {
                cap: SharingMode::Internal,
            },
            AppError::NotShared,
            AppError::ThreadActive(NotDeletable::AskRunning),
            AppError::Invalid("bad".into()),
            AppError::Finished,
            AppError::Refused("taken".into()),
            AppError::Unprocessable("no such server".into()),
            AppError::PickNotOffered(orch_core::PickRefusal::ModelNotOffered {
                picked: "deep".into(),
                offered: vec!["fast".into()],
            }),
            AppError::arrangement("bad_anchor"),
            AppError::arrangement("nested_row"),
            AppError::blocked("session_closed", "closed"),
            AppError::UserTool(UserToolRefusal::Off),
            AppError::UserTool(UserToolRefusal::NotFound),
            AppError::UserTool(UserToolRefusal::TooMany { limit: 8 }),
            AppError::UserTool(UserToolRefusal::StaleRevision),
            AppError::UserTool(UserToolRefusal::NoCredentials),
            AppError::UserTool(UserToolRefusal::UrlRefused {
                reason: "no".into(),
            }),
            AppError::UserTool(UserToolRefusal::CredentialsUnavailable),
            AppError::UserTool(UserToolRefusal::EgressRefused {
                reason: "no".into(),
            }),
            AppError::UserTool(UserToolRefusal::OAuthNotConfigured),
            AppError::UserTool(UserToolRefusal::NotReady {
                id: "u-abcdefghij".into(),
                reason: OwnReason::Reauthorize,
            }),
            AppError::UserTool(UserToolRefusal::AuthorizationUnavailable),
            AppError::UserTool(UserToolRefusal::Busy),
            AppError::Prompt(PromptRefusal::NotFound),
            AppError::Prompt(PromptRefusal::NameTaken),
            AppError::Prompt(PromptRefusal::TooMany { limit: 100 }),
            AppError::Prompt(PromptRefusal::StaleRevision),
            AppError::Prompt(PromptRefusal::RenderedTooLong),
            AppError::Fork(ForkError::OutOfRange),
            AppError::Fork(ForkError::TurnOpen),
            AppError::Store(StoreError::unavailable(io("down"))),
            AppError::Artifacts(ArtifactError::unavailable("down")),
            AppError::Artifacts(ArtifactError::Corrupt("meta".into())),
            AppError::Transition(TransitionError::InvalidInState {
                state: ThreadState::Queued,
                input: "cancel",
            }),
            AppError::upstream(&AgentId::new("coder"), AgentError::unreachable("card")),
            AppError::RegistryUnavailable {
                source: RegistryError::unavailable("platform", "the registry could not be reached"),
            },
            AppError::CardUnavailable {
                agent: AgentId::new("coder"),
            },
            AppError::Contended,
            AppError::MessageLimit {
                limit: "messages_per_minute",
                in_use: 3,
                max: 2,
                retry_after: Duration::from_secs(4),
            },
            AppError::internal("broken"),
        ];
        for e in all {
            // Exhaustive: a new variant forces a class decision.
            let expected = match &e {
                AppError::NotFound => ErrorClass::NotFound,
                AppError::Forbidden { .. } | AppError::SharingDisabled => ErrorClass::Forbidden,
                AppError::Invalid(_) => ErrorClass::Invalid,
                AppError::Finished
                | AppError::Refused(_)
                | AppError::Unprocessable(_)
                | AppError::PickNotOffered(_)
                | AppError::Arrangement { .. }
                | AppError::Blocked { .. }
                | AppError::OverCap { .. }
                | AppError::ThreadActive(_)
                | AppError::NotShared => ErrorClass::Rejected,
                AppError::UserTool(refusal) => match refusal {
                    UserToolRefusal::Off | UserToolRefusal::NotFound => ErrorClass::NotFound,
                    UserToolRefusal::UrlRefused { .. } => ErrorClass::Invalid,
                    UserToolRefusal::TooMany { .. }
                    | UserToolRefusal::StaleRevision
                    | UserToolRefusal::NoCredentials
                    | UserToolRefusal::EgressRefused { .. }
                    | UserToolRefusal::NotReady { .. }
                    | UserToolRefusal::OAuthNotConfigured => ErrorClass::Rejected,
                    UserToolRefusal::CredentialsUnavailable
                    | UserToolRefusal::AuthorizationUnavailable => ErrorClass::Transient,
                    UserToolRefusal::Busy => ErrorClass::Conflict,
                },
                AppError::Prompt(refusal) => match refusal {
                    PromptRefusal::NotFound => ErrorClass::NotFound,
                    PromptRefusal::Invalid(_) => ErrorClass::Invalid,
                    _ => ErrorClass::Rejected,
                },
                AppError::Fork(inner) => inner.class(),
                AppError::Store(inner) => inner.class(),
                AppError::Artifacts(inner) => inner.class(),
                AppError::Transition(inner) => inner.class(),
                AppError::Upstream { source, .. } => source.class(),
                AppError::RegistryUnavailable { source } => source.class(),
                AppError::CardUnavailable { .. } => ErrorClass::Transient,
                AppError::Contended => ErrorClass::Conflict,
                AppError::MessageLimit { .. } => ErrorClass::RateLimited,
                AppError::Internal { .. } => ErrorClass::Internal,
            };
            assert_eq!(e.class(), expected, "{e}");
        }
    }

    #[test]
    fn the_hint_of_a_rate_limited_agent_reaches_the_app_error() {
        let e = AppError::upstream(
            &AgentId::new("coder"),
            AgentError::RateLimited {
                retry_after: Some(Duration::from_secs(9)),
            },
        );
        assert_eq!(e.class(), ErrorClass::RateLimited);
        assert_eq!(e.retry_after(), Some(Duration::from_secs(9)));
        assert_eq!(AppError::Contended.retry_after(), None);
    }

    #[test]
    fn no_layer_prints_its_source() {
        let e = AppError::upstream(
            &AgentId::new("coder"),
            AgentError::unreachable("card fetch failed").with_source(io("refused")),
        );
        assert_eq!(e.to_string(), "agent coder is unavailable");
        assert_eq!(
            orch_core::report(&e),
            "agent coder is unavailable: agent unreachable: card fetch failed: refused"
        );
        let e = AppError::Store(StoreError::unavailable(io("pool timed out")));
        assert_eq!(e.to_string(), "store unavailable");
        assert_eq!(orch_core::report(&e), "store unavailable: pool timed out");
    }
}
