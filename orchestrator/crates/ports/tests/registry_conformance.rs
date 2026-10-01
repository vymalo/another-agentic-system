//! Runs the registry testkit against the in-memory registry, and against a composition of the
//! static list and the in-memory registry (the shape of the orchestrator's own).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{AgentId, AgentSource};
use orch_ports::memory::MemoryRegistry;
use orch_ports::testkit::registry::{RegistryFixture, entry};
use orch_ports::{AgentRegistry, CompositeRegistry, FixedRegistry, RegistryEntry};

struct Memory(MemoryRegistry);

impl RegistryFixture for Memory {
    type Registry = MemoryRegistry;

    fn registry(&self) -> &MemoryRegistry {
        &self.0
    }
    fn add(&self, entry: RegistryEntry) {
        self.0.add(entry);
    }
    fn remove(&self, id: &AgentId) {
        self.0.remove(id);
    }
    fn set_down(&self, down: bool) {
        self.0.set_down(down);
    }
    fn add_invalid(&self) {
        self.0.add(entry("Not A Valid Id", "Invalid"));
    }
}

mod memory {
    use super::*;

    async fn make() -> Option<Memory> {
        Some(Memory(MemoryRegistry::new()))
    }

    orch_ports::agent_registry_conformance!(make);
}

/// An empty static list in front of the registry: the cases see only the registry's agents, and
/// the static source is always there and always available.
struct Composite {
    registry: CompositeRegistry<FixedRegistry, MemoryRegistry>,
    memory: MemoryRegistry,
}

impl RegistryFixture for Composite {
    type Registry = CompositeRegistry<FixedRegistry, MemoryRegistry>;

    fn registry(&self) -> &Self::Registry {
        &self.registry
    }
    fn add(&self, entry: RegistryEntry) {
        self.memory.add(entry);
    }
    fn remove(&self, id: &AgentId) {
        self.memory.remove(id);
    }
    fn set_down(&self, down: bool) {
        self.memory.set_down(down);
    }
    fn add_invalid(&self) {
        self.memory.add(entry("Not A Valid Id", "Invalid"));
    }
}

mod composite {
    use super::*;

    async fn make() -> Option<Composite> {
        let memory = MemoryRegistry::new();
        Some(Composite {
            registry: CompositeRegistry::new(FixedRegistry::default(), memory.clone()),
            memory,
        })
    }

    orch_ports::agent_registry_conformance!(make);
}

/// With the static list in front, its agents stay listed and found while the registry is down,
/// and an id only the registry could list is "cannot tell", not "no such agent".
#[tokio::test]
async fn static_agents_survive_a_registry_that_is_down() {
    let memory = MemoryRegistry::new();
    memory.add(entry("platform-coder", "Platform coder"));
    let mut coder = entry("coder", "Coder");
    coder.origin = AgentSource::Static;
    let registry = CompositeRegistry::new(FixedRegistry::new([coder]), memory.clone());

    let listing = registry.list().await;
    let got: Vec<_> = listing.entries.iter().map(|e| e.id().as_str()).collect();
    assert_eq!(got, ["coder", "platform-coder"]);

    memory.set_down(true);
    let listing = registry.list().await;
    let got: Vec<_> = listing.entries.iter().map(|e| e.id().as_str()).collect();
    assert_eq!(
        got,
        ["coder"],
        "the static agents stay, the registry's are gone"
    );
    let names: Vec<_> = listing
        .sources
        .iter()
        .map(|s| (s.name.as_str(), s.available))
        .collect();
    assert_eq!(names, [("static", true), ("memory", false)]);

    // The static agent is found without asking the registry; any other id cannot be answered.
    assert!(
        registry
            .get(&AgentId::new("coder"))
            .await
            .unwrap()
            .is_some()
    );
    assert!(registry.get(&AgentId::new("platform-coder")).await.is_err());
    assert!(registry.get(&AgentId::new("nobody")).await.is_err());

    memory.set_down(false);
    assert!(
        registry
            .get(&AgentId::new("platform-coder"))
            .await
            .unwrap()
            .is_some()
    );
}
