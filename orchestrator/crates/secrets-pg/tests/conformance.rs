//! The `SecretStore` conformance testkit against Postgres, one schema for each case. Skipped
//! unless `ORCH_TEST_DATABASE_URL` is set (as `orch-store-postgres`' tests are).
#![allow(missing_docs)]

mod support;

use orch_secrets_pg::PgSecretStore;
use support::TestDb;

async fn make_store() -> Option<PgSecretStore> {
    Some(TestDb::new().await?.store(support::ring_k1()).await)
}

/// What the Postgres adapter supplies to break itself: its pool is closed, so every call fails as
/// a database that cannot be reached does.
async fn break_the_store(store: &PgSecretStore) {
    store.close().await;
}

orch_ports::secret_store_conformance!(make_store, breaker = break_the_store);
