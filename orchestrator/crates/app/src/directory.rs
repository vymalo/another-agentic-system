use std::collections::BTreeMap;

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
    /// Other names a configured agent answers to: alias to canonical id (`aliases` of an entry of
    /// `AGENTS_FILE`, ADR 0049). The configuration has refused an alias that is another id or
    /// alias.
    aliases: BTreeMap<AgentId, AgentId>,
}

impl AgentDirectory {
    /// Builds the directory; order is preserved for listing.
    pub fn new(entries: Vec<AgentEntry>) -> Self {
        AgentDirectory {
            entries,
            aliases: BTreeMap::new(),
        }
    }

    /// The same directory, where each `(alias, canonical)` pair makes `alias` another name of the
    /// configured agent `canonical`. A pair whose `canonical` is not configured is ignored: the
    /// configuration has refused it before this is called.
    #[must_use]
    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = (AgentId, AgentId)>) -> Self {
        for (alias, canonical) in aliases {
            if self.entries.iter().any(|e| e.endpoint.id == canonical) {
                self.aliases.insert(alias, canonical);
            }
        }
        self
    }

    /// The agent with this id, or this alias.
    pub fn get(&self, id: &AgentId) -> Option<&AgentEntry> {
        let id = self.aliases.get(id).unwrap_or(id);
        self.entries.iter().find(|e| &e.endpoint.id == id)
    }

    /// The id an agent is listed under: `id` itself, or the agent `id` is an alias of. An id that
    /// names no configured agent (a platform agent's, an unknown one) is returned as it is.
    pub fn canonical(&self, id: &AgentId) -> AgentId {
        self.aliases.get(id).unwrap_or(id).clone()
    }

    /// The other names of the configured agent `id`, in order; none for an agent that has none
    /// and for one that is not configured.
    pub fn aliases_of(&self, id: &AgentId) -> Vec<AgentId> {
        self.aliases
            .iter()
            .filter(|(_, canonical)| *canonical == id)
            .map(|(alias, _)| alias.clone())
            .collect()
    }

    /// Every alias with the agent it names.
    pub fn aliases(&self) -> &BTreeMap<AgentId, AgentId> {
        &self.aliases
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
