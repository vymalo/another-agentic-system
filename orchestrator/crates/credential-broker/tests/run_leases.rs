//! The run-lease broker's conformance (ADR 0084, ADR 0106) over the in-memory binding store and
//! credentials.

use std::collections::BTreeMap;
use std::time::Duration;

use orch_credential_broker::RunLeaseBroker;
use orch_ports::memory::{MemoryModelCredentials, MemoryRunBindingStore};
use orch_ports::testkit::run_leases::{an_ended_run_is_not_found_after_the_bound, token};

fn credentials() -> MemoryModelCredentials {
    MemoryModelCredentials::new(BTreeMap::from([("glm-5.3".to_owned(), token())]))
}

async fn make() -> Option<RunLeaseBroker<MemoryRunBindingStore, MemoryModelCredentials>> {
    Some(RunLeaseBroker::new(
        MemoryRunBindingStore::default(),
        credentials(),
    ))
}

orch_ports::run_leases_conformance!(make);

#[tokio::test]
async fn a_run_that_ended_is_not_found_after_its_tombstone_bound() {
    let broker = RunLeaseBroker::new(MemoryRunBindingStore::default(), credentials())
        .with_tombstone_retention(Duration::ZERO);
    an_ended_run_is_not_found_after_the_bound(&broker).await;
}
