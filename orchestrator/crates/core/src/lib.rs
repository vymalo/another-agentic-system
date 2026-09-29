//! Pure core of the orchestrator.
//!
//! This crate holds the types of the chat API contract (`docs/api/chat-api.yaml`)
//! and the one function that decides what happens next: [`transition`]. It has no
//! async runtime, no I/O and no protocol dependencies, so the compiler enforces
//! that it stays pure (ADR 0001, ADR 0004).

mod agent;
mod event;
mod ids;
mod thread;
mod transition;

pub use agent::{AgentTaskState, AgentUpdate};
pub use event::{
    Actor, ActorType, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData,
    Event, EventBody, EventKind, ThreadStateData, UserMessageData,
};
pub use ids::{AgentId, ThreadId, UserId};
pub use thread::{AgentInfo, AgentTarget, Releases, ThreadRecord, ThreadState};
pub use transition::{Command, EventDraft, Input, TransitionError, transition};

/// Timestamps are `jiff` instants everywhere (no `f64` time).
pub use jiff::Timestamp;
