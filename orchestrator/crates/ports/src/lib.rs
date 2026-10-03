//! Ports of the orchestrator (ADR 0009): one trait per infrastructure boundary.
//!
//! - [`ThreadStore`]: threads, per-thread events with a strictly increasing `seq`, an outbox;
//! - the inbox (unsolicited webhook reports and timers) is part of [`ThreadStore`]: a thread commit
//!   must be atomic with the inbox row it applies;
//! - [`Wakeup`]: notify/listen hints, and the live text of a reply being written (never stored,
//!   ADR 0027);
//! - [`AgentClient`]: talking to a delegated agent (send, stream, resubscribe, poll, cancel, card),
//!   and [`ByTransport`], which serves one endpoint set from two clients (remote A2A, in-process);
//! - [`Authenticator`]: who is calling, from the credentials a request carried (a bearer token or
//!   the identity header of a proxy), failing closed (ADR 0033);
//! - [`ChatModel`]: one question to a language model and its answer (the orchestrator's titles);
//! - [`AgentRegistry`]: which agents exist right now, read live and failing closed (ADR 0022), with
//!   [`FixedRegistry`] (the static list) and [`CompositeRegistry`] (two registries as one);
//! - [`ArtifactStore`]: where the files agents hand over are kept, by the hash of their content;
//!   the log keeps only the reference (ADR 0032); [`NoArtifacts`] is a deployment without one;
//! - [`ToolServerClient`]: the orchestrator as a client of an MCP server, to list its tools and call
//!   one (ADR 0024); what a server answers is untrusted text, and its credentials are
//!   [`ToolSecret`]s that never print;
//! - [`Clock`], [`IdGen`]: time and identifiers.
//!
//! No implementation type appears in any signature. Implementations live in separate crates;
//! this crate ships in-memory ones behind the `testkit` feature together with a conformance
//! testkit every `ThreadStore` / `Wakeup` / `AgentClient` / `AgentRegistry` / `ChatModel` / `ArtifactStore` /
//! `ToolServerClient` implementation must pass.

mod agent;
mod artifacts;
mod auth;
mod bundle;
mod clock;
mod inbox;
mod model;
mod registry;
mod route;
mod store;
mod tools;
mod wakeup;

#[cfg(feature = "testkit")]
pub mod memory;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use agent::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream,
    AgentTransport, IdemKey, MentionInfo, SendContent, SendRequest, TaskHandle, TaskSnapshot,
    UiSupport,
};
pub use artifacts::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, CHUNK_BYTES,
    MAX_NAME_BYTES, NoArtifacts, stream_of,
};
pub use auth::{
    AuthError, Authenticator, ByCredential, CredentialKind, Credentials, Principal, RefuseAll, Role,
};
pub use bundle::{PortSet, Ports};
pub use clock::{Clock, IdGen, SystemClock, UuidV7Ids};
pub use inbox::{
    InboxFinal, InboxId, InboxItem, InboxLease, InboxPayload, InboxStatus, NewInbox, NewTimer,
    Parking, Received, TIMER_SOURCE, UndecodablePayload,
};
pub use model::{ChatModel, ChatRequest, ModelError, NoModel};
pub use registry::{
    AgentListing, AgentRegistry, CompositeRegistry, FixedRegistry, RegistryEntry, RegistryError,
    STATIC_SOURCE, SourceStatus,
};
pub use route::ByTransport;
pub use store::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, ForkOrigin, Lease, NewEvent, NewOutbox,
    NewThreadRecord, OutboxFinal, OutboxId, OutboxItem, OutboxKind, OutboxPayload, OutboxStats,
    OutboxStatus, StoreError, ThreadStore,
};
pub use tools::{
    MAX_REMOTE_MESSAGE_BYTES, MAX_RESULT_BYTES, ToolCall, ToolCallOutput, ToolDef, ToolSecret,
    ToolServerClient, ToolServerEndpoint, ToolServerError,
};
pub use wakeup::{Topic, Wakeup, WakeupCapabilities, WakeupError};
