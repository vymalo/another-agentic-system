//! The `MemoryNoteStore` testkit against `MemoryStore` (ADR 0091, invariant 6).
//!
//! A file of its own: the macro writes the names of its cases into the module, and some of them are
//! another suite's, so both in one module would not compile.
#![allow(missing_docs)]

use orch_ports::memory::MemoryStore;

async fn make_store() -> Option<MemoryStore> {
    Some(MemoryStore::new())
}

orch_ports::memory_note_store_conformance!(make_store);
