//! The port conformance testkit against Postgres. Skipped unless `ORCH_TEST_DATABASE_URL`
//! is set; each case runs in its own schema.
#![allow(missing_docs)]

mod support;

use std::time::Duration;

use orch_store_postgres::{PgStore, PgWakeup};
use support::TestDb;

async fn make_store() -> Option<PgStore> {
    Some(TestDb::new().await?.store().await)
}

async fn make_wakeup() -> Option<PgWakeup> {
    let db = TestDb::new().await?;
    let wakeup = PgWakeup::start(db.pool("orch-test-wakeup", 4).await);
    assert!(
        wakeup.wait_listening(Duration::from_secs(10)).await,
        "listener did not attach"
    );
    Some(wakeup)
}

orch_ports::thread_store_conformance!(make_store);
orch_ports::wakeup_conformance!(make_wakeup);
