//! In-memory implementations of the ports, for tests and for running the orchestrator
//! without any network dependency. Behind the `testkit` feature.

mod agent;
mod artifacts;
mod auth;
mod fixtures;
mod model;
mod pending;
mod prompts;
mod registry;
mod run_leases;
mod secrets;
mod store;
mod tools;
mod user_tools;
mod vault;
mod wakeup;

pub use agent::{
    AnswerFault, Call, STREAM_PIECES, ScriptedAgent, SendCall, VerdictScript, stream_id,
    stream_text,
};
pub use artifacts::{Download, MemoryArtifacts};
pub use auth::MemoryAuth;
pub use fixtures::{FixedClock, SeqIds, sample_releases};
pub use model::{ModelStep, ScriptedModel};
pub use pending::MemoryPendingAuthorizations;
pub use registry::{MEMORY_SOURCE, MemoryRegistry};
pub use run_leases::{MemoryModelCredentials, MemoryRunBindingStore, MemoryRunLeases};
pub use secrets::MemorySecretStore;
pub use store::MemoryStore;
pub use tools::{MemoryToolServer, MemoryToolServers, ToolScript};
pub use user_tools::MemoryUserToolServers;
pub use vault::MemoryVault;
pub use wakeup::MemoryWakeup;
