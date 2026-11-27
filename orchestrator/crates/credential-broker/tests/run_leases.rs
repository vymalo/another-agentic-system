//! The run-lease broker's conformance (ADR 0084, ADR 0107) over the in-memory binding store and
//! credentials.

use std::collections::BTreeMap;
use std::time::Duration;

use orch_credential_broker::RunLeaseBroker;
use orch_ports::memory::{MemoryModelCredentials, MemoryRunBindingStore};
use orch_ports::testkit::run_leases::token;

type Broker = RunLeaseBroker<MemoryRunBindingStore, MemoryModelCredentials>;

fn broker() -> Broker {
    RunLeaseBroker::new(
        MemoryRunBindingStore::default(),
        MemoryModelCredentials::new(BTreeMap::from([("glm-5.3".to_owned(), token())])),
    )
}

async fn make() -> Option<Broker> {
    Some(broker())
}

/// The same with no tombstone: the bound is zero.
async fn make_unkept() -> Option<Broker> {
    Some(broker().with_tombstone_retention(Duration::ZERO))
}

orch_ports::run_leases_conformance!(make, make_unkept);
