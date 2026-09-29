//! AG-UI 1.0 wire types.
//!
//! The protocol's [`Event`] is a closed serde enum over all 31 event types, and
//! [`RunAgentInput`] is the one consumer-to-producer message. The types are checked against
//! the official 1.0 JSON Schema, vendored in `schema/` (see the crate README for its source
//! and hash), by the `testkit` feature.
//!
//! Rules kept by every type here:
//!
//! - **Absent means absent.** Optional members are skipped when `None`; `null` is never
//!   written for a missing value.
//! - **Time is an `i64` of milliseconds**, never a float.
//! - **Outbound is strict, inbound is lenient.** Serialising emits only declared members.
//!   [`RunAgentInput::parse`] drops undeclared members and reports them, and rejects
//!   malformed input.
//! - **No dependency on any orchestrator crate, agent host or AG-UI SDK.**

mod event;
mod ids;
mod input;
mod message;
mod patch;
mod run;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use event::*;
pub use ids::{InterruptId, MessageId, RunId, SubagentRunId, ThreadId, ToolCallId};
pub use input::{InputError, ParsedInput, RunAgentInput};
pub use message::{
    ActivityMessage, AssistantMessage, ContentPart, Context, DeveloperMessage, FunctionCall,
    Message, MessageContent, Metadata, PartSource, ReasoningMessage, ReasoningRole, SystemMessage,
    TextMessageRole, Tool, ToolCall, ToolCallKind, ToolMessage, ToolRole, UserMessage,
};
pub use patch::{InvalidJsonPointer, JsonPatch, JsonPatchOperation, JsonPointer};
pub use run::{
    Interrupt, ResumeEntry, ResumeStatus, RunFinishedOutcome, SubagentFinishedOutcome, TokenUsage,
};

/// The protocol version these types implement, as announced in `RUN_STARTED.protocolVersion`.
pub const PROTOCOL_VERSION: &str = "1.0";

/// The vendored official 1.0 JSON Schema (JSON Schema 2020-12,
/// `$id` `https://ag-ui.com/spec/1.0/schema.json`).
pub const SCHEMA_1_0: &str = include_str!("../schema/ag-ui-1.0.schema.json");
