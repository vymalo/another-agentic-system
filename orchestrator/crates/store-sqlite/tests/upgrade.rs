//! An upgrade of a database that already holds a log and an outbox (ADR 0035, amended 2026-11-28):
//! SQLite cannot alter a CHECK constraint, so `0010_thread_suggestions.sql` rebuilds `events` and
//! `outbox`. The rows of both, and of `message_terms` (which points at `events`), must survive; the
//! foreign keys must still hold; the indexes must come back; and deleting the thread must still
//! cascade. Needs no server.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use rusqlite::Connection;

/// The migrations before the one under test, in order.
const BEFORE: &[(&str, &str)] = &[
    ("0001_init.sql", include_str!("../migrations/0001_init.sql")),
    (
        "0002_usage_and_search.sql",
        include_str!("../migrations/0002_usage_and_search.sql"),
    ),
    (
        "0003_user_prompts.sql",
        include_str!("../migrations/0003_user_prompts.sql"),
    ),
    (
        "0004_thread_pick.sql",
        include_str!("../migrations/0004_thread_pick.sql"),
    ),
    (
        "0005_user_tool_servers.sql",
        include_str!("../migrations/0005_user_tool_servers.sql"),
    ),
    (
        "0006_user_tool_oauth_pending.sql",
        include_str!("../migrations/0006_user_tool_oauth_pending.sql"),
    ),
    (
        "0007_user_team_presets.sql",
        include_str!("../migrations/0007_user_team_presets.sql"),
    ),
    (
        "0008_user_tool_server_last4.sql",
        include_str!("../migrations/0008_user_tool_server_last4.sql"),
    ),
    (
        "0009_outbox_refunded.sql",
        include_str!("../migrations/0009_outbox_refunded.sql"),
    ),
];

/// A v9 database with one thread, one event, one message term and one outbox row, then the
/// migration under test.
fn upgraded() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    // The migration runner turns the foreign keys off around a migration, as the rebuild needs;
    // `SqliteStore::open` turns them on for normal work, and so does this test after the upgrade.
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    for (name, sql) in BEFORE {
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    conn.execute(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, rail_rank, created_at, \
         updated_at) VALUES ('t', 'o', 'ti', 'a', 'done', 1, 'a', 1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
         VALUES ('t', 1, 1, 'thread_described', '{}', '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message_terms (thread_id, seq, owner, key) VALUES ('t', 1, 'o', 'word')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO outbox (id, ord, thread_id, owner, kind, payload, status, attempts, \
         next_attempt_at, created_at, updated_at) \
         VALUES ('x', 1, 't', 'o', 'description', '{}', 'delivered', 1, 1, 1, 1)",
        [],
    )
    .unwrap();
    conn.execute_batch(include_str!("../migrations/0010_thread_suggestions.sql"))
        .unwrap();
    conn
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn migration_0010_upgrades_a_database_that_holds_a_log_and_an_outbox() {
    let conn = upgraded();
    // Every row survives the two rebuilds.
    assert_eq!(count(&conn, "threads"), 1);
    assert_eq!(count(&conn, "events"), 1);
    assert_eq!(count(&conn, "message_terms"), 1);
    assert_eq!(count(&conn, "outbox"), 1);
    // `message_terms` still points at `events`: its foreign key holds, and the rows are the same.
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let violations: Vec<(String, i64, String, i64)> = conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(violations.is_empty(), "{violations:?}");
    // The indexes the rebuild dropped are back.
    let indexes: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'index' \
             AND tbl_name IN ('events', 'outbox') ORDER BY 1",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for wanted in [
        "events_idempotency",
        "outbox_open",
        "outbox_open_thread",
        "outbox_open_owner",
        "outbox_inflight_owner",
        "outbox_open_cancel",
    ] {
        assert!(
            indexes.iter().any(|name| name == wanted),
            "{wanted} is missing: {indexes:?}"
        );
    }
    // The two new kinds are accepted, with the rows of the old ones still there.
    conn.execute(
        "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
         VALUES ('t', 2, 1, 'thread_suggested', '{}', '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO outbox (id, ord, thread_id, owner, kind, payload, status, attempts, \
         next_attempt_at, created_at, updated_at) \
         VALUES ('y', 2, 't', 'o', 'suggestions', '{}', 'delivered', 1, 1, 1, 1)",
        [],
    )
    .unwrap();
    assert_eq!(count(&conn, "events"), 2);
    assert_eq!(count(&conn, "outbox"), 2);
    // Deleting the thread still cascades the log, the index and the outbox.
    conn.execute("DELETE FROM threads WHERE id = 't'", [])
        .unwrap();
    assert_eq!(count(&conn, "events"), 0);
    assert_eq!(count(&conn, "message_terms"), 0);
    assert_eq!(count(&conn, "outbox"), 0);
}
