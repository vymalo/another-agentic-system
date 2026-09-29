//! The orchestrator application: the thread service ([`App`]) and the durable outbox
//! [`Dispatcher`]. Everything here is written against the ports (ADR 0009), so any
//! composition of adapters can run it.

mod app;
mod directory;
mod dispatcher;
mod error;

pub use app::{AgentDescription, App, AppConfig, ApplyOutcome, Creation, Inbound, NewThread};
pub use directory::{AgentDirectory, AgentEntry};
pub use dispatcher::{Dispatcher, DispatcherConfig};
pub use error::AppError;
