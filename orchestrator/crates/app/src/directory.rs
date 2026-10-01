use orch_core::{AgentId, AgentSource};
use orch_ports::{AgentEndpoint, FixedRegistry, RegistryEntry};

/// One configured agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    /// Where the agent lives.
    pub endpoint: AgentEndpoint,
    /// Display name.
    pub name: String,
}

impl From<&AgentEntry> for RegistryEntry {
    /// A configured agent as the registry lists it: from the static source, with no tags.
    fn from(entry: &AgentEntry) -> Self {
        RegistryEntry {
            endpoint: entry.endpoint.clone(),
            name: entry.name.clone(),
            tags: Vec::new(),
            origin: AgentSource::Static,
        }
    }
}

/// The static set of agents the deployment configures (`AGENTS_FILE`).
///
/// It is what the startup checks run on: a gate layer belongs to a configured agent, and the
/// verifier is another configured agent ([`GateRules::validate`](crate::GateRules::validate)).
/// Which agents a person can *target* is the [`AgentRegistry`](orch_ports::AgentRegistry) of the
/// ports (ADR 0022), which lists these and, when the deployment reads one, the platform's: a
/// composition root hands the same entries to both, see [`AgentDirectory::fixed_registry`].
#[derive(Debug, Clone, Default)]
pub struct AgentDirectory {
    entries: Vec<AgentEntry>,
}

impl AgentDirectory {
    /// Builds the directory; order is preserved for listing.
    pub fn new(entries: Vec<AgentEntry>) -> Self {
        AgentDirectory { entries }
    }

    /// The agent with this id.
    pub fn get(&self, id: &AgentId) -> Option<&AgentEntry> {
        self.entries.iter().find(|e| &e.endpoint.id == id)
    }

    /// All agents, in configuration order.
    pub fn iter(&self) -> impl Iterator<Item = &AgentEntry> {
        self.entries.iter()
    }

    /// The agents as registry entries (origin `static`, no tags), in configuration order.
    pub fn registry_entries(&self) -> Vec<RegistryEntry> {
        self.entries.iter().map(RegistryEntry::from).collect()
    }

    /// The agents as the static registry a composition root puts in its [`Ports`](orch_ports::Ports),
    /// alone or in front of a platform registry.
    pub fn fixed_registry(&self) -> FixedRegistry {
        FixedRegistry::new(self.registry_entries())
    }
}
