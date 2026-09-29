use orch_core::TransitionError;
use orch_ports::{AgentError, StoreError};

/// Application failure. The API maps these to RFC 9457 problems.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// No such thread for this user (a foreign thread is indistinguishable from a missing one).
    #[error("not found")]
    NotFound,
    /// The request is invalid.
    #[error("{0}")]
    Invalid(String),
    /// The thread is finished; start a new one.
    #[error("thread is finished")]
    Finished,
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An input was not valid in the thread's state.
    #[error(transparent)]
    Transition(TransitionError),
    /// The agent failed.
    #[error(transparent)]
    Agent(#[from] AgentError),
}

impl From<TransitionError> for AppError {
    fn from(e: TransitionError) -> Self {
        match e {
            TransitionError::Finished { .. } => AppError::Finished,
            TransitionError::InvalidInState { .. } => AppError::Transition(e),
        }
    }
}
