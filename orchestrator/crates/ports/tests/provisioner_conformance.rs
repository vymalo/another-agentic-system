//! Runs the `AgentProvisioner` testkit against the in-memory provisioner.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use orch_ports::memory::MemoryProvisioner;
use orch_ports::testkit::provisioner::ProvisionerFixture;

struct Memory(MemoryProvisioner);

impl ProvisionerFixture for Memory {
    type Provisioner = MemoryProvisioner;
    fn provisioner(&self) -> &MemoryProvisioner {
        &self.0
    }
    fn agent_count(&self) -> usize {
        self.0.len()
    }
    fn set_ready_delay(&self, delay: Duration) {
        self.0.set_ready_delay(delay);
    }
}

async fn make() -> Option<Memory> {
    Some(Memory(MemoryProvisioner::new()))
}

orch_ports::agent_provisioner_conformance!(make);
