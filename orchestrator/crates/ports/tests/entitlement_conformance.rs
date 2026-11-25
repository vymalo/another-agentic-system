//! Runs the `EntitlementSource` testkit against the in-memory source.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::UserId;
use orch_ports::memory::MemoryEntitlement;
use orch_ports::testkit::entitlement::EntitlementFixture;

struct Memory(MemoryEntitlement);

impl EntitlementFixture for Memory {
    type Source = MemoryEntitlement;

    fn source(&self) -> &MemoryEntitlement {
        &self.0
    }

    fn set_active(&self, person: &UserId) {
        self.0.set_active(person);
    }

    fn set_lapsed(&self, person: &UserId, since: Timestamp) {
        self.0.set_lapsed(person, since);
    }

    fn forget(&self, person: &UserId) {
        self.0.forget(person);
    }

    fn set_unavailable(&self, unavailable: bool) {
        self.0.set_unavailable(unavailable);
    }

    fn asked(&self) -> Vec<UserId> {
        self.0.asked()
    }
}

async fn make() -> Option<Memory> {
    Some(Memory(MemoryEntitlement::new()))
}

orch_ports::entitlement_conformance!(make);
