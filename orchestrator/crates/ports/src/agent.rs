use std::fmt;
use std::future::Future;
use std::time::Duration;

use futures::stream::BoxStream;
use orch_core::{AgentId, AgentTaskState, AgentUpdate, BoxError, Classify, ErrorClass, Releases};

/// How to reach an agent, one variant per way (ADR 0004: a closed enum, so the compiler lists
/// every `match` a new transport must handle). It lives in the ports, not in the core, because it
/// carries a secret that `transition` never sees. `Debug` never prints the bearer token.
///
/// The variants are always compiled, whatever Cargo features a binary is built with: what a
/// build can *serve* is the composition root's business (an adapter answers
/// [`AgentError::Unsupported`] for an endpoint that is not its own), not the shape of this type.
#[derive(Clone, PartialEq, Eq)]
pub enum AgentTransport {
    /// A remote A2A agent, read from its agent card.
    A2a {
        /// URL of the agent card (or of the agent's base URL).
        card_url: String,
        /// Bearer token for the agent, if configured (already resolved from the environment).
        bearer: Option<String>,
    },
    /// An agent hosted in the orchestrator's own process (ADR 0015): no card URL, no network hop,
    /// no bearer token. `name` is the kind of local agent, as the composition root names it (for
    /// example `echo`); which kinds a build contains is decided there, never here.
    Local {
        /// The kind of local agent.
        name: String,
    },
}

impl fmt::Debug for AgentTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentTransport::A2a { card_url, bearer } => f
                .debug_struct("A2a")
                .field("card_url", card_url)
                .field("bearer", &bearer.as_ref().map(|_| "<redacted>"))
                .finish(),
            AgentTransport::Local { name } => f.debug_struct("Local").field("name", name).finish(),
        }
    }
}

/// Where an agent lives: its configuration key and how to reach it. `Debug` never prints a
/// secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEndpoint {
    /// Configuration key.
    pub id: AgentId,
    /// How to reach it.
    pub transport: AgentTransport,
}

impl AgentEndpoint {
    /// A remote A2A agent.
    pub fn a2a(id: AgentId, card_url: impl Into<String>, bearer: Option<String>) -> Self {
        AgentEndpoint {
            id,
            transport: AgentTransport::A2a {
                card_url: card_url.into(),
                bearer,
            },
        }
    }

    /// An agent hosted in this process; `name` is the kind of local agent.
    pub fn local(id: AgentId, name: impl Into<String>) -> Self {
        AgentEndpoint {
            id,
            transport: AgentTransport::Local { name: name.into() },
        }
    }
}

/// What the live agent card says (read fresh every time, never cached: ADR 0008).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentCardInfo {
    /// Card description.
    pub description: Option<String>,
    /// Present only when the card advertises the release-channels extension.
    pub releases: Option<Releases>,
}

/// One message to deliver to the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    /// Target agent.
    pub endpoint: AgentEndpoint,
    /// Idempotent message id (the outbox row id).
    pub message_id: String,
    /// The thread's A2A context.
    pub context_id: String,
    /// Continue this task (a follow-up to an `input-required` task).
    pub task_id: Option<String>,
    /// Message text.
    pub text: String,
    /// Selected release channel or revision (only sent when the card offers releases).
    pub release: Option<String>,
}

/// A task on an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHandle {
    /// Agent.
    pub endpoint: AgentEndpoint,
    /// Task id.
    pub task_id: String,
}

/// How an envelope's idempotency key is scoped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdemKey {
    /// Unique per thread whatever the turn (artifact ids, message ids).
    Task(String),
    /// Unique per delivered outbox row: the dispatcher prefixes it with the row id.
    Turn(String),
}

/// One thing an agent reported, with its idempotency key and task bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEnvelope {
    /// Task the update belongs to.
    pub task_id: String,
    /// Context of the task.
    pub context_id: String,
    /// Task state this envelope implies, if any.
    pub task_state: Option<AgentTaskState>,
    /// Revision that produced it (ADR 0008 echo).
    pub revision: Option<String>,
    /// Idempotency key.
    pub key: IdemKey,
    /// The update; `None` when the envelope only carries bookkeeping.
    pub update: Option<AgentUpdate>,
}

/// A polled view of a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    /// Task id.
    pub task_id: String,
    /// Context id.
    pub context_id: String,
    /// Current state.
    pub state: AgentTaskState,
    /// Serving revision.
    pub revision: Option<String>,
    /// What happened so far, as envelopes (same keys as the live stream produces).
    pub envelopes: Vec<AgentEnvelope>,
}

/// A stream of envelopes; it ends when the agent's stream ends.
pub type AgentStream = BoxStream<'static, Result<AgentEnvelope, AgentError>>;

