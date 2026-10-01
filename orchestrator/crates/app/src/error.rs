use std::time::Duration;

use orch_core::{AgentId, Classify, ErrorClass, TransitionError};
use orch_ports::{AgentError, RegistryError, StoreError};

/// Application failure. The API maps these to RFC 9457 problems by [`class`](Classify::class).
///
/// A message describes this layer only; the lower error is the `source`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AppError {
    /// No such thread for this user (a foreign thread is indistinguishable from a missing one).
    #[error("not found")]
    NotFound,
    /// The request is invalid.
    #[error("{0}")]
    Invalid(String),
    /// An action on a card of a finished job (ADR 0020). A message is not refused: it starts the
    /// thread's next job.
    #[error("this card belongs to a finished request")]
    Finished,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
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

impl From<TransitionError> for AppError {
    fn from(e: TransitionError) -> Self {
        match e {
            TransitionError::Finished { .. } => AppError::Finished,
            other => AppError::Transition(other),
        }
    }
}

impl Classify for AppError {
    fn class(&self) -> ErrorClass {
        match self {
            AppError::NotFound => ErrorClass::NotFound,
            AppError::Invalid(_) => ErrorClass::Invalid,
            AppError::Finished => ErrorClass::Rejected,
            AppError::Store(e) => e.class(),
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
            | AppError::Invalid(_)
            | AppError::Finished
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
            AppError::Invalid("bad".into()),
            AppError::Finished,
            AppError::Store(StoreError::unavailable(io("down"))),
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
                AppError::Invalid(_) => ErrorClass::Invalid,
                AppError::Finished => ErrorClass::Rejected,
                AppError::Store(inner) => inner.class(),
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
