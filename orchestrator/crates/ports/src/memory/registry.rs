use std::sync::{Arc, Mutex, MutexGuard};

use orch_core::{AgentId, is_valid_agent_id};

use crate::{AgentListing, AgentRegistry, RegistryEntry, RegistryError, SourceStatus};

/// The name of a [`MemoryRegistry`] in [`SourceStatus`].
pub const MEMORY_SOURCE: &str = "memory";

#[derive(Debug, Default)]
struct State {
    entries: Vec<RegistryEntry>,
    down: bool,
}

/// An agent registry held in memory that a test changes while it runs: agents come and go
/// ([`add`](MemoryRegistry::add), [`remove`](MemoryRegistry::remove)) and the whole source can
/// be taken down ([`set_down`](MemoryRegistry::set_down)). Clones share the same list, the way
/// two replicas read the same registry.
///
/// It is the reference implementation of the registry testkit and what the orchestrator's own
/// tests use to play a registry that changes or fails.
#[derive(Debug, Clone, Default)]
pub struct MemoryRegistry {
    state: Arc<Mutex<State>>,
}

impl MemoryRegistry {
    /// An empty, available registry.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves a list that is still a list.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Lists `entry` from the next read on, after the agents already listed. An entry with the
    /// id of one already listed replaces it in place.
    pub fn add(&self, entry: RegistryEntry) {
        let mut state = self.state();
        match state.entries.iter_mut().find(|e| e.id() == entry.id()) {
            Some(existing) => *existing = entry,
            None => state.entries.push(entry),
        }
    }

    /// Stops listing the agent `id`.
    pub fn remove(&self, id: &AgentId) {
        self.state().entries.retain(|e| e.id() != id);
    }

    /// Takes the source down (`true`): until it is put back up it lists nothing and cannot say
    /// whether an agent exists. `false` puts it back.
    pub fn set_down(&self, down: bool) {
        self.state().down = down;
    }
}

impl AgentRegistry for MemoryRegistry {
    async fn list(&self) -> AgentListing {
        let state = self.state();
        if state.down {
            return AgentListing {
                entries: Vec::new(),
                sources: vec![SourceStatus::unavailable(
                    MEMORY_SOURCE,
                    "the registry could not be reached",
                )],
            };
        }
        AgentListing {
            // An entry that is not an agent (its id is not one) is never listed.
            entries: state
                .entries
                .iter()
                .filter(|e| is_valid_agent_id(e.id().as_str()))
                .cloned()
                .collect(),
            sources: vec![SourceStatus::ok(MEMORY_SOURCE)],
        }
    }

    async fn get(&self, id: &AgentId) -> Result<Option<RegistryEntry>, RegistryError> {
        let state = self.state();
        if state.down {
            return Err(RegistryError::unavailable(
                MEMORY_SOURCE,
                "the registry could not be reached",
            ));
        }
        Ok(state
            .entries
            .iter()
            .find(|e| e.id() == id && is_valid_agent_id(id.as_str()))
            .cloned())
    }
}