/// Agent failure, classified so the dispatcher can decide between retry and give-up.
///
/// `Rejected`, `TaskNotFound`, `Unsupported` and `NotCancelable` carry the peer's own message.
/// `Unreachable` and `Protocol` describe this side; the transport error is their `source`
/// (adapters box theirs: ADR 0009) and may hold URLs, so it never reaches the chat log:
/// [`AgentError::public_detail`] is what may.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    /// Could not reach the agent (connection, timeout, 5xx). Retryable.
    #[error("agent unreachable: {detail}")]
    Unreachable {
        /// What was attempted, without URLs or secrets.
        detail: String,
        /// The transport error.
        #[source]
        source: Option<BoxError>,
    },
    /// The agent refused the request for good (invalid params, extension required, release rejected).
    #[error("agent rejected the request: {0}")]
    Rejected(String),
    /// The agent does not know the task.
    #[error("task not found: {0}")]
    TaskNotFound(String),
    /// The agent does not support the operation.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The task is already finished.
    #[error("task not cancelable: {0}")]
    NotCancelable(String),
    /// Unexpected protocol behaviour. Retryable (bounded).
    #[error("protocol error: {detail}")]
    Protocol {
        /// What was unexpected, without URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// The agent refused the orchestrator's credentials, or wants some it was not given.
    #[error("agent refused the credentials: {detail}")]
    Unauthenticated {
        /// What was refused.
        detail: String,
    },
    /// The agent (or a proxy in front of it) asked the orchestrator to slow down.
    #[error("agent rate limited the request")]
    RateLimited {
        /// How long the peer asked to wait, when it said.
        retry_after: Option<Duration>,
    },
}

impl AgentError {
    /// The agent could not be reached.
    pub fn unreachable(detail: impl Into<String>) -> Self {
        AgentError::Unreachable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The agent behaved unexpectedly.
    pub fn protocol(detail: impl Into<String>) -> Self {
        AgentError::Protocol {
            detail: detail.into(),
            source: None,
        }
    }

    /// The agent refused the credentials.
    pub fn unauthenticated(detail: impl Into<String>) -> Self {
        AgentError::Unauthenticated {
            detail: detail.into(),
        }
    }

    /// Keeps `source` as the cause of an `Unreachable` or `Protocol` error; the other variants
    /// carry peer text and have no source, so they are returned unchanged.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        match &mut self {
            AgentError::Unreachable { source: slot, .. }
            | AgentError::Protocol { source: slot, .. } => *slot = Some(source.into()),
            AgentError::Rejected(_)
            | AgentError::TaskNotFound(_)
            | AgentError::Unsupported(_)
            | AgentError::NotCancelable(_)
            | AgentError::Unauthenticated { .. }
            | AgentError::RateLimited { .. } => {}
        }
        self
    }

    /// What may be shown to the chat's users: the agent's own message where it gave one that is
    /// about the request, otherwise fixed text for the class. Never transport text (URLs,
    /// proxy bodies): that goes to the operator through [`report`](orch_core::report).
    pub fn public_detail(&self) -> String {
        match self {
            AgentError::Rejected(m)
            | AgentError::TaskNotFound(m)
            | AgentError::Unsupported(m)
            | AgentError::NotCancelable(m) => m.clone(),
            AgentError::Unreachable { .. } => "the agent could not be reached".to_owned(),
            AgentError::Protocol { .. } => "the agent sent an unexpected response".to_owned(),
            AgentError::Unauthenticated { .. } => {
                "the agent did not accept the orchestrator's credentials".to_owned()
            }
            AgentError::RateLimited { .. } => "the agent is limiting requests".to_owned(),
        }
    }
}

