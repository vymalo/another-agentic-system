use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{AgentId, ThreadId, UserId};
use crate::thread::ThreadState;

/// Contract `EventKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A message from the user.
    UserMessage,
    /// A message from an agent.
    AgentMessage,
    /// A task state change reported by the agent.
    AgentStatus,
    /// A produced artifact.
    Artifact,
    /// The thread entered a new state.
    ThreadState,
    /// Something went wrong.
    Error,
}

impl EventKind {
    /// The wire spelling (also the SSE `event:` name).
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::UserMessage => "user_message",
            EventKind::AgentMessage => "agent_message",
            EventKind::AgentStatus => "agent_status",
            EventKind::Artifact => "artifact",
            EventKind::ThreadState => "thread_state",
            EventKind::Error => "error",
        }
    }
}

/// Contract `Actor.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// A human.
    User,
    /// A delegated agent.
    Agent,
    /// The orchestrator itself.
    System,
}

/// Contract `Actor`: who produced an event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Actor {
    /// Kind of actor.
    #[serde(rename = "type")]
    pub r#type: ActorType,
    /// User e-mail, agent id, or `orchestrator`.
    pub name: String,
    /// Agent revision that produced the event, when known (ADR 0008 echo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl Actor {
    /// The user actor.
    pub fn user(user: &UserId) -> Self {
        Actor {
            r#type: ActorType::User,
            name: user.to_string(),
            revision: None,
        }
    }

    /// An agent actor, with the revision when known.
    pub fn agent(agent: &AgentId, revision: Option<String>) -> Self {
        Actor {
            r#type: ActorType::Agent,
            name: agent.to_string(),
            revision,
        }
    }

    /// The orchestrator.
    pub fn system() -> Self {
        Actor {
            r#type: ActorType::System,
            name: "orchestrator".to_owned(),
            revision: None,
        }
    }
}

/// Contract `agent_status.status` (mirrors A2A `TaskState`; note the contract spelling
/// `input_required` / `canceled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Accepted by the agent.
    Submitted,
    /// Being worked on.
    Working,
    /// Waiting for user input.
    InputRequired,
    /// Finished successfully.
    Completed,
    /// Failed.
    Failed,
    /// Cancelled.
    Canceled,
}

/// `data` of a `user_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageData {
    /// Message text.
    pub text: String,
}

/// `data` of an `agent_message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageData {
    /// Message text.
    pub text: String,
    /// Stable message id (a partial is replaced by the next message with the same id).
    pub message_id: String,
    /// `false` for a streamed partial.
    #[serde(rename = "final")]
    pub is_final: bool,
}

/// `data` of an `agent_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatusData {
    /// The status.
    pub status: AgentStatus,
    /// Detail text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `data` of an `artifact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactData {
    /// Artifact name.
    pub name: String,
    /// Media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Inline text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// `data` of a `thread_state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStateData {
    /// The state the thread entered.
    pub state: ThreadState,
}

/// `data` of an `error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorData {
    /// What went wrong.
    pub message: String,
    /// Whether retrying (e.g. sending another message) can help.
    pub retryable: bool,
}

/// The kind-specific payload of an event (contract `EventData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventBody {
    /// See [`UserMessageData`].
    UserMessage(UserMessageData),
    /// See [`AgentMessageData`].
    AgentMessage(AgentMessageData),
    /// See [`AgentStatusData`].
    AgentStatus(AgentStatusData),
    /// See [`ArtifactData`].
    Artifact(ArtifactData),
    /// See [`ThreadStateData`].
    ThreadState(ThreadStateData),
    /// See [`ErrorData`].
    Error(ErrorData),
}

impl EventBody {
    /// The kind tag of this body.
    pub fn kind(&self) -> EventKind {
        match self {
            EventBody::UserMessage(_) => EventKind::UserMessage,
            EventBody::AgentMessage(_) => EventKind::AgentMessage,
            EventBody::AgentStatus(_) => EventKind::AgentStatus,
            EventBody::Artifact(_) => EventKind::Artifact,
            EventBody::ThreadState(_) => EventKind::ThreadState,
            EventBody::Error(_) => EventKind::Error,
        }
    }

    /// The `data` JSON object. Optional fields are omitted, never `null`.
    pub fn data_value(&self) -> Value {
        let value = match self {
            EventBody::UserMessage(d) => serde_json::to_value(d),
            EventBody::AgentMessage(d) => serde_json::to_value(d),
            EventBody::AgentStatus(d) => serde_json::to_value(d),
            EventBody::Artifact(d) => serde_json::to_value(d),
            EventBody::ThreadState(d) => serde_json::to_value(d),
            EventBody::Error(d) => serde_json::to_value(d),
        };
        // Plain structs of strings/bools/enums always serialise.
        value.unwrap_or(Value::Null)
    }

    /// Rebuilds a body from its kind and `data` (used when reading the log back).
    pub fn from_parts(kind: EventKind, data: Value) -> Result<Self, serde_json::Error> {
        Ok(match kind {
            EventKind::UserMessage => EventBody::UserMessage(serde_json::from_value(data)?),
            EventKind::AgentMessage => EventBody::AgentMessage(serde_json::from_value(data)?),
            EventKind::AgentStatus => EventBody::AgentStatus(serde_json::from_value(data)?),
            EventKind::Artifact => EventBody::Artifact(serde_json::from_value(data)?),
            EventKind::ThreadState => EventBody::ThreadState(serde_json::from_value(data)?),
            EventKind::Error => EventBody::Error(serde_json::from_value(data)?),
        })
    }
}

/// One entry of a thread's append-only event log (contract `Event`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Strictly increasing within a thread, starting at 1.
    pub seq: i64,
    /// Owning thread.
    pub thread_id: ThreadId,
    /// When the event was recorded.
    pub at: Timestamp,
    /// Who produced it.
    pub actor: Actor,
    /// The payload.
    pub body: EventBody,
}

impl Event {
    /// The kind tag.
    pub fn kind(&self) -> EventKind {
        self.body.kind()
    }
}

/// Exact wire shape `{seq, threadId, at, kind, actor, data}`.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventWire {
    seq: i64,
    thread_id: ThreadId,
    at: Timestamp,
    kind: EventKind,
    actor: Actor,
    data: Value,
}

impl Serialize for Event {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        EventWire {
            seq: self.seq,
            thread_id: self.thread_id,
            at: self.at,
            kind: self.body.kind(),
            actor: self.actor.clone(),
            data: self.body.data_value(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = EventWire::deserialize(deserializer)?;
        let body = EventBody::from_parts(wire.kind, wire.data).map_err(serde::de::Error::custom)?;
        Ok(Event {
            seq: wire.seq,
            thread_id: wire.thread_id,
            at: wire.at,
            actor: wire.actor,
            body,
        })
    }
}
