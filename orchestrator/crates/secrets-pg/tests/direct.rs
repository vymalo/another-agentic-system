//! The migrations of the sealed secret store on a connection of their own (issue 290), for a pool that
//! goes through a transaction-mode pooler. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_secrets_pg::{MIGRATIONS_TABLE, PgSecretStore};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use support::{TestDb, ring_k1};

/// A pool whose server is not there: any use of it fails.
fn unreachable_pool() -> PgPool {
    let options = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(1)
        .database("none");
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(500))
        .connect_lazy_with(options)
}

#[tokio::test]
async fn the_migrations_run_on_the_direct_connection_and_touch_no_pool() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = PgSecretStore::from_pool(unreachable_pool(), ring_k1());
    assert!(store.migrate().await.is_err(), "the pool is unreachable");

    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .expect("migrate on the direct connection");
    // a second run finds nothing to do
    PgSecretStore::migrate_direct(&db.pinned_direct_url())
        .await
        .expect("again");

    let raw = db.pool("orch-secrets-raw", 2).await;
    let applied: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {MIGRATIONS_TABLE} WHERE success"
    )))
    .fetch_one(&raw)
    .await
    .unwrap();
    assert!(applied > 0);
    // the session lock of the migrator went with its connection
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND database = \
         (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .fetch_one(&raw)
    .await
    .unwrap();
    assert_eq!(held, 0, "no advisory lock is left held");
}

#[tokio::test]
async fn a_connection_string_that_is_not_one_is_refused_without_its_text() {
    let secret = "hunter2-the-password";
    let error = PgSecretStore::migrate_direct(&format!("postgres://u:{secret}@h:notaport/db"))
        .await
        .unwrap_err();
    assert!(!format!("{error} {error:?}").contains(secret), "{error:?}");
}
