use std::time::Duration;

use orch_core::{AgentId, Classify, ErrorClass, ForkError, NotDeletable, TransitionError};
use orch_ports::{AgentError, ArtifactError, RegistryError, StoreError};

use crate::authz::Permission;
use crate::sharing::SharingMode;

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
    /// A change of how the person's list shows a thread that cannot be made (ADR 0042): well
    /// formed, and what it names is in the way. `code` is the stable name a client acts on
    /// (`bad_anchor`, `nested_row`).
    #[error("{detail}")]
    Arrangement {
        /// The stable name.
        code: &'static str,
        /// What is in the way, fit to show the caller.
        detail: String,
    },
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
            _ => "this arrangement cannot be made",
        };
        AppError::Arrangement {
            code,
            detail: detail.to_owned(),
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
            | AppError::Arrangement { .. }
            | AppError::OverCap { .. }
            | AppError::ThreadActive(_)
            | AppError::NotShared => ErrorClass::Rejected,
            AppError::Fork(e) => e.class(),
            AppError::Store(e) => e.class(),
            AppError::Artifacts(e) => e.class(),
            AppError::Transition(e) => e.class(),
            AppError::Upstream { source, .. } => source.class(),
            AppError::RegistryUnavailable { source } => source.class(),
            AppError::Contended => ErrorClass::Conflict,
            AppError::Internal { .. } => ErrorClass::Internal,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            AppError::Store(e) => e.retry_after(),
            AppError::Upstream { source, .. } => source.retry_after(),
            AppError::RegistryUnavailable { .. } => None,
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
            | AppError::Arrangement { .. }
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

    use super::*;

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
            AppError::arrangement("bad_anchor"),
            AppError::arrangement("nested_row"),
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
            AppError::Contended,
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
                | AppError::Arrangement { .. }
                | AppError::OverCap { .. }
                | AppError::ThreadActive(_)
                | AppError::NotShared => ErrorClass::Rejected,
                AppError::Fork(inner) => inner.class(),
                AppError::Store(inner) => inner.class(),
                AppError::Artifacts(inner) => inner.class(),
                AppError::Transition(inner) => inner.class(),
                AppError::Upstream { source, .. } => source.class(),
                AppError::RegistryUnavailable { source } => source.class(),
                AppError::Contended => ErrorClass::Conflict,
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
