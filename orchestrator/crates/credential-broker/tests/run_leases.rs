//! The run-lease broker's conformance (ADR 0084) over the in-memory binding store and credentials.

use std::collections::BTreeMap;

use orch_credential_broker::RunLeaseBroker;
use orch_ports::memory::{MemoryModelCredentials, MemoryRunBindingStore};
use orch_ports::testkit::run_leases::token;

async fn make() -> Option<RunLeaseBroker<MemoryRunBindingStore, MemoryModelCredentials>> {
    Some(RunLeaseBroker::new(
        MemoryRunBindingStore::default(),
        MemoryModelCredentials::new(BTreeMap::from([("glm-5.3".to_owned(), token())])),
    ))
}

orch_ports::run_leases_conformance!(make);
