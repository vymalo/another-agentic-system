//! In-memory implementations of the ports, for tests and for running the orchestrator
//! without any network dependency. Behind the `testkit` feature.

mod agent;
mod artifacts;
mod auth;
mod fixtures;
mod model;
mod registry;
mod store;
mod wakeup;

pub use agent::{Call, STREAM_PIECES, ScriptedAgent, VerdictScript, stream_id, stream_text};
pub use artifacts::MemoryArtifacts;
pub use auth::MemoryAuth;
pub use fixtures::{FixedClock, SeqIds, sample_releases};
pub use model::{ModelStep, ScriptedModel};
pub use registry::{MEMORY_SOURCE, MemoryRegistry};
pub use store::MemoryStore;
pub use wakeup::MemoryWakeup;
