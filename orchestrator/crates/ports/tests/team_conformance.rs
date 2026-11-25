//! The `TeamPresetStore` testkit against `MemoryStore` (ADR 0074, invariant 6).
//!
//! A file of its own: the macro writes the names of its cases into the module, and some of them are
//! a prompt suite's too (`prompt_store_conformance!` in `tests/memory_conformance.rs`), so both in
//! one module would not compile.
#![allow(missing_docs)]

use orch_ports::memory::MemoryStore;

async fn make_store() -> Option<MemoryStore> {
    Some(MemoryStore::new())
}

orch_ports::team_preset_store_conformance!(make_store);
