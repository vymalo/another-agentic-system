//! The run-lease broker's conformance over `PgRunBindings` (ADR 0084): the same ten cases as the
//! in-memory broker, against the real table of migration `0030`. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;

use orch_credential_broker::RunLeaseBroker;
use orch_ports::memory::MemoryModelCredentials;
use orch_ports::testkit::run_leases::token;
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
