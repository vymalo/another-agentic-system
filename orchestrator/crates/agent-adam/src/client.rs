//! [`LocalAgentClient`]: the [`AgentClient`] port over agents hosted in this process.
//!
//! Every call goes to an [`adam_a2a::TaskBackend`] over the adam-rs runtime, the same seam
//! that serves a remote client over HTTP, and the results go through the same
//! `orch_a2a_mapping` functions the HTTP adapter uses. The keys of the envelopes are therefore
//! identical to what an A2A agent would produce, and the dispatcher cannot tell the two apart.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use a2a::{Message, Part, Role, StreamResponse};
use adam_a2a::{BackendError, Caller, TaskBackend as _, TaskEvent};
use adam_a2a_runtime::{RuntimeTaskBackend, task_id_for};
use futures::StreamExt as _;
use futures::stream::BoxStream;
use orch_a2a_mapping::{StreamMapper, snapshot};
use orch_ports::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentError, AgentStream, AgentTransport,
    SendContent, SendRequest, TaskHandle, TaskSnapshot,
};

/// One hosted kind: its card text and the backend that runs its tasks.
#[derive(Debug)]
pub(crate) struct Entry {
    description: String,
    backend: RuntimeTaskBackend,
}

impl Entry {
    pub(crate) fn new(description: String, backend: RuntimeTaskBackend) -> Self {
        Entry {
            description,
            backend,
        }
    }
}

/// [`AgentClient`] for the endpoints of the agents a [`LocalAgents`](crate::LocalAgents) hosts.
///
/// Cheap to clone. It serves `AgentTransport::Local` endpoints only: an A2A endpoint is answered
/// `Unsupported` (the composition root routes by transport, this is the second line of
/// defence), and a local endpoint of a kind this process does not host is `Unreachable`, which
/// is transient: a process that does host it may pick the work up.
///
/// `LocalAgentClient::default()` hosts nothing: every local endpoint is `Unreachable`. It is what a
/// composition that routes by transport uses when no local agent is configured.
///
/// A task belongs to the caller `orch:<agent id>` (the configuration key of the agent), so two
/// configured agents of one kind never see each other's tasks.
#[derive(Debug, Clone, Default)]
pub struct LocalAgentClient {
    entries: Arc<HashMap<String, Entry>>,
}

impl LocalAgentClient {
    pub(crate) fn new(entries: Arc<HashMap<String, Entry>>) -> Self {
        LocalAgentClient { entries }
    }

    /// The kind's entry and the caller for `ep`.
    fn resolve(&self, ep: &AgentEndpoint) -> Result<(&Entry, String, Caller), AgentError> {
        let name = match &ep.transport {
            AgentTransport::Local { name } => name.as_str(),
            AgentTransport::A2a { .. } => {
                return Err(AgentError::Unsupported(format!(
                    "agent {} is a remote A2A agent; the local client only serves agents hosted \
                     in this process",
                    ep.id
                )));
            }
        };
        match self.entries.get(name) {
            Some(entry) => Ok((
                entry,
                name.to_owned(),
                Caller::new(format!("orch:{}", ep.id)),
            )),
            None => Err(AgentError::unreachable(format!(
                "this process does not host the local agent {name:?}"
            ))),
        }
    }
}

/// The seam's error as the port's. The seam keeps its own text away from users; so does this:
/// only what the agent said about the request (`Rejected`, `NotCancelable`) is passed on.
fn agent_error(err: BackendError) -> AgentError {
    match err {
        BackendError::TaskNotFound(task) => AgentError::TaskNotFound(task),
        BackendError::NotCancelable { task_id, state } => {
            AgentError::NotCancelable(format!("task {task_id} is {state} and cannot be canceled"))
        }
        BackendError::InvalidParams(detail) => AgentError::Rejected(detail),
        err @ BackendError::Unavailable { .. } => {
            AgentError::unreachable("the local agents' runtime is unavailable").with_source(err)
        }
        err => AgentError::protocol("the local agents' runtime failed").with_source(err),
    }
}

fn stream_response(event: TaskEvent) -> StreamResponse {
    match event {
        TaskEvent::Snapshot(task) => StreamResponse::Task(task),
        TaskEvent::Status(update) => StreamResponse::StatusUpdate(update),
        TaskEvent::Artifact(update) => StreamResponse::ArtifactUpdate(update),
    }
}

type Item = Result<orch_ports::AgentEnvelope, AgentError>;

struct Mapping {
    inner: BoxStream<'static, Result<TaskEvent, BackendError>>,
    mapper: StreamMapper,
    queue: VecDeque<Item>,
    seen_any: bool,
    ended: bool,
}

