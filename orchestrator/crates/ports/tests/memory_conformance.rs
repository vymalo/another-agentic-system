//! Runs the conformance testkit against the in-memory implementations.
#![allow(missing_docs)]

use orch_ports::memory::{MemoryStore, MemoryWakeup};

async fn make_store() -> Option<MemoryStore> {
    Some(MemoryStore::new())
}

async fn make_wakeup() -> Option<MemoryWakeup> {
    Some(MemoryWakeup::new())
}

orch_ports::thread_store_conformance!(make_store);
orch_ports::wakeup_conformance!(make_wakeup);
