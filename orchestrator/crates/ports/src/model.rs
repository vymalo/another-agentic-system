//! The model port: one question to a language model and its answer (ADR 0005, ADR 0009).
//!
//! The orchestrator's first use of a model is small and its own: the title of a thread, written
//! from the first words of the conversation (`orch_core::title_prompt`). It is not an agent, it has
//! no tools and no memory, and it speaks the one protocol every model endpoint speaks (an
//! OpenAI-compatible chat completion, ADR 0005). The port is what the dispatcher calls, so a
//! deployment without a model (`NoModel`) and a test with a scripted one (`memory::ScriptedModel`)
//! are the same code path.

use std::future::Future;
use std::time::Duration;

use orch_core::{BoxError, Classify, ErrorClass};

/// One question: the instruction (`system`), the text to work on (`user`), the model to ask and
/// how long an answer may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatRequest {
    /// The model's name at the endpoint (a deployment's choice, never a secret).
    pub model: String,
    /// What the model is told to do.
    pub system: String,
    /// What it is told it about. Untrusted data, fenced by whoever wrote it.
    pub user: String,
    /// Most tokens of answer.
    pub max_tokens: u32,
}

/// The model's answer, as text.
///
/// An implementation never stores anything, never retries (the caller decides, by
/// [`Classify::class`]) and never puts the endpoint's credential into an error.
pub trait ChatModel: Send + Sync + 'static {
    /// Asks the model and returns the text of its first answer.
    ///
    /// # Errors
    /// [`ModelError`]; its class says whether asking again can help.
    fn complete(
        &self,
        request: &ChatRequest,
    ) -> impl Future<Output = Result<String, ModelError>> + Send;
}

/// Why a model did not answer.
///
/// `Unreachable` and `Protocol` describe this side; the transport error is their `source` (adapters
/// box theirs: ADR 0009) and may hold URLs, so it never reaches the chat log. None of them ever
/// holds the credential. `Rejected` carries the peer's own message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ModelError {
    /// Could not reach the model (connection, timeout, 5xx). Retryable.
    #[error("model unreachable: {detail}")]
    Unreachable {
        /// What was attempted, without URLs or secrets.
        detail: String,
        /// The transport error.
        #[source]
        source: Option<BoxError>,
    },
    /// The endpoint asked the orchestrator to slow down (429).
    #[error("model rate limited the request")]
    RateLimited {
        /// How long the peer asked to wait, when it said.
        retry_after: Option<Duration>,
    },
    /// The endpoint refused the request for good (an unknown model, a request it cannot read).
    #[error("model rejected the request: {0}")]
    Rejected(String),
    /// The endpoint refused the credential, or wants one it was not given.
    #[error("model refused the credentials")]
    Unauthenticated,
    /// The endpoint answered something that is not a chat completion. Retryable (bounded).
    #[error("model protocol error: {detail}")]
    Protocol {
        /// What was unexpected, without URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// This deployment has no model to ask ([`NoModel`]).
    #[error("no model is configured")]
    NotConfigured,
}

impl ModelError {
    /// The model could not be reached.
    pub fn unreachable(detail: impl Into<String>) -> Self {
        ModelError::Unreachable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The model answered something unexpected.
    pub fn protocol(detail: impl Into<String>) -> Self {
        ModelError::Protocol {
            detail: detail.into(),
            source: None,
        }
    }

    /// Keeps `source` as the cause of an `Unreachable` or `Protocol` error; the other variants
    /// have no source and are returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            ModelError::Unreachable { detail, .. } => ModelError::Unreachable {
                detail,
                source: Some(source.into()),
            },
            ModelError::Protocol { detail, .. } => ModelError::Protocol {
                detail,
                source: Some(source.into()),
            },
            other @ (ModelError::RateLimited { .. }
            | ModelError::Rejected(_)
            | ModelError::Unauthenticated
            | ModelError::NotConfigured) => other,
        }
    }
}

impl Classify for ModelError {
    fn class(&self) -> ErrorClass {
        match self {
            ModelError::Unreachable { .. } | ModelError::Protocol { .. } => ErrorClass::Transient,
            ModelError::RateLimited { .. } => ErrorClass::RateLimited,
            ModelError::Rejected(_) => ErrorClass::Invalid,
            ModelError::Unauthenticated => ErrorClass::Unauthenticated,
            ModelError::NotConfigured => ErrorClass::Unsupported,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            ModelError::RateLimited { retry_after } => *retry_after,
            ModelError::Unreachable { .. }
            | ModelError::Rejected(_)
            | ModelError::Unauthenticated
            | ModelError::Protocol { .. }
            | ModelError::NotConfigured => None,
        }
    }
}

/// The model of a deployment that has none: every question is [`ModelError::NotConfigured`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoModel;

impl ChatModel for NoModel {
    async fn complete(&self, _request: &ChatRequest) -> Result<String, ModelError> {
        Err(ModelError::NotConfigured)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_the_class_that_says_what_to_do() {
        let cases: [(ModelError, ErrorClass); 6] = [
            (ModelError::unreachable("down"), ErrorClass::Transient),
            (ModelError::protocol("not json"), ErrorClass::Transient),
            (
                ModelError::RateLimited {
                    retry_after: Some(Duration::from_secs(3)),
                },
                ErrorClass::RateLimited,
            ),
            (
                ModelError::Rejected("no such model".into()),
                ErrorClass::Invalid,
            ),
            (ModelError::Unauthenticated, ErrorClass::Unauthenticated),
            (ModelError::NotConfigured, ErrorClass::Unsupported),
        ];
        for (err, class) in cases {
            assert_eq!(err.class(), class, "{err}");
        }
        assert!(ModelError::unreachable("x").is_retryable());
        assert!(!ModelError::Rejected("x".into()).is_retryable());
        assert_eq!(
            ModelError::RateLimited {
                retry_after: Some(Duration::from_secs(3))
            }
            .retry_after(),
            Some(Duration::from_secs(3))
        );
    }

    #[tokio::test]
    async fn no_model_is_not_configured() {
        let request = ChatRequest {
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            max_tokens: 10,
        };
        let answer = NoModel.complete(&request).await;
        assert!(
            matches!(answer, Err(ModelError::NotConfigured)),
            "{answer:?}"
        );
    }
}
