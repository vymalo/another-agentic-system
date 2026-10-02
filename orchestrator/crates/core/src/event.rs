use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::description::ThreadDescribedData;
use crate::fork::ThreadForkedData;
use crate::gate::{CheckResult, CiReport, ReworkData};
use crate::ids::{AgentId, ThreadId, UserId};
use crate::step::AgentStepData;
use crate::thread::ThreadState;
use crate::title::ThreadTitledData;
use crate::ui::{UiActionData, UiSurfaceData};
use crate::ui_catalog::UiCatalogData;

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
    /// An agent sent (part of) an A2UI surface (ADR 0013).
    UiSurface,
    /// The user acted on an A2UI surface (ADR 0013).
    UiAction,
    /// A CI provider reported a completed check on the pushed commit (ADR 0017).
    CiResult,
    /// A source of the verification gate answered (ADR 0018).
    CheckResult,
    /// The gate failed and the agent was sent back to work (ADR 0018).
    Rework,
    /// A message on a finished thread started its next job (ADR 0020).
    JobStarted,
    /// The person's screen sent a version of its UI component catalog (ADR 0023).
    UiCatalog,
    /// A step of the agent's work started, moved or ended (ADR 0025).
    AgentStep,
    /// The thread has a new title: a person renamed it.
    ThreadTitled,
    /// The thread began as a copy of another (ADR 0029): a fork or an edited message.
    ThreadForked,
    /// The thread has a new description: the model wrote one, or a person wrote or cleared it
    /// (ADR 0035).
    ThreadDescribed,
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
            EventKind::UiSurface => "ui_surface",
            EventKind::UiAction => "ui_action",
            EventKind::CiResult => "ci_result",
            EventKind::CheckResult => "check_result",
            EventKind::Rework => "rework",
            EventKind::JobStarted => "job_started",
            EventKind::UiCatalog => "ui_catalog",
            EventKind::AgentStep => "agent_step",
            EventKind::ThreadTitled => "thread_titled",
            EventKind::ThreadForked => "thread_forked",
            EventKind::ThreadDescribed => "thread_described",
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
    /// Waiting for the user to authenticate somewhere (A2A `auth-required`). The thread is
    /// blocked and resumable exactly as for [`AgentStatus::InputRequired`]; the detail names
    /// what to authenticate to.
    AuthRequired,
    /// Finished successfully.
    Completed,
    /// Failed.
    Failed,
    /// Cancelled.
    Canceled,
}

/// Which surface a user message came in through (ADR 0019). Closed (ADR 0004): a new surface that
/// speaks for a user adds a variant.
///
/// [`Origin::Agui`] is the default, and the log does not spell it: an event without an `origin`
/// reads as `agui`, so every log written before the field existed reads as it always did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The chat, over AG-UI (the web, or any AG-UI consumer).
    #[default]
    Agui,
    /// An MCP client (Claude Code, opencode, ...), through the MCP server.
    Mcp,
}

impl Origin {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Agui => "agui",
            Origin::Mcp => "mcp",
        }
    }

    /// Whether this is the default, which the log leaves out.
    pub fn is_default(&self) -> bool {
        *self == Origin::default()
    }
}

/// `data` of a `user_message`.
///
/// `message_id` and `run_id` are set when the message came from a surface that names them (an
/// AG-UI message id and run id); both are absent, never `null`, otherwise. `origin` is absent for
/// a message from the chat (`agui`) and `mcp` for one an MCP client sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessageData {
    /// Message text.
    pub text: String,
    /// The id the surface gave this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// The id of the run this message started or continued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// The surface the message came in through; absent (the default, `agui`) in older logs.
    #[serde(default, skip_serializing_if = "Origin::is_default")]
    pub origin: Origin,
}

impl UserMessageData {
    /// A message from the chat with no surface-assigned ids.
    pub fn new(text: impl Into<String>) -> Self {
        UserMessageData {
            text: text.into(),
            message_id: None,
            run_id: None,
            origin: Origin::default(),
        }
    }
}

/// What an agent's words are for within its turn (ADR 0031).
///
/// The adapter reads it off the A2A status the text came in: words stated on a `working` status
/// are said while the agent is still going (the sentence before a tool call), and words stated
/// on a status that ends the turn (`completed`, `input_required`, `auth_required`) are what the
/// turn ends with. A message that came as a plain A2A `Message` says nothing about it, and the
/// field is absent (as in every log written before it existed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessagePurpose {
    /// Working text: said while the agent works, before a tool call or between steps.
    Working,
    /// The turn's answer: the words that end the turn.
    Answer,
}

impl MessagePurpose {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MessagePurpose::Working => "working",
            MessagePurpose::Answer => "answer",
        }
    }
}

/// How an answer was announced, when it was not by the status that ends the turn (ADR 0031).
///
/// Written by the `turn_output` thread tool ([`Input::Answer`](crate::Input::Answer)): an agent
/// that announces its answer and goes on working. It is a closed enum, so a new way is a new
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerVia {
    /// The agent called the `turn_output` thread tool.
    TurnOutput,
}

impl AnswerVia {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            AnswerVia::TurnOutput => "turn_output",
        }
    }
}

/// `data` of an `agent_message`.
///
/// `purpose` and `via` are absent when nothing says (a plain A2A `Message`, and every log written
/// before ADR 0031), never `null`.
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
    /// What the words are for in the turn, when the agent's protocol says ([`MessagePurpose`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<MessagePurpose>,
    /// How an answer was announced, when it was not by the turn's last status ([`AnswerVia`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<AnswerVia>,
}

