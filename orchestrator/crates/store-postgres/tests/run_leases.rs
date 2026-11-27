//! The run-lease broker's conformance over `PgRunBindings` (ADR 0084, ADR 0106): the same cases as
//! the in-memory broker, against the real table of migrations `0030` and `0035`. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use orch_credential_broker::RunLeaseBroker;
use orch_ports::memory::MemoryModelCredentials;
use orch_ports::testkit::run_leases::{an_ended_run_is_not_found_after_the_bound, token};
use orch_store_postgres::PgRunBindings;
use support::TestDb;

async fn make() -> Option<RunLeaseBroker<PgRunBindings, MemoryModelCredentials>> {
    let db = TestDb::new().await?;
    // The store migrates the schema (migration 0030 included); the bindings then use their own pool
    // on the same pinned `search_path`.
    drop(db.store().await);
    let pool = support::pool_of(support::named(db.options(), "orch-run-leases-test"), 4);
    Some(RunLeaseBroker::new(
        PgRunBindings::from_pool(pool),
        MemoryModelCredentials::new(BTreeMap::from([("glm-5.3".to_owned(), token())])),
    ))
}

orch_ports::run_leases_conformance!(make);

#[tokio::test]
async fn a_run_that_ended_is_not_found_after_its_tombstone_bound() {
    let Some(broker) = make().await else {
        eprintln!("skipped: no run-lease broker available");
        return;
    };
    let broker = broker.with_tombstone_retention(Duration::ZERO);
    an_ended_run_is_not_found_after_the_bound(&broker).await;
}
