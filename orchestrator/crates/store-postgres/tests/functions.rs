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
    // 0037 (P1): the opening of a commit.
    (
        "orch_commit_open(p_thread uuid, p_expected_version bigint, p_outbox_id uuid, \
         p_outbox_owner text, p_outbox_attempts integer, p_inbox_id uuid, p_inbox_owner text, \
         p_inbox_attempts integer, p_keys text[])",
        // a seeded thread, so a renamed column of `threads`, `outbox` or `inbox` fails here: the
        // answer is `applied` for the thread's own version, with the fences and a key it holds
        Some(
            "INSERT INTO threads (id, owner, title, agent_id, state, version, last_seq, created_at, \
             updated_at, rail_rank) VALUES ('00000000-0000-7000-8000-0000000000f1', 'o', 't', 'a', \
             'queued', 1, 1, now(), now(), 'h'); \
             INSERT INTO events (thread_id, seq, at, kind, actor, data, idempotency_key) VALUES \
             ('00000000-0000-7000-8000-0000000000f1', 1, now(), 'agent_status', '{}', '{}', 'k'); \
             INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, lease_owner, \
             lease_until, next_attempt_at, created_at, updated_at) VALUES \
             ('00000000-0000-7000-8000-0000000000f2', '00000000-0000-7000-8000-0000000000f1', \
             'delegate', '{}', 'inflight', 1, 'w', now() + interval '1 hour', now(), now(), now()); \
             INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, available_at, \
             attempts, lease_owner, lease_until, created_at, updated_at) VALUES \
             ('00000000-0000-7000-8000-0000000000f3', 's', 'k', 'timer', '{}', 'inflight', now(), 2, \
             'i', now() + interval '1 hour', now(), now()); \
             DO $$ BEGIN IF (SELECT outcome FROM orch_commit_open(\
             '00000000-0000-7000-8000-0000000000f1', 1, '00000000-0000-7000-8000-0000000000f2', 'w', 1, \
             '00000000-0000-7000-8000-0000000000f3', 'i', 2, '{k2}')) <> 'applied' \
             OR (SELECT outcome FROM orch_commit_open('00000000-0000-7000-8000-0000000000f1', 1, \
             NULL, NULL, NULL, NULL, NULL, NULL, '{k}')) <> 'duplicate' \
             OR (SELECT outcome FROM orch_commit_open(gen_random_uuid(), 1, NULL, NULL, NULL, NULL, \
             NULL, NULL, '{}')) <> 'not_found' THEN RAISE EXCEPTION 'outcome'; END IF; END $$",
        ),
    ),
    // 0037 (P1): how far the usage ledger has counted a log; the greater of the two.
    (
        "orch_usage_counted_through(p_usage_through bigint, p_forked_at bigint)",
        Some(
            "DO $$ BEGIN IF orch_usage_counted_through(3, NULL) <> 3 \
             OR orch_usage_counted_through(3, 7) <> 7 OR orch_usage_counted_through(9, 7) <> 9 \
             THEN RAISE EXCEPTION 'counted'; END IF; END $$",
        ),
    ),
    // 0038 (P2): admission.
    (
        "orch_admit(p_owner text, p_limit integer)",
        // both outcomes for a person with no open delegation: room, and none at a limit of zero.
        // The count reads `outbox`, so a renamed column of it fails here.
        Some(
            "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM orch_admit('nobody', 1) \
             WHERE outcome = 'admitted' AND running = 0) \
             OR NOT EXISTS (SELECT 1 FROM orch_admit('nobody', 0) \
             WHERE outcome = 'no_room' AND running = 0) \
             THEN RAISE EXCEPTION 'admit'; END IF; END $$",
        ),
    ),
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

/// ADR 0107 decision 1: a PL/pgSQL function carries `SET search_path FROM CURRENT`, so it resolves
/// its tables through the migration's search path and not the caller's. A function without it
/// would work in a test whose every connection sets the path, and fail in a deployment whose
/// roles do not.
#[tokio::test]
async fn every_plpgsql_function_pins_its_search_path() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    drop(db.store().await);
    let conn = support::connect_with(&db.options()).await;
    let unpinned: Vec<String> = query_scalar(
        "SELECT p.proname::text FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace JOIN pg_language l ON l.oid = p.prolang \
         WHERE n.nspname = $1 AND l.lanname = 'plpgsql' AND p.proname LIKE 'orch\\_%' \
         AND NOT EXISTS (SELECT 1 FROM unnest(coalesce(p.proconfig, '{}')) c \
                         WHERE c LIKE 'search_path=%') ORDER BY 1",
    )
    .bind(db.schema.clone())
    .fetch_all(&conn)
    .await
    .unwrap();
    assert!(
        unpinned.is_empty(),
        "no `SET search_path FROM CURRENT`: {unpinned:?}"
    );
    // the check sees a function that lacks it
    raw_sql(
        "CREATE FUNCTION orch_unpinned() RETURNS integer LANGUAGE plpgsql AS $$ BEGIN RETURN 1; END $$",
        &conn,
    )
    .await
    .unwrap();
    let found: Vec<String> = query_scalar(
        "SELECT p.proname::text FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = $1 AND p.proname = 'orch_unpinned' AND p.proconfig IS NULL",
    )
    .bind(db.schema.clone())
    .fetch_all(&conn)
    .await
    .unwrap();
    assert_eq!(found, ["orch_unpinned"]);
}
