//! The fresh-database test of ADR 0107 decision 6: the store's migrations, applied to an empty
//! schema, create exactly the functions and views [`ROUTINES`] lists, and each `orch_` one answers a
//! call. A slice that adds, replaces or drops one moves this list in the same pull request, so a
//! function a migration leaves behind, or one a migration forgot, fails here. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_store_postgres::sql::{Client, query_scalar, raw_sql};
use support::TestDb;

/// Every function and view of the store's schema, as `name(identity arguments)` (a view as
/// `view name`), each with the statement that calls it once on the migrated, empty schema. `None`
/// is a routine that cannot be called on its own: a trigger's function, fired by its table.
const ROUTINES: &[(&str, Option<&str>)] = &[
    // 0024: the outbox row's owner, from its thread when an insert gives none. A trigger's
    // function from before ADR 0107, so it has no `orch_` prefix; `outbox` inserts fire it in every
    // conformance case that writes an outbox row.
    ("outbox_set_owner()", None),
];

/// The functions (of any kind) and the views of `schema`, sorted.
async fn routines(conn: &Client, schema: &str) -> Vec<String> {
    query_scalar(
        "SELECT p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')' \
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace WHERE n.nspname = $1 \
         UNION ALL SELECT 'view ' || viewname::text FROM pg_views WHERE schemaname = $1 \
         ORDER BY 1",
    )
    .bind(schema.to_owned())
    .fetch_all(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn every_function_is_created_and_answers() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    drop(db.store().await);
    let conn = support::connect_with(&db.options()).await;

    let mut listed: Vec<&str> = ROUTINES.iter().map(|(name, _)| *name).collect();
    listed.sort_unstable();
    assert_eq!(
        routines(&conn, &db.schema).await,
        listed,
        "the migrations' functions and views differ from ROUTINES: a slice that adds, changes or \
         drops one lists it here (ADR 0107 decision 6)"
    );
    for (name, call) in ROUTINES {
        assert!(
            name.starts_with("orch_") || call.is_none(),
            "{name}: a function of ADR 0107 is named orch_…"
        );
        if let Some(call) = call {
            raw_sql(call, &conn)
                .await
                .unwrap_or_else(|e| panic!("{name}: {call}: {e}"));
        }
    }
}

/// The listing sees what a migration would leave: a stray `orch_` function and view created
/// after the migrations are found, so the test above cannot pass on a query that sees nothing.
#[tokio::test]
async fn a_stray_function_or_view_is_found() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    drop(db.store().await);
    let conn = support::connect_with(&db.options()).await;
    raw_sql(
        "CREATE FUNCTION orch_stray(a bigint, b text) RETURNS bigint LANGUAGE sql \
         IMMUTABLE RETURN a; \
         CREATE VIEW orch_stray_view AS SELECT 1 AS one",
        &conn,
    )
    .await
    .unwrap();
    let found = routines(&conn, &db.schema).await;
    assert!(
        found.iter().any(|r| r == "orch_stray(a bigint, b text)"),
        "{found:?}"
    );
    assert!(
        found.iter().any(|r| r == "view orch_stray_view"),
        "{found:?}"
    );
}
