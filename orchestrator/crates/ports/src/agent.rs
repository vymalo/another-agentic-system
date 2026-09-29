use std::fmt;
use std::future::Future;

use futures::stream::BoxStream;
use orch_core::{AgentId, AgentTaskState, AgentUpdate, Releases};

/// Where an agent lives. `Debug` never prints the bearer token.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentEndpoint {
    /// Configuration key.
    pub id: AgentId,
    /// URL of the agent card.
    pub card_url: String,
    /// Bearer token for the agent, if configured.
    pub bearer: Option<String>,
}

impl fmt::Debug for AgentEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentEndpoint")
            .field("id", &self.id)
            .field("card_url", &self.card_url)
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .finish()
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// Could not reach the agent. Retryable.
    #[error("agent unreachable: {0}")]
    Unreachable(String),
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
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl AgentError {
    /// `Unreachable` and `Protocol` are worth retrying.
    pub fn is_retryable(&self) -> bool {
        match self {
            AgentError::Unreachable(_) | AgentError::Protocol(_) => true,
            AgentError::Rejected(_)
            | AgentError::TaskNotFound(_)
            | AgentError::Unsupported(_)
            | AgentError::NotCancelable(_) => false,
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
