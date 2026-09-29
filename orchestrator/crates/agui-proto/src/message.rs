//! Conversation messages, content parts, tools and other members of `RunAgentInput`.
//!
//! Optional members are omitted when `None`, never serialised as `null`. On input, members the
//! schema does not declare are ignored by serde; [`RunAgentInput::parse`](crate::RunAgentInput::parse)
//! reports which ones were dropped.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{MessageId, SubagentRunId, ToolCallId};

/// Extra information attached to an event, message, tool call, tool, interrupt or resume
/// entry. Open by key; the key `ag-ui` is reserved for the protocol, and our own keys carry a
/// vendor prefix.
pub type Metadata = serde_json::Map<String, Value>;

/// Who a text message is from (`TextMessageRole`): the four roles that carry plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextMessageRole {
    /// Developer instructions.
    Developer,
    /// System prompt.
    System,
    /// The agent. An absent role means this.
    Assistant,
    /// The person.
    User,
}

/// The fixed role of a tool result event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolRole {
    /// `"tool"`.
    #[default]
    Tool,
}

/// The fixed role of a reasoning message event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningRole {
    /// `"reasoning"`.
    #[default]
    Reasoning,
}

/// The only kind of tool call the protocol models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallKind {
    /// `"function"`.
    #[default]
    Function,
}

/// A message body: plain text, or an ordered list of parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text.
    Text(String),
    /// Parts of a multimodal message.
    Parts(Vec<ContentPart>),
}

impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

/// One part of a message body. Closed over the five part types of 1.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum ContentPart {
    /// Text.
    Text {
        /// Identifies the part within its message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// The text.
        text: String,
        /// Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    /// An image.
    Image {
        /// Identifies the part within its message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Where the bytes come from.
        source: PartSource,
        /// Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    /// Audio.
    Audio {
        /// Identifies the part within its message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Where the bytes come from.
        source: PartSource,
        /// Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    /// Video.
    Video {
        /// Identifies the part within its message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Where the bytes come from.
        source: PartSource,
        /// Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    /// A document.
    Document {
        /// Identifies the part within its message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Where the bytes come from.
        source: PartSource,
        /// Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
}

/// Where a media part's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum PartSource {
    /// Bytes carried inline.
    Data {
        /// Base64-encoded bytes (not decoded here).
        value: String,
        /// What the bytes are.
        mime_type: String,
    },
    /// Bytes at a URL.
    Url {
        /// The URL.
        value: String,
        /// What the bytes are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// Bytes already at a provider, under a handle it issued.
    File {
        /// The provider's handle.
        value: String,
        /// Which provider.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// What the bytes are.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

/// What is being called, and with what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The tool's name.
    pub name: String,
    /// The arguments as a JSON-encoded string.
    pub arguments: String,
}

/// A call an assistant message made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// Identifies the call.
    pub id: ToolCallId,
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: ToolCallKind,
    /// The function.
    pub function: FunctionCall,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl ToolCall {
    /// A function call with no optional members.
    pub fn new(
        id: impl Into<ToolCallId>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
            encrypted_value: None,
            metadata: None,
        }
    }
}

/// A developer message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// The text.
    pub content: String,
    /// Display name of the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// A system message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// The text.
    pub content: String,
    /// Display name of the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// An assistant message. Content is optional: a turn may be tool calls only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// What the agent said, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// The tool calls this turn made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Display name of the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// A user message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// Text, or parts for a multimodal message.
    pub content: MessageContent,
    /// Display name of the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// What a tool returned, as a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// Text, or parts.
    pub content: MessageContent,
    /// The call this answers.
    pub tool_call_id: ToolCallId,
    /// Why the tool failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// Structured progress that is not conversation content, kept in the message sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// What kind of activity this is (open string).
    pub activity_type: String,
    /// The payload, open by key.
    pub content: Metadata,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// A span of the agent's reasoning, as a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningMessage {
    /// Identifies the message.
    pub id: MessageId,
    /// The reasoning text.
    pub content: String,
    /// A provider's opaque artefact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_value: Option<String>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
    /// The subagent invocation this belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_run_id: Option<SubagentRunId>,
}

/// A conversation message, discriminated by `role`. Closed over the seven roles of 1.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    /// See [`DeveloperMessage`].
    Developer(DeveloperMessage),
    /// See [`SystemMessage`].
    System(SystemMessage),
    /// See [`AssistantMessage`].
    Assistant(AssistantMessage),
    /// See [`UserMessage`].
    User(UserMessage),
    /// See [`ToolMessage`].
    Tool(ToolMessage),
    /// See [`ActivityMessage`].
    Activity(ActivityMessage),
    /// See [`ReasoningMessage`].
    Reasoning(ReasoningMessage),
}

impl Message {
    /// The message id, whatever the role.
    pub fn id(&self) -> &MessageId {
        match self {
            Message::Developer(m) => &m.id,
            Message::System(m) => &m.id,
            Message::Assistant(m) => &m.id,
            Message::User(m) => &m.id,
            Message::Tool(m) => &m.id,
            Message::Activity(m) => &m.id,
            Message::Reasoning(m) => &m.id,
        }
    }

    /// The wire spelling of the role.
    pub fn role(&self) -> &'static str {
        match self {
            Message::Developer(_) => "developer",
            Message::System(_) => "system",
            Message::Assistant(_) => "assistant",
            Message::User(_) => "user",
            Message::Tool(_) => "tool",
            Message::Activity(_) => "activity",
            Message::Reasoning(_) => "reasoning",
        }
    }

    /// A user message with plain text and no optional members.
    pub fn user(id: impl Into<MessageId>, text: impl Into<String>) -> Self {
        Message::User(UserMessage {
            id: id.into(),
            content: MessageContent::Text(text.into()),
            name: None,
            encrypted_value: None,
            metadata: None,
            subagent_run_id: None,
        })
    }
}

/// A frontend tool the agent may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// The tool's name.
    pub name: String,
    /// What the tool does.
    pub description: String,
    /// A JSON Schema for the arguments, carried opaquely. Any non-null JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    /// Extra information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

/// A named piece of ambient information for the run, distinct from the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// What this context is.
    pub description: String,
    /// The context itself.
    pub value: String,
}
