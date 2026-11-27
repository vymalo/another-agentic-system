//! The run-lease conformance (ADR 0084, ADR 0108) over `MemoryRunLeases`, the in-memory
//! implementation of `RunLeases`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::time::Duration;

use orch_ports::memory::MemoryRunLeases;
use orch_ports::testkit::run_leases::token;

fn leases() -> MemoryRunLeases {
    MemoryRunLeases::new(BTreeMap::from([("glm-5.3".to_owned(), token())]))
}

async fn make() -> Option<MemoryRunLeases> {
    Some(leases())
}

/// The same with no tombstone: the bound is zero.
async fn make_unkept() -> Option<MemoryRunLeases> {
    Some(leases().with_tombstone_retention(Duration::ZERO))
}

orch_ports::run_leases_conformance!(make, make_unkept);