impl Classify for AgentError {
    fn class(&self) -> ErrorClass {
        match self {
            AgentError::Unreachable { .. } | AgentError::Protocol { .. } => ErrorClass::Transient,
            AgentError::RateLimited { .. } => ErrorClass::RateLimited,
            AgentError::Unauthenticated { .. } => ErrorClass::Unauthenticated,
            AgentError::Rejected(_) => ErrorClass::Invalid,
            AgentError::TaskNotFound(_) => ErrorClass::NotFound,
            AgentError::Unsupported(_) => ErrorClass::Unsupported,
            AgentError::NotCancelable(_) => ErrorClass::Rejected,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            AgentError::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}

/// Talking to a delegated agent. Protocol adapters (A2A, …) implement this.
pub trait AgentClient: Send + Sync + 'static {
    /// Reads the live agent card.
    fn read_card(
        &self,
        ep: &AgentEndpoint,
    ) -> impl Future<Output = Result<AgentCardInfo, AgentError>> + Send;

    /// Sends a message and streams the agent's progress (the first envelope names the task).
    fn send_stream(
        &self,
        req: SendRequest,
    ) -> impl Future<Output = Result<AgentStream, AgentError>> + Send;

    /// Re-attaches to a running task (A2A `SubscribeToTask`). Fails with
    /// [`AgentError::TaskNotFound`] for a finished task.
    fn resubscribe(
        &self,
        task: &TaskHandle,
    ) -> impl Future<Output = Result<AgentStream, AgentError>> + Send;

    /// Polls a task (A2A `GetTask`).
    fn get_task(
        &self,
        task: &TaskHandle,
    ) -> impl Future<Output = Result<TaskSnapshot, AgentError>> + Send;

    /// Cancels a task (A2A `CancelTask`).
    fn cancel(
        &self,
        task: &TaskHandle,
    ) -> impl Future<Output = Result<TaskSnapshot, AgentError>> + Send;

    /// Best-effort recovery after a crash between sending and recording: finds the task created
    /// by `message_id` in `context_id`. `Ok(None)` if not found or unsupported.
    fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<Option<String>, AgentError>> + Send;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn all() -> Vec<AgentError> {
        vec![
            AgentError::unreachable("card fetch failed"),
            AgentError::Rejected("bad params".into()),
            AgentError::TaskNotFound("t1".into()),
            AgentError::Unsupported("nope".into()),
            AgentError::NotCancelable("done".into()),
            AgentError::protocol("odd answer"),
            AgentError::unauthenticated("401"),
            AgentError::RateLimited { retry_after: None },
        ]
    }

    #[test]
    fn debug_never_prints_the_bearer() {
        let ep = AgentEndpoint::a2a(
            AgentId::new("coder"),
            "https://coder.example.com/card.json",
            Some("s3cret".into()),
        );
        let text = format!("{ep:?} {:?}", ep.transport);
        assert!(!text.contains("s3cret"), "{text}");
        assert!(text.contains("<redacted>") && text.contains("coder.example.com"));
        assert!(
            !format!("{:?}", AgentEndpoint::a2a(AgentId::new("a"), "u", None)).contains("redacted")
        );
    }

    #[test]
    fn a_local_endpoint_names_its_kind_and_carries_no_secret() {
        let ep = AgentEndpoint::local(AgentId::new("helper"), "echo");
        assert_eq!(
            ep.transport,
            AgentTransport::Local {
                name: "echo".into()
            }
        );
        assert_ne!(ep, AgentEndpoint::a2a(AgentId::new("helper"), "echo", None));
        let text = format!("{ep:?}");
        assert!(text.contains("Local") && text.contains("echo"), "{text}");
    }

    #[test]
    fn class_table() {
        for e in all() {
            // Exhaustive: a new variant forces a class decision.
            let expected = match &e {
                AgentError::Unreachable { .. } => ErrorClass::Transient,
                AgentError::Protocol { .. } => ErrorClass::Transient,
                AgentError::RateLimited { .. } => ErrorClass::RateLimited,
                AgentError::Unauthenticated { .. } => ErrorClass::Unauthenticated,
                AgentError::Rejected(_) => ErrorClass::Invalid,
                AgentError::TaskNotFound(_) => ErrorClass::NotFound,
                AgentError::Unsupported(_) => ErrorClass::Unsupported,
                AgentError::NotCancelable(_) => ErrorClass::Rejected,
            };
            assert_eq!(e.class(), expected, "{e}");
            assert_eq!(e.is_retryable(), expected.is_retryable());
        }
    }

    #[test]
    fn retry_after_comes_from_rate_limiting_only() {
        let e = AgentError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        };
        assert_eq!(e.retry_after(), Some(Duration::from_secs(7)));
        assert_eq!(AgentError::unreachable("x").retry_after(), None);
    }

    #[test]
    fn the_source_is_kept_and_never_printed_twice() {
        let cause = std::io::Error::other("connection refused to http://10.0.0.1/secret");
        let e = AgentError::unreachable("card fetch failed").with_source(cause);
        assert!(std::error::Error::source(&e).is_some());
        assert!(!e.to_string().contains("connection refused"));
        assert_eq!(
            orch_core::report(&e),
            "agent unreachable: card fetch failed: connection refused to http://10.0.0.1/secret"
        );
    }

    #[test]
    fn public_detail_never_carries_transport_text() {
        let cause = std::io::Error::other("dial http://internal.example:9/token=abc");
        let e = AgentError::unreachable("card fetch failed").with_source(cause);
        assert_eq!(e.public_detail(), "the agent could not be reached");
        assert_eq!(
            AgentError::Rejected("release nightly is unknown".into()).public_detail(),
            "release nightly is unknown"
        );
        for e in all() {
            let d = e.public_detail();
            assert!(!d.contains("http"), "{d}");
        }
    }
}
