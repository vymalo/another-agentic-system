//! The 31 AG-UI 1.0 event types as one closed enum.
//!
//! [`Event`] is `#[serde(tag = "type")]` over one struct per event type, named as in the
//! schema. Envelope members (`timestamp`, `rawEvent`, `metadata`) live in [`BaseFields`] and
//! are flattened in. Optional members are omitted when `None`, never `null`. A `type` this
//! enum does not know fails to deserialise: a consumer that must tolerate newer minor
//! versions has to skip unknown events before handing them to serde.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ids::{MessageId, RunId, SubagentRunId, ThreadId, ToolCallId};
use crate::input::RunAgentInput;
use crate::message::{Message, MessageContent, Metadata, ReasoningRole, TextMessageRole, ToolRole};
use crate::patch::JsonPatch;
use crate::run::{RunFinishedOutcome, SubagentFinishedOutcome, TokenUsage};

/// The members every event may carry, whatever its type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseFields {
    /// When the event was created, in **milliseconds** since the Unix epoch. Informational:
    /// arrival order is the protocol's order. Bounded by ±(2^53 - 1) in the schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// The provider-native event this one was translated from. Any non-null JSON value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_event: Option<Value>,
    /// Extra information. The key `ag-ui` is reserved; our own keys carry a vendor prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

