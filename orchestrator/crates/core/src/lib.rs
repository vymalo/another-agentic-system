//! Pure core of the orchestrator.
//!
//! This crate holds the types of the chat API contract (`docs/api/chat-api.yaml`)
//! and the one function that decides what happens next: [`transition`]. It has no
//! async runtime, no I/O and no protocol dependencies, so the compiler enforces
//! that it stays pure (ADR 0001, ADR 0004).

mod agent;
mod error;
mod event;
mod ids;
mod thread;
mod transition;
mod ui;

pub use agent::{AgentTaskState, AgentUpdate};
pub use error::{BoxError, Classify, ErrorClass, report};
pub use event::{
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData,
    Event, EventBody, EventKind, ThreadStateData, UserMessageData,
};
pub use ids::{AgentId, ThreadId, UserId};
pub use thread::{AgentInfo, AgentTarget, Releases, ThreadRecord, ThreadState};
pub use transition::{Command, EventDraft, Input, TransitionError, transition};
pub use ui::{
    A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, A2UI_MEDIA_TYPE, MAX_ACTION_CONTEXT_BYTES,
    MAX_ID_BYTES, MAX_OPERATIONS, MAX_OPERATIONS_BYTES, MAX_SURFACE_BYTES, OperationError,
    OperationInfo, SurfaceOp, UiActionData, UiActionError, UiRejection, UiSurfaceData, UiVersion,
    check_operation_list, check_operations, inspect, serialized_len,
};

/// Timestamps are `jiff` instants everywhere (no `f64` time).
pub use jiff::Timestamp;
