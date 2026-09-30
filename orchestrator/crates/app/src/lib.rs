//! The orchestrator application: the thread service ([`App`]), the durable outbox
//! [`Dispatcher`] and the [`InboxWorker`] that applies timers and reports. Everything here is
//! written against the ports (ADR 0009), so any composition of adapters can run it.

mod app;
mod directory;
mod dispatcher;
mod error;
mod gate_config;
mod inbox;

pub use app::{
    AgentDescription, App, AppConfig, ApplyOutcome, Creation, Inbound, NewThread, Received,
};
pub use directory::{AgentDirectory, AgentEntry};
pub use dispatcher::{Dispatcher, DispatcherConfig};
pub use error::AppError;
pub use gate_config::{
    CiLayer, DEFAULT_MAX_ATTEMPTS_CAP, GateError, GateLayer, GateRules, Layer,
    MAX_ATTEMPTS_CAP_CEILING, SourceName, THREAD_GATE_KEY, known_sources, pending_reason,
};
pub use inbox::{
    DEFAULT_LEASE_SECS, DEFAULT_MAX_ATTEMPTS, DEFAULT_PARKED_TTL_SECS, DEFAULT_POLL_SECS,
    InboxConfig, InboxWorker,
};