macro_rules! events {
    ($( $(#[$doc:meta])* $variant:ident($name:ident) => $wire:literal {
        $( $(#[$fdoc:meta])* $field:ident : $ty:ty ),* $(,)?
    } )*) => {
        $(
            $(#[$doc])*
            #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
            #[serde(rename_all = "camelCase")]
            pub struct $name {
                /// Envelope members.
                #[serde(flatten)]
                pub base: BaseFields,
                $( $(#[$fdoc])* pub $field: $ty, )*
            }

            impl From<$name> for Event {
                fn from(event: $name) -> Self {
                    Event::$variant(event)
                }
            }
        )*

        /// Any AG-UI 1.0 event, discriminated by `type`.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum Event {
            $( $(#[$doc])* $variant($name), )*
        }

        /// The `type` discriminator of an [`Event`], without its payload.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum EventType {
            $( $(#[$doc])* $variant, )*
        }

        impl EventType {
            /// Every event type of 1.0, in schema order.
            pub const ALL: &'static [EventType] = &[ $( EventType::$variant, )* ];

            /// The wire spelling, for example `"RUN_STARTED"`.
            pub fn as_str(self) -> &'static str {
                match self { $( EventType::$variant => $wire, )* }
            }
        }

        impl Event {
            /// The `type` of this event.
            pub fn event_type(&self) -> EventType {
                match self { $( Event::$variant(_) => EventType::$variant, )* }
            }

            /// The envelope members.
            pub fn base(&self) -> &BaseFields {
                match self { $( Event::$variant(e) => &e.base, )* }
            }

            /// The envelope members, mutably.
            pub fn base_mut(&mut self) -> &mut BaseFields {
                match self { $( Event::$variant(e) => &mut e.base, )* }
            }
        }
    };
}

// Declared in the schema's `EventType` order.
events! {
    /// Streamed text: opens a message.
    TextMessageStart(TextMessageStartEvent) => "TEXT_MESSAGE_START" {
        /// The message being built.
        message_id: MessageId,
        /// Who it is from. Absent means assistant.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<TextMessageRole>,
        /// Display name of the author.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Streamed text: appends to a message.
    TextMessageContent(TextMessageContentEvent) => "TEXT_MESSAGE_CONTENT" {
        /// The message.
        message_id: MessageId,
        /// Text to append.
        delta: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Streamed text: closes a message.
    TextMessageEnd(TextMessageEndEvent) => "TEXT_MESSAGE_END" {
        /// The message.
        message_id: MessageId,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Streamed text in one event: start, content and end are inferred.
    TextMessageChunk(TextMessageChunkEvent) => "TEXT_MESSAGE_CHUNK" {
        /// The message (required on the first chunk of one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<MessageId>,
        /// Who it is from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<TextMessageRole>,
        /// Text to append.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        /// Display name of the author.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A tool call begins.
    ToolCallStart(ToolCallStartEvent) => "TOOL_CALL_START" {
        /// The call.
        tool_call_id: ToolCallId,
        /// The tool's name.
        tool_call_name: String,
        /// The assistant message the call belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_message_id: Option<MessageId>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A fragment of a tool call's JSON arguments.
    ToolCallArgs(ToolCallArgsEvent) => "TOOL_CALL_ARGS" {
        /// The call.
        tool_call_id: ToolCallId,
        /// Argument text to append.
        delta: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A tool call's arguments are complete.
    ToolCallEnd(ToolCallEndEvent) => "TOOL_CALL_END" {
        /// The call.
        tool_call_id: ToolCallId,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A tool call in one event: start, args and end are inferred.
    ToolCallChunk(ToolCallChunkEvent) => "TOOL_CALL_CHUNK" {
        /// The call (required on the first chunk of one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<ToolCallId>,
        /// The tool's name (required on the first chunk of one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_name: Option<String>,
        /// The assistant message the call belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_message_id: Option<MessageId>,
        /// Argument text to append.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// What a tool returned. Mints a tool message.
    ToolCallResult(ToolCallResultEvent) => "TOOL_CALL_RESULT" {
        /// The tool message this becomes.
        message_id: MessageId,
        /// The call being answered.
        tool_call_id: ToolCallId,
        /// Text, or parts.
        content: MessageContent,
        /// Always `"tool"` when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<ToolRole>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// The complete agent state.
    StateSnapshot(StateSnapshotEvent) => "STATE_SNAPSHOT" {
        /// Any JSON value.
        snapshot: Value,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A change to the agent state, as an RFC 6902 patch.
    StateDelta(StateDeltaEvent) => "STATE_DELTA" {
        /// The patch.
        delta: JsonPatch,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// The complete set of messages the producer owns.
    MessagesSnapshot(MessagesSnapshotEvent) => "MESSAGES_SNAPSHOT" {
        /// The messages, in order.
        messages: Vec<Message>,
    }
    /// Structured progress that is not conversation content: the full payload.
    ActivitySnapshot(ActivitySnapshotEvent) => "ACTIVITY_SNAPSHOT" {
        /// The activity message this describes.
        message_id: MessageId,
        /// What kind of activity this is (open string).
        activity_type: String,
        /// The payload, open by key.
        content: Metadata,
        /// Whether this overwrites existing content. Absent means it does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replace: Option<bool>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Structured progress: a change to an activity's payload.
    ActivityDelta(ActivityDeltaEvent) => "ACTIVITY_DELTA" {
        /// The activity message this changes.
        message_id: MessageId,
        /// What kind of activity this is (open string).
        activity_type: String,
        /// The patch.
        patch: JsonPatch,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A provider-native event passed through untranslated.
    Raw(RawEvent) => "RAW" {
        /// The provider's event. Any JSON value.
        event: Value,
        /// Which provider or framework.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// An application-defined event. Names carry a vendor prefix; unknown names are ignored.
    Custom(CustomEvent) => "CUSTOM" {
        /// The event's name.
        name: String,
        /// Any JSON value.
        value: Value,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Opens a run.
    RunStarted(RunStartedEvent) => "RUN_STARTED" {
        /// The conversation.
        thread_id: ThreadId,
        /// This run.
        run_id: RunId,
        /// The producer's own `MAJOR.MINOR` version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol_version: Option<String>,
        /// The run that spawned this one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<RunId>,
        /// The input the run started from (boxed: it dwarfs every other event).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Box<RunAgentInput>>,
    }
    /// Ends a run that did not fail.
    RunFinished(RunFinishedEvent) => "RUN_FINISHED" {
        /// The conversation.
        thread_id: ThreadId,
        /// This run.
        run_id: RunId,
        /// The run's return value. Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        /// How it ended. Absent means success.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<RunFinishedOutcome>,
        /// Token usage, one entry per provider and model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Vec<TokenUsage>>,
    }
    /// Ends a run that failed.
    RunError(RunErrorEvent) => "RUN_ERROR" {
        /// What went wrong, for a person.
        message: String,
        /// A machine-readable code (open string).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Token usage accrued before the failure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Vec<TokenUsage>>,
    }
    /// A named step begins.
    StepStarted(StepStartedEvent) => "STEP_STARTED" {
        /// The step's name.
        step_name: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A named step ends.
    StepFinished(StepFinishedEvent) => "STEP_FINISHED" {
        /// The step's name.
        step_name: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A reasoning span begins.
    ReasoningStart(ReasoningStartEvent) => "REASONING_START" {
        /// The span.
        message_id: MessageId,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A reasoning message begins.
    ReasoningMessageStart(ReasoningMessageStartEvent) => "REASONING_MESSAGE_START" {
        /// The message.
        message_id: MessageId,
        /// Always `"reasoning"`.
        role: ReasoningRole,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// Text appended to a reasoning message.
    ReasoningMessageContent(ReasoningMessageContentEvent) => "REASONING_MESSAGE_CONTENT" {
        /// The message.
        message_id: MessageId,
        /// Text to append.
        delta: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A reasoning message ends.
    ReasoningMessageEnd(ReasoningMessageEndEvent) => "REASONING_MESSAGE_END" {
        /// The message.
        message_id: MessageId,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A reasoning message in one event.
    ReasoningMessageChunk(ReasoningMessageChunkEvent) => "REASONING_MESSAGE_CHUNK" {
        /// The message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<MessageId>,
        /// Text to append.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A reasoning span ends.
    ReasoningEnd(ReasoningEndEvent) => "REASONING_END" {
        /// The span.
        message_id: MessageId,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A provider's opaque reasoning artefact.
    ReasoningEncryptedValue(ReasoningEncryptedValueEvent) => "REASONING_ENCRYPTED_VALUE" {
        /// What the value belongs to.
        subtype: ReasoningEncryptedValueSubtype,
        /// The tool call or message it belongs to.
        entity_id: String,
        /// The opaque value.
        encrypted_value: String,
        /// The subagent invocation this belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_run_id: Option<SubagentRunId>,
    }
    /// A subagent invocation begins. Here `subagentRunId` names the subagent itself.
    SubagentStarted(SubagentStartedEvent) => "SUBAGENT_STARTED" {
        /// The invocation being announced.
        subagent_run_id: SubagentRunId,
        /// The subagent's name, reusable across invocations.
        name: String,
        /// What it is for.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// The enclosing subagent invocation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_subagent_run_id: Option<SubagentRunId>,
        /// The tool call that spawned it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_tool_call_id: Option<ToolCallId>,
        /// The message that spawned it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_message_id: Option<MessageId>,
    }
    /// A subagent invocation's segment of the run ends.
    SubagentFinished(SubagentFinishedEvent) => "SUBAGENT_FINISHED" {
        /// The invocation being closed.
        subagent_run_id: SubagentRunId,
        /// Its return value. Any non-null JSON value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        /// Why it ended. Absent means success.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<SubagentFinishedOutcome>,
    }
    /// A subagent invocation failed without ending the run.
    SubagentError(SubagentErrorEvent) => "SUBAGENT_ERROR" {
        /// The invocation that failed.
        subagent_run_id: SubagentRunId,
        /// What went wrong, for a person.
        message: String,
        /// A machine-readable code (open string).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    }
}

/// What a [`ReasoningEncryptedValueEvent`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningEncryptedValueSubtype {
    /// A tool call.
    ToolCall,
    /// A message.
    Message,
}

impl Event {
    /// The subagent invocation this event is attributed to, if any.
    ///
    /// `SUBAGENT_STARTED`, `SUBAGENT_FINISHED` and `SUBAGENT_ERROR` name an invocation rather
    /// than belong to one, so they return `None`; so do the run-scoped events.
    pub fn attributed_to(&self) -> Option<&SubagentRunId> {
        match self {
            Event::TextMessageStart(e) => e.subagent_run_id.as_ref(),
            Event::TextMessageContent(e) => e.subagent_run_id.as_ref(),
            Event::TextMessageEnd(e) => e.subagent_run_id.as_ref(),
            Event::TextMessageChunk(e) => e.subagent_run_id.as_ref(),
            Event::ToolCallStart(e) => e.subagent_run_id.as_ref(),
            Event::ToolCallArgs(e) => e.subagent_run_id.as_ref(),
            Event::ToolCallEnd(e) => e.subagent_run_id.as_ref(),
            Event::ToolCallChunk(e) => e.subagent_run_id.as_ref(),
            Event::ToolCallResult(e) => e.subagent_run_id.as_ref(),
            Event::StateSnapshot(e) => e.subagent_run_id.as_ref(),
            Event::StateDelta(e) => e.subagent_run_id.as_ref(),
            Event::ActivitySnapshot(e) => e.subagent_run_id.as_ref(),
            Event::ActivityDelta(e) => e.subagent_run_id.as_ref(),
            Event::Raw(e) => e.subagent_run_id.as_ref(),
            Event::Custom(e) => e.subagent_run_id.as_ref(),
            Event::StepStarted(e) => e.subagent_run_id.as_ref(),
            Event::StepFinished(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningStart(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningMessageStart(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningMessageContent(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningMessageEnd(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningMessageChunk(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningEnd(e) => e.subagent_run_id.as_ref(),
            Event::ReasoningEncryptedValue(e) => e.subagent_run_id.as_ref(),
            Event::MessagesSnapshot(_)
            | Event::RunStarted(_)
            | Event::RunFinished(_)
            | Event::RunError(_)
            | Event::SubagentStarted(_)
            | Event::SubagentFinished(_)
            | Event::SubagentError(_) => None,
        }
    }

    /// Sets the envelope timestamp (milliseconds since the Unix epoch).
    #[must_use]
    pub fn with_timestamp_ms(mut self, timestamp: i64) -> Self {
        self.base_mut().timestamp = Some(timestamp);
        self
    }

    /// Sets the envelope metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.base_mut().metadata = Some(metadata);
        self
    }
}

// Constructors for the events the orchestrator emits. The others are built as struct literals.

impl RunStartedEvent {
    /// `RUN_STARTED` for `thread_id` / `run_id`, announcing protocol 1.0.
    pub fn new(thread_id: impl Into<ThreadId>, run_id: impl Into<RunId>) -> Self {
        Self {
            base: BaseFields::default(),
            thread_id: thread_id.into(),
            run_id: run_id.into(),
            protocol_version: Some(crate::PROTOCOL_VERSION.to_owned()),
            parent_run_id: None,
            input: None,
        }
    }
}

impl RunFinishedEvent {
    /// `RUN_FINISHED` with the given outcome.
    pub fn new(
        thread_id: impl Into<ThreadId>,
        run_id: impl Into<RunId>,
        outcome: RunFinishedOutcome,
    ) -> Self {
        Self {
            base: BaseFields::default(),
            thread_id: thread_id.into(),
            run_id: run_id.into(),
            result: None,
            outcome: Some(outcome),
            usage: None,
        }
    }
}

impl RunErrorEvent {
    /// `RUN_ERROR` with a message and an optional code.
    pub fn new(message: impl Into<String>, code: Option<String>) -> Self {
        Self {
            base: BaseFields::default(),
            message: message.into(),
            code,
            usage: None,
        }
    }
}

impl StateSnapshotEvent {
    /// `STATE_SNAPSHOT`, unattributed.
    pub fn new(snapshot: Value) -> Self {
        Self {
            base: BaseFields::default(),
            snapshot,
            subagent_run_id: None,
        }
    }
}

impl TextMessageStartEvent {
    /// `TEXT_MESSAGE_START` with a role, unattributed.
    pub fn new(message_id: impl Into<MessageId>, role: TextMessageRole) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            role: Some(role),
            name: None,
            subagent_run_id: None,
        }
    }
}

impl TextMessageContentEvent {
    /// `TEXT_MESSAGE_CONTENT`, unattributed.
    pub fn new(message_id: impl Into<MessageId>, delta: impl Into<String>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            delta: delta.into(),
            subagent_run_id: None,
        }
    }
}

impl TextMessageEndEvent {
    /// `TEXT_MESSAGE_END`, unattributed.
    pub fn new(message_id: impl Into<MessageId>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            subagent_run_id: None,
        }
    }
}

impl ReasoningStartEvent {
    /// `REASONING_START`, unattributed.
    pub fn new(message_id: impl Into<MessageId>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            subagent_run_id: None,
        }
    }
}

impl ReasoningMessageStartEvent {
    /// `REASONING_MESSAGE_START` (role `reasoning`), unattributed.
    pub fn new(message_id: impl Into<MessageId>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            role: ReasoningRole::Reasoning,
            subagent_run_id: None,
        }
    }
}

impl ReasoningMessageContentEvent {
    /// `REASONING_MESSAGE_CONTENT`, unattributed.
    pub fn new(message_id: impl Into<MessageId>, delta: impl Into<String>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            delta: delta.into(),
            subagent_run_id: None,
        }
    }
}

impl ReasoningMessageEndEvent {
    /// `REASONING_MESSAGE_END`, unattributed.
    pub fn new(message_id: impl Into<MessageId>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            subagent_run_id: None,
        }
    }
}

impl ReasoningEndEvent {
    /// `REASONING_END`, unattributed.
    pub fn new(message_id: impl Into<MessageId>) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            subagent_run_id: None,
        }
    }
}

impl ActivitySnapshotEvent {
    /// `ACTIVITY_SNAPSHOT`, unattributed, replacing any earlier content.
    pub fn new(
        message_id: impl Into<MessageId>,
        activity_type: impl Into<String>,
        content: Metadata,
    ) -> Self {
        Self {
            base: BaseFields::default(),
            message_id: message_id.into(),
            activity_type: activity_type.into(),
            content,
            replace: None,
            subagent_run_id: None,
        }
    }
}

impl SubagentStartedEvent {
    /// `SUBAGENT_STARTED` for invocation `subagent_run_id` of the subagent `name`.
    pub fn new(subagent_run_id: impl Into<SubagentRunId>, name: impl Into<String>) -> Self {
        Self {
            base: BaseFields::default(),
            subagent_run_id: subagent_run_id.into(),
            name: name.into(),
            description: None,
            parent_subagent_run_id: None,
            parent_tool_call_id: None,
            parent_message_id: None,
        }
    }
}

impl SubagentFinishedEvent {
    /// `SUBAGENT_FINISHED` with the given outcome (`None` means success).
    pub fn new(
        subagent_run_id: impl Into<SubagentRunId>,
        outcome: Option<SubagentFinishedOutcome>,
    ) -> Self {
        Self {
            base: BaseFields::default(),
            subagent_run_id: subagent_run_id.into(),
            result: None,
            outcome,
        }
    }
}

impl SubagentErrorEvent {
    /// `SUBAGENT_ERROR` with a message and an optional code.
    pub fn new(
        subagent_run_id: impl Into<SubagentRunId>,
        message: impl Into<String>,
        code: Option<String>,
    ) -> Self {
        Self {
            base: BaseFields::default(),
            subagent_run_id: subagent_run_id.into(),
            message: message.into(),
            code,
        }
    }
}
