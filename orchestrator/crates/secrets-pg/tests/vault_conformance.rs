//! The `VaultStore` conformance testkit against Postgres, one schema for each case. Skipped
//! unless `ORCH_TEST_DATABASE_URL` is set (as `orch-store-postgres`' tests are).
//!
//! The clock is the case's: every case gets its own [`FixedClock`], which the vault's grace runs on,
//! so the 24 hours are moved and never slept through.
#![allow(missing_docs)]

mod support;

use std::sync::Arc;

use jiff::Timestamp;
use orch_ports::memory::FixedClock;
use orch_secrets_pg::PgVault;
use support::TestDb;

/// The instant every case's clock starts at: a whole second, which Postgres keeps exactly.
const START: i64 = 1_700_000_000;

async fn make_store() -> Option<(PgVault, FixedClock)> {
    let db = TestDb::new().await?;
    let store = db.store(support::ring_k1()).await;
    let clock = FixedClock::new(Timestamp::from_second(START).ok()?);
    Some((PgVault::new(store, Arc::new(clock.clone())), clock))
}

orch_ports::vault_store_conformance!(make_store);
