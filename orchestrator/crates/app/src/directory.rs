use orch_core::AgentId;
use orch_ports::AgentEndpoint;

/// One configured agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    /// Where the agent lives.
    pub endpoint: AgentEndpoint,
    /// Display name.
    pub name: String,
}

/// The static set of agents users can target (MVP: from configuration).
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
}
