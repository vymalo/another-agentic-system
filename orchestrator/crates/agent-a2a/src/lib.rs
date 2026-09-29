//! The A2A adapter: an [`orch_ports::AgentClient`] over `a2a-client-lf`.
//!
//! Protocol notes (A2A 1.0, verified against the SDK sources 2026-09-29):
//! - methods used: `SendStreamingMessage`, `SubscribeToTask` (the port's `resubscribe`),
//!   `GetTask`, `CancelTask`, `ListTasks`;
//! - `SubscribeToTask` only works while the task executes in the answering process; a finished
//!   task or one running elsewhere yields `TASK_NOT_FOUND`, which the dispatcher answers by
//!   polling `GetTask`;
//! - the SDK loses the HTTP status of failed calls; errors are classified by JSON-RPC code and
//!   by the SDK's message prefixes (see the `errors` module).
//!
//! Release channels (ADR 0008) are an optional extension: [`releases_from_card`] reads them
//! from the live card, and a selected release is sent as the `A2A-Extensions` header plus
//! namespaced message metadata. Nothing here depends on a specific agent host.

mod client;
mod errors;
mod mapping;
mod releases;

pub use client::{A2aAgentClient, A2aConfig, BuildError, install_crypto_provider};
pub use releases::{RELEASE_CHANNELS_URI, releases_from_card};
