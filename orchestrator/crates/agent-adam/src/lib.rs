//! Local agents for the orchestrator (ADR 0015): [`AgentClient`](orch_ports::AgentClient) over
//! adam-rs agents hosted in the orchestrator's own process.
//!
//! A remote agent stays a plain A2A agent behind `orch-agent-a2a`. This crate is the other
//! way to reach an agent: no card URL, no network hop. The orchestrator's binary links it only
//! behind its off-by-default Cargo feature `agent-local`.
//!
//! * [`LocalAgents`] owns one adam-rs runtime per process, with every configured
//!   [`LocalKind`] registered, a journal in the orchestrator's own Postgres (tables and
//!   `NOTIFY` channels prefixed [`TABLE_PREFIX`]) and a worker.
//! * [`LocalAgentClient`] is the [`AgentClient`](orch_ports::AgentClient) the dispatcher
//!   calls. It drives the runtime through the A2A `TaskBackend` seam and maps the results with
//!   `orch-a2a-mapping`, so its idempotency keys are the HTTP adapter's.
//!
//! Everything durable is in the store: a process that dies loses nothing, and another one
//! resumes each run from its last commit once the lease has expired. See the crate README and
//! `docs/orchestrator.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod agents;
mod client;
mod echo;
mod error;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use agents::{LocalAgents, LocalKind, LocalOptions};
pub use client::LocalAgentClient;
pub use error::LocalAgentsError;

/// The prefix of every table and every `NOTIFY` channel of the local agents' journal
/// (`orch_agent_runs`, `orch_agent_journal`, `orch_agent_meta`; `orch_agent_events`,
/// `orch_agent_signals`). It keeps them apart from the orchestrator's own (`threads`, `events`,
/// `a2a_bindings`, `outbox`; `orch_thread`, `orch_outbox`, `orch_resync`) in the shared database.
pub const TABLE_PREFIX: &str = "orch_agent_";
