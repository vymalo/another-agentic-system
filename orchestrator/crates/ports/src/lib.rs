//! Ports of the orchestrator (ADR 0009): one trait per infrastructure boundary.
//!
//! - [`ThreadStore`]: threads, per-thread events with a strictly increasing `seq`, an outbox;
//! - the inbox (unsolicited webhook reports and timers) is part of [`ThreadStore`]: a thread commit
//!   must be atomic with the inbox row it applies;
//! - [`Wakeup`]: notify/listen hints, and the live text of a reply being written (never stored,
//!   ADR 0027);
//! - [`AgentClient`]: talking to a delegated agent (send, stream, resubscribe, poll, cancel, card),
//!   and [`ByTransport`], which serves one endpoint set from two clients (remote A2A, in-process);
//! - [`ChatModel`]: one question to a language model and its answer (the orchestrator's titles);
//! - [`AgentRegistry`]: which agents exist right now, read live and failing closed (ADR 0022), with
//!   [`FixedRegistry`] (the static list) and [`CompositeRegistry`] (two registries as one);
//! - [`Clock`], [`IdGen`]: time and identifiers.
//!
//! No implementation type appears in any signature. Implementations live in separate crates;
//! this crate ships in-memory ones behind the `testkit` feature together with a conformance
//! testkit every `ThreadStore` / `Wakeup` / `AgentClient` / `AgentRegistry` implementation must pass.

mod agent;
mod bundle;
mod clock;
mod inbox;
mod model;
mod registry;
mod route;
mod store;
mod wakeup;

#[cfg(feature = "testkit")]
pub mod memory;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use agent::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream,
    AgentTransport, IdemKey, SendContent, SendRequest, TaskHandle, TaskSnapshot, UiSupport,
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
pub use wakeup::{Topic, Wakeup, WakeupCapabilities, WakeupError};