/// Maps the seam's event stream to envelopes, the way the HTTP adapter maps the SDK's stream:
/// through [`StreamMapper`], so the keys are the same. The stream ends after the event that
/// leaves the task finished or waiting for its caller (the seam's own rule, applied by the A2A
/// server before a client sees the stream) and after the first error. A stream that ends
/// without a single event is an error, not an empty turn.
fn map_events(inner: BoxStream<'static, Result<TaskEvent, BackendError>>) -> AgentStream {
    let state = Mapping {
        inner,
        mapper: StreamMapper::default(),
        queue: VecDeque::new(),
        seen_any: false,
        ended: false,
    };
    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                return Some((item, st));
            }
            if st.ended {
                return None;
            }
            match st.inner.next().await {
                Some(Ok(event)) => {
                    st.seen_any = true;
                    st.ended = event.ends_stream();
                    let mapped = st.mapper.map(stream_response(event));
                    st.queue.extend(mapped);
                }
                Some(Err(e)) => {
                    st.seen_any = true;
                    st.ended = true;
                    st.queue.push_back(Err(agent_error(e)));
                }
                None => {
                    st.ended = true;
                    if !st.seen_any {
                        st.queue.push_back(Err(AgentError::protocol(
                            "the local agent's task stream closed without an event",
                        )));
                    }
                }
            }
        }
    })
    .boxed()
}

impl AgentClient for LocalAgentClient {
    async fn read_card(&self, ep: &AgentEndpoint) -> Result<AgentCardInfo, AgentError> {
        let (entry, _, _) = self.resolve(ep)?;
        Ok(AgentCardInfo {
            description: Some(entry.description.clone()),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            releases: None,
            ui: None,
        })
    }

    async fn send_stream(&self, req: SendRequest) -> Result<AgentStream, AgentError> {
        let (entry, _, caller) = self.resolve(&req.endpoint)?;
        if req.release.is_some() {
            // Like the A2A adapter towards a card without release channels: running the
            // default release when another was asked for would be worse than refusing.
            return Err(AgentError::Rejected(
                "a local agent does not offer release channels; refusing to run a default release"
                    .to_owned(),
            ));
        }
        let text = match &req.content {
            SendContent::Text(text) => text.clone(),
            SendContent::UiAction { .. } => {
                return Err(AgentError::Rejected(
                    "a local agent does not offer A2UI surfaces, so it takes no UI action"
                        .to_owned(),
                ));
            }
        };
        let mut message = Message::new(Role::User, vec![Part::text(text)]);
        message.message_id = req.message_id.clone();
        message.context_id = Some(req.context_id.clone());
        message.task_id = req.task_id.clone();
        let task = entry
            .backend
            .submit(caller.clone(), message, req.task_id, Some(req.context_id))
            .await
            .map_err(agent_error)?;
        Ok(map_events(entry.backend.subscribe(&caller, &task.id)))
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        let (entry, _, caller) = self.resolve(&task.endpoint)?;
        // The A2A rule: a finished task cannot be re-attached to (the dispatcher polls it).
        match entry
            .backend
            .get(&caller, &task.task_id)
            .await
            .map_err(agent_error)?
        {
            Some(found) if !found.status.state.is_terminal() => {
                Ok(map_events(entry.backend.subscribe(&caller, &found.id)))
            }
            Some(_) | None => Err(AgentError::TaskNotFound(task.task_id.clone())),
        }
    }

    async fn get_task(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let (entry, _, caller) = self.resolve(&task.endpoint)?;
        let found = entry
            .backend
            .get(&caller, &task.task_id)
            .await
            .map_err(agent_error)?
            .ok_or_else(|| AgentError::TaskNotFound(task.task_id.clone()))?;
        snapshot(&found)
    }

    async fn cancel(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let (entry, _, caller) = self.resolve(&task.endpoint)?;
        let canceled = entry
            .backend
            .cancel(&caller, &task.task_id)
            .await
            .map_err(agent_error)?;
        snapshot(&canceled)
    }

    /// A new task's id is derived from the agent, the caller, the context and the message id
    /// ([`task_id_for`]), so the task a message started is found without a side table, and it
    /// is `Some` only if that task exists. A follow-up message that joined an existing task
    /// started nothing, so it is not found (`None`, which the trait allows).
    async fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: &str,
        message_id: &str,
    ) -> Result<Option<String>, AgentError> {
        let (entry, kind, caller) = self.resolve(ep)?;
        let id = task_id_for(&kind, &caller.subject, Some(context_id), message_id).to_string();
        Ok(entry
            .backend
            .get(&caller, &id)
            .await
            .map_err(agent_error)?
            .map(|task| task.id))
    }
}