impl AgentMessageData {
    /// A final message that says nothing about its purpose: what a plain A2A `Message` is.
    pub fn plain(message_id: impl Into<String>, text: impl Into<String>) -> Self {
        AgentMessageData {
            text: text.into(),
            message_id: message_id.into(),
            is_final: true,
            purpose: None,
            via: None,
        }
    }
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
    /// The file this artifact is, when an agent handed one over and the artifact store kept it
    /// (ADR 0032). Then `mime_type` is the type the worker sniffed, and the bytes are in the store,
    /// never in the log. Absent for an artifact of text or a link, and for a file that was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileRef>,
}

/// A file kept in the artifact store, as the log refers to it (ADR 0032): the content's hash (the
/// second half of the store's key `threads/<thread>/<sha256>`, the thread being the event's), its
/// size and the name the agent gave it. A decision reads this, never the bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    /// The SHA-256 of the content, 64 lowercase hexadecimal digits.
    pub sha256: String,
    /// The size in bytes.
    pub size: u64,
    /// The file's name as the agent gave it, cleaned of path parts and control characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

impl FileRef {
    /// Where the file is served, relative to the API's origin
    /// (`GET /api/threads/{threadId}/artifacts/{sha256}`, ADR 0032): the `href` of the projected
    /// artifact. The thread is the event's.
    pub fn href(&self, thread: crate::ThreadId) -> String {
        format!("/api/threads/{thread}/artifacts/{}", self.sha256)
    }
}

/// How much of a kept file a person can look at without downloading it (ADR 0032, decision 8 and
/// 10): the types the API serves inline. Everything else is an attachment only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preview {
    /// An image: png, jpeg, gif, webp or svg (the API sanitizes an SVG).
    Image,
    /// Plain text: `text/plain` or `application/json`.
    Text,
}

impl Preview {
    /// The preview of `media_type` (a lower-case `type/subtype` as the worker kept it), or `None`
    /// when the file is an attachment only.
    pub fn of(media_type: &str) -> Option<Preview> {
        match media_type {
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml" => {
                Some(Preview::Image)
            }
            "text/plain" | "application/json" => Some(Preview::Text),
            _ => None,
        }
    }

    /// The word the projection uses (`vymalo.artifact.preview`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Preview::Image => "image",
            Preview::Text => "text",
        }
    }
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

/// `data` of a `job_started`: the thread's next job began (ADR 0020). The first job of a thread
/// has no such event; it starts with the thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobStartedData {
    /// The number of the job that started, from 2.
    pub job: u32,
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
    /// See [`UiSurfaceData`].
    UiSurface(UiSurfaceData),
    /// See [`UiActionData`].
    UiAction(UiActionData),
    /// See [`CiReport`].
    CiResult(CiReport),
    /// See [`CheckResult`].
    CheckResult(CheckResult),
    /// See [`ReworkData`].
    Rework(ReworkData),
    /// See [`JobStartedData`].
    JobStarted(JobStartedData),
    /// See [`UiCatalogData`].
    UiCatalog(UiCatalogData),
    /// See [`AgentStepData`].
    AgentStep(AgentStepData),
    /// See [`ThreadTitledData`].
    ThreadTitled(ThreadTitledData),
    /// See [`ThreadForkedData`].
    ThreadForked(ThreadForkedData),
    /// See [`ThreadDescribedData`].
    ThreadDescribed(ThreadDescribedData),
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
            EventBody::UiSurface(_) => EventKind::UiSurface,
            EventBody::UiAction(_) => EventKind::UiAction,
            EventBody::CiResult(_) => EventKind::CiResult,
            EventBody::CheckResult(_) => EventKind::CheckResult,
            EventBody::Rework(_) => EventKind::Rework,
            EventBody::JobStarted(_) => EventKind::JobStarted,
            EventBody::UiCatalog(_) => EventKind::UiCatalog,
            EventBody::AgentStep(_) => EventKind::AgentStep,
            EventBody::ThreadTitled(_) => EventKind::ThreadTitled,
            EventBody::ThreadForked(_) => EventKind::ThreadForked,
            EventBody::ThreadDescribed(_) => EventKind::ThreadDescribed,
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
            EventBody::UiSurface(d) => serde_json::to_value(d),
            EventBody::UiAction(d) => serde_json::to_value(d),
            EventBody::CiResult(d) => serde_json::to_value(d),
            EventBody::CheckResult(d) => serde_json::to_value(d),
            EventBody::Rework(d) => serde_json::to_value(d),
            EventBody::JobStarted(d) => serde_json::to_value(d),
            EventBody::UiCatalog(d) => serde_json::to_value(d),
            EventBody::AgentStep(d) => serde_json::to_value(d),
            EventBody::ThreadTitled(d) => serde_json::to_value(d),
            EventBody::ThreadForked(d) => serde_json::to_value(d),
            EventBody::ThreadDescribed(d) => serde_json::to_value(d),
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
            EventKind::UiSurface => EventBody::UiSurface(serde_json::from_value(data)?),
            EventKind::UiAction => EventBody::UiAction(serde_json::from_value(data)?),
            EventKind::CiResult => EventBody::CiResult(serde_json::from_value(data)?),
            EventKind::CheckResult => EventBody::CheckResult(serde_json::from_value(data)?),
            EventKind::Rework => EventBody::Rework(serde_json::from_value(data)?),
            EventKind::JobStarted => EventBody::JobStarted(serde_json::from_value(data)?),
            EventKind::UiCatalog => EventBody::UiCatalog(serde_json::from_value(data)?),
            EventKind::AgentStep => EventBody::AgentStep(serde_json::from_value(data)?),
            EventKind::ThreadTitled => EventBody::ThreadTitled(serde_json::from_value(data)?),
            EventKind::ThreadForked => EventBody::ThreadForked(serde_json::from_value(data)?),
            EventKind::ThreadDescribed => EventBody::ThreadDescribed(serde_json::from_value(data)?),
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
