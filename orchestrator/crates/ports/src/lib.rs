//! Ports of the orchestrator (ADR 0009): one trait per infrastructure boundary.
//!
//! - [`ThreadStore`]: threads, per-thread events with a strictly increasing `seq`, an outbox;
//! - [`Wakeup`]: notify/listen hints;
//! - [`AgentClient`]: talking to a delegated agent (send, stream, resubscribe, poll, cancel, card);
//! - [`Clock`], [`IdGen`]: time and identifiers.
//!
//! No implementation type appears in any signature. Implementations live in separate crates;
//! this crate ships in-memory ones behind the `testkit` feature together with a conformance
//! testkit every `ThreadStore` / `Wakeup` implementation must pass.

mod agent;
mod bundle;
mod clock;
mod store;
mod wakeup;

#[cfg(feature = "testkit")]
pub mod memory;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use agent::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, IdemKey,
    SendRequest, TaskHandle, TaskSnapshot,
};
pub use bundle::{PortSet, Ports};
pub use clock::{Clock, IdGen, SystemClock, UuidV7Ids};
pub use store::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, NewEvent, NewOutbox, NewThreadRecord,
    OutboxFinal, OutboxId, OutboxItem, OutboxKind, OutboxPayload, OutboxStatus, StoreError,
    ThreadStore,
};
pub use wakeup::{Topic, Wakeup, WakeupCapabilities, WakeupError};
