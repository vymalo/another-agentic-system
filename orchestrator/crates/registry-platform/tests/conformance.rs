//! The `AgentRegistry` conformance suite of `orch-ports`, against the platform registry over real
//! HTTP, served by an in-process registry that says `Cache-Control: no-store`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::AgentId;
use orch_ports::RegistryEntry;
use orch_ports::testkit::registry::RegistryFixture;
use orch_registry_platform::{PlatformConfig, PlatformRegistry};
use serde_json::json;
use support::*;

struct Fixture {
    server: Server,
    registry: PlatformRegistry,
}

impl RegistryFixture for Fixture {
    type Registry = PlatformRegistry;

    fn registry(&self) -> &PlatformRegistry {
        &self.registry
    }

    fn add(&self, entry: RegistryEntry) {
        self.server.push(item_of(&entry));
    }

    fn remove(&self, id: &AgentId) {
        self.server.script(|s| {
            s.items
                .retain(|item| item["service"][0].as_str() != Some(id.as_str()));
        });
    }

    fn set_down(&self, down: bool) {
        self.server.script(|s| {
            s.mode = if down { Mode::Status(503) } else { Mode::Serve };
        });
    }

    fn add_invalid(&self) {
        // Several ways to be no agent at once; the registry keeps the rest.
        self.server
            .push(json!({"href": "http://bad.test/card", "service": ["Not_Valid"]}));
        self.server.push(json!({"service": ["no-href"]}));
        self.server.push(json!("not even an object"));
    }
}

async fn make() -> Option<Fixture> {
    let server = Server::start().await;
    let registry =
        PlatformRegistry::new(PlatformConfig::new(server.url.clone()).without_system_proxy())
            .unwrap();
    Some(Fixture { server, registry })
}

orch_ports::agent_registry_conformance!(make);
