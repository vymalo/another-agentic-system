//! The run-lease conformance (ADR 0084, ADR 0107) over `MemoryRunLeases`, the in-memory
//! implementation of `RunLeases`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::time::Duration;

use orch_ports::memory::MemoryRunLeases;
use orch_ports::testkit::run_leases::{an_ended_run_is_not_found_after_the_bound, token};

async fn make() -> Option<MemoryRunLeases> {
    Some(MemoryRunLeases::new(BTreeMap::from([(
        "glm-5.3".to_owned(),
        token(),
    )])))
}

orch_ports::run_leases_conformance!(make);

#[tokio::test]
async fn a_run_that_ended_is_not_found_after_its_tombstone_bound() {
    let broker = MemoryRunLeases::new(BTreeMap::from([("glm-5.3".to_owned(), token())]))
        .with_tombstone_retention(Duration::ZERO);
    an_ended_run_is_not_found_after_the_bound(&broker).await;
}
