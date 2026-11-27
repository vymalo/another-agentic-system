//! The fresh-database test of ADR 0107 decision 6: the secret store's migrations, applied to an
//! empty schema, create exactly the functions and views [`ROUTINES`] lists (none before slice P12),
//! and each answers a call. A slice that adds, replaces or drops one (`orch_secret_…`,
//! `orch_vault_…`) moves this list in the same pull request. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use support::{Pool, TestDb, execute, fetch_all, ring_k1};

/// Every function and view of the secret store's schema, as `name(identity arguments)` (a view as
/// `view name`), each with the statement that calls it once on the migrated, empty schema.
const ROUTINES: &[(&str, &str)] = &[];

/// The functions (of any kind) and the views of `schema`, sorted.
async fn routines(pool: &Pool, schema: &str) -> Vec<String> {
    fetch_all::<(String,)>(
        pool,
        "SELECT p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')' \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = $1 \
         UNION ALL SELECT 'view ' || viewname::text FROM pg_views WHERE schemaname = $1 \
         ORDER BY 1",
        &[&schema],
    )
    .await
    .into_iter()
    .map(|(r,)| r)
    .collect()
}

#[tokio::test]
async fn every_function_is_created_and_answers() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    drop(db.store(ring_k1()).await);
    let pool = db.pool("orch-secrets-functions", 1).await;

    let mut listed: Vec<&str> = ROUTINES.iter().map(|(name, _)| *name).collect();
    listed.sort_unstable();
    assert_eq!(
        routines(&pool, &db.schema).await,
        listed,
        "the migrations' functions and views differ from ROUTINES: a slice that adds, changes or \
         drops one lists it here (ADR 0107 decision 6)"
    );
    for (name, call) in ROUTINES {
        assert!(
            name.starts_with("orch_secret_")
                || name.starts_with("orch_vault_")
                || name.starts_with("view orch_secret_")
                || name.starts_with("view orch_vault_"),
            "{name}: the secret store's functions are orch_secret_… and orch_vault_…"
        );
        execute(&pool, call, &[]).await;
    }
}

/// The listing sees what a migration would leave: a stray function and view created after the
/// migrations are found, so the test above cannot pass on a query that sees nothing.
#[tokio::test]
async fn a_stray_function_or_view_is_found() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    drop(db.store(ring_k1()).await);
    let pool = db.pool("orch-secrets-functions", 1).await;
    execute(
        &pool,
        "CREATE FUNCTION orch_secret_stray(a bigint) RETURNS bigint LANGUAGE sql IMMUTABLE RETURN a",
        &[],
    )
    .await;
    execute(
        &pool,
        "CREATE VIEW orch_vault_stray AS SELECT 1 AS one",
        &[],
    )
    .await;
    let found = routines(&pool, &db.schema).await;
    assert_eq!(
        found,
        ["orch_secret_stray(a bigint)", "view orch_vault_stray"],
        "{found:?}"
    );
}
