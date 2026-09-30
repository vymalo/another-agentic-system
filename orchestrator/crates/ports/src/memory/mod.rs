//! In-memory implementations of the ports, for tests and for running the orchestrator
//! without any network dependency. Behind the `testkit` feature.

mod agent;
mod fixtures;
mod store;
mod wakeup;

pub use agent::{Call, ScriptedAgent, VerdictScript};
pub use fixtures::{FixedClock, SeqIds, sample_releases};
pub use store::MemoryStore;
pub use wakeup::MemoryWakeup;
