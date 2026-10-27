//! Runs the conformance testkit against the in-memory implementations.
#![allow(missing_docs)]

use orch_ports::memory::{MemoryStore, MemoryWakeup};

async fn make_store() -> Option<MemoryStore> {
    Some(MemoryStore::new())
}

async fn make_wakeup() -> Option<MemoryWakeup> {
    Some(MemoryWakeup::new())
}

/// A log the ledger was never written for (ADR 0065): the ledger is emptied and no thread has been
/// gone through.
async fn forget_ledger(store: &MemoryStore) {
    store.forget_usage_ledger();
}

orch_ports::thread_store_conformance!(make_store);
orch_ports::usage_ledger_conformance!(make_store, forget = forget_ledger);
orch_ports::wakeup_conformance!(make_wakeup);
