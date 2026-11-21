//! The port conformance testkit against Postgres. Skipped unless `ORCH_TEST_DATABASE_URL`
//! is set; each case runs in its own schema.
#![allow(missing_docs)]

mod support;

use std::time::Duration;

use orch_store_postgres::sql::query;
use orch_store_postgres::{PgPendingAuthorizations, PgStore, PgUserToolServers, PgWakeup};
use support::TestDb;

async fn make_store() -> Option<PgStore> {
    Some(TestDb::new().await?.store().await)
}

async fn make_wakeup() -> Option<PgWakeup> {
    let db = TestDb::new().await?;
    let wakeup = db.wakeup("orch-test-wakeup", 4).await;
    assert!(
        wakeup.wait_listening(Duration::from_secs(10)).await,
        "listener did not attach"
    );
    Some(wakeup)
}

/// The definitions of people's servers: the table is created by the store's migrations, the adapter
/// shares the store's pool.
async fn make_user_tool_servers() -> Option<PgUserToolServers> {
    let store = TestDb::new().await?.store().await;
    Some(PgUserToolServers::from_pool(store.pool().clone()))
}

/// The authorizations that have begun: the table is created by the store's migrations, the adapter
/// shares the store's pool.
async fn make_pending_authorizations() -> Option<PgPendingAuthorizations> {
    let store = TestDb::new().await?.store().await;
    Some(PgPendingAuthorizations::from_pool(store.pool().clone()))
}

/// Closes the pool, so every later call fails as an unreachable database does.
async fn break_pending_authorizations(store: &PgPendingAuthorizations) {
    store.pool().close().await;
}

/// The log as a build without the ledger left it (ADR 0065): the rows are gone and no thread has
/// been gone through.
async fn forget_ledger(store: &PgStore) {
    query("DELETE FROM usage_tasks")
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("empty the ledger: {e}"));
    query("UPDATE threads SET usage_through = 0")
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("mark no thread as gone through: {e}"));
}

/// The log as a build without the search index left it (ADR 0068): the rows are gone, no title is
/// indexed and no thread has been gone through.
async fn forget_index(store: &PgStore) {
    query("DELETE FROM message_terms")
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("empty the index: {e}"));
    query("UPDATE threads SET search_through = 0, title_terms = NULL, description_terms = NULL")
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("mark no thread as gone through: {e}"));
}

orch_ports::thread_store_conformance!(make_store);
orch_ports::usage_ledger_conformance!(make_store, forget = forget_ledger);
/// An index made under an older fold version (ADR 0068): every thread's stored version is lower and the
/// keys it holds are ones no query makes, as keys of other rules would be.
async fn age_index(store: &PgStore) {
    query("UPDATE message_terms SET terms = array_to_tsvector(ARRAY['zzzold'])")
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("write keys of other rules: {e}"));
    query(
        "UPDATE threads SET search_fold = search_fold - 1, \
         title_terms = array_to_tsvector(ARRAY['zzzold']), \
         description_terms = array_to_tsvector(ARRAY['zzzold'])",
    )
    .execute(store.pool())
    .await
    .unwrap_or_else(|e| panic!("lower the fold version: {e}"));
}

orch_ports::thread_search_conformance!(make_store, forget = forget_index, age = age_index);
orch_ports::wakeup_conformance!(make_wakeup);
orch_ports::user_tool_server_store_conformance!(make_user_tool_servers);
orch_ports::pending_authorizations_conformance!(
    make_pending_authorizations,
    breaker = break_pending_authorizations
);
