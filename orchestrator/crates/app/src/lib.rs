//! The orchestrator application: the thread service ([`App`]) and the durable outbox
//! [`Dispatcher`]. Everything here is written against the ports (ADR 0009), so any
//! composition of adapters can run it.

mod app;
mod directory;
mod dispatcher;
mod error;
mod gate_config;

pub use app::{AgentDescription, App, AppConfig, ApplyOutcome, Creation, Inbound, NewThread};
pub use directory::{AgentDirectory, AgentEntry};
pub use dispatcher::{Dispatcher, DispatcherConfig};
pub use error::AppError;
pub use gate_config::{
    CiLayer, DEFAULT_MAX_ATTEMPTS_CAP, GateError, GateLayer, GateRules, Layer,
    MAX_ATTEMPTS_CAP_CEILING, SourceName, THREAD_GATE_KEY, known_sources, pending_reason,
};
