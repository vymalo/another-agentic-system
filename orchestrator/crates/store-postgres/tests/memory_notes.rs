//! `MemoryNoteStore` on `PgStore` against a real Postgres (ADR 0091): the conformance suite, and
//! what it cannot say about this adapter, which is how it is built.
//!
//! **This suite cannot skip.** The other Postgres tests print "skipped" when `ORCH_TEST_DATABASE_URL`
//! is unset, because they test a store a developer may not have; this one is the evidence that the
//! owner predicate is in the adapter's queries, so a run without a database **fails** instead of
//! passing with nothing done. The cases come from `memory_note_store_conformance!`, whose `make` here
//! is never `None`, so none is skipped and none is missing.
//!
//! The migration is applied to a database at the previous number, the table's CHECKs refuse what the
//! rules would never write, and the read uses the `(owner, ord)` index.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod history;
mod support;

use orch_core::{ForgetInput, Grant, MAX_NOTE_BYTES, ReadInput, SaveInput, Timestamp, UserId};
use orch_ports::{MemoryNoteStore, MemoryNoteStoreError};
use orch_store_postgres::PgStore;
use orch_store_postgres::sql::{query, query_as, query_scalar};
use support::TestDb;
use uuid::Uuid;

/// The store of this test, or a failure: this suite has no "skipped" branch.
async fn store() -> PgStore {
    let db = TestDb::new().await.expect(
        "ORCH_TEST_DATABASE_URL is unset: the memory note conformance suite cannot skip, so it \
         fails here instead of passing with nothing done",
    );
    db.store().await
}

fn owner() -> UserId {
    UserId::new(&format!("{}@example.test", Uuid::now_v7().simple()))
}

fn at() -> Timestamp {
    Timestamp::from_second(1_700_000_000).expect("a time")
}

/// The suite's `make`: always `Some`, because `store()` fails without a database where the other
/// suites' `make` returns `None` to skip.
async fn make_store() -> Option<PgStore> {
    Some(store().await)
}

// The macro writes one test per case of the testkit, from the one list in `orch_ports`: a case added
// there runs here with no edit to this file, so it cannot be left out of the Postgres run.
orch_ports::memory_note_store_conformance!(make_store);

// ------------------------------------------------------------------------------- the migration

/// Migration 0038 on a database that has run 0001 to 0037: the tables are not there, `migrate()`
/// applies exactly the one migration (the history ends at 37, then at 38), and the store works on
/// what it made, a note and a grant included.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn migration_0038_applies_to_a_database_at_the_previous_number() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    let pool = db.pool("orch-test-upgrade", 4).await;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.as_str() < "0038" {
            std::fs::copy(&path, dir.join(&name)).unwrap();
        }
    }
    history::migrate_dir(&pool, &dir).await;
    std::fs::remove_dir_all(&dir).unwrap();

    let versions = |pool: orch_store_postgres::PgPool| async move {
        let rows: Vec<(i64,)> = query_as("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
        rows.into_iter().map(|(v,)| v).collect::<Vec<_>>()
    };
    let before = versions(pool.clone()).await;
    assert_eq!(before.last(), Some(&37), "the database is at 0037");
    assert!(
        query("SELECT 1 FROM user_memories")
            .execute(&pool)
            .await
            .is_err(),
        "the notes table exists before its migration"
    );
    assert!(
        query("SELECT 1 FROM user_memory_settings")
            .execute(&pool)
            .await
            .is_err(),
        "the settings table exists before its migration"
    );

    let store = PgStore::from_pool(pool.clone());
    store.migrate().await.unwrap();
    let after = versions(pool).await;
    assert_eq!(after.last(), Some(&38), "the database is at 0038");
    assert_eq!(after.len(), before.len() + 1, "exactly one migration ran");

    let a = owner();
    let saved = store
        .save_note(
            &a,
            &SaveInput("after the upgrade".to_owned()),
            "person",
            "m-up",
            at(),
        )
        .await
        .unwrap();
    assert_eq!(saved.id, "m-up");
    store
        .set_memory_grant(
            &a,
            "coder",
            Grant {
                read: true,
                write: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(store.list_notes(&a).await.unwrap().len(), 1);
    // a second start on a current schema runs nothing
    store.migrate().await.unwrap();
}

// --------------------------------------------------------------------------------- the table

/// The table's CHECKs refuse what the rules would never write: an empty or an over-long text, an id
/// of 65 bytes, an empty owner, and an agent of 129 bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tables_checks_refuse_what_the_rules_refuse() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    let store = db.store().await;
    let insert = "INSERT INTO user_memories (owner, id, body, written_by, saved_at) \
                  VALUES ($1, $2, $3, 'person', now())";
    let who = owner();
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            insert,
            "m-1".into(),
            String::new(),
            "user_memories_body_check",
        ),
        (
            insert,
            "m-2".into(),
            "a".repeat(MAX_NOTE_BYTES + 1),
            "user_memories_body_check",
        ),
        (
            insert,
            "m-".repeat(33),
            "ok".into(),
            "user_memories_id_check",
        ),
    ];
    for (sql, id, body, constraint) in cases {
        let error = query(sql)
            .bind(who.as_str())
            .bind(id)
            .bind(body)
            .execute(store.pool())
            .await
            .expect_err(constraint);
        let words = format!("{error:?}");
        assert!(words.contains(constraint), "{constraint}: {words}");
    }
    let error = query(insert)
        .bind("")
        .bind("m-3")
        .bind("ok")
        .execute(store.pool())
        .await
        .expect_err("an empty owner");
    assert!(format!("{error:?}").contains("user_memories_owner_check"));
    let error = query("INSERT INTO user_memory_settings (owner, agent) VALUES ($1, $2)")
        .bind(who.as_str())
        .bind("a".repeat(129))
        .execute(store.pool())
        .await
        .expect_err("an agent of 129 bytes");
    assert!(format!("{error:?}").contains("user_memory_settings_agent_check"));
}

/// The owner is a column of every row, and what a person saved is counted under them alone: two
/// people's notes of one id are two rows, each under its own owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_is_on_every_row() {
    let store = store().await;
    let (a, b) = (owner(), owner());
    for who in [&a, &b] {
        store
            .save_note(
                who,
                &SaveInput("mine".to_owned()),
                "person",
                "m-shared-id",
                at(),
            )
            .await
            .unwrap();
    }
    for who in [&a, &b] {
        let rows: i64 =
            query_scalar("SELECT count(*) FROM user_memories WHERE owner = $1 AND id = $2")
                .bind(who.as_str())
                .bind("m-shared-id")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(rows, 1);
    }
    assert!(
        store
            .forget_note(&a, &ForgetInput("m-shared-id".to_owned()))
            .await
            .unwrap()
    );
    assert_eq!(store.list_notes(&b).await.unwrap().len(), 1);
    // the second read of b is unharmed, and a's is empty
    let read = store
        .read_notes(
            &a,
            &ReadInput {
                query: None,
                limit: 50,
            },
        )
        .await
        .unwrap();
    assert_eq!(read.total, 0);
}

/// A text Postgres cannot store is refused before the statement, as `Internal`, and writes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_text_with_a_nul_is_refused_and_writes_nothing() {
    let store = store().await;
    let a = owner();
    let refused = store
        .save_note(&a, &SaveInput("a\0b".to_owned()), "person", "m-nul", at())
        .await;
    assert!(
        matches!(refused, Err(MemoryNoteStoreError::Internal { .. })),
        "{refused:?}"
    );
    assert!(store.list_notes(&a).await.unwrap().is_empty());
}

/// The read uses the `(owner, ord)` index: with sequential scans forbidden, the plan is an index
/// scan of `user_memories_owner_ord`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_uses_the_owner_index() {
    let db = TestDb::new().await.expect("ORCH_TEST_DATABASE_URL");
    // the store migrates this test's schema: the table has to exist before it is asked a plan
    let store = db.store().await;
    let owner = owner();
    let mut tx = store.pool().begin().await.expect("begin");
    query("SET LOCAL enable_seqscan = off")
        .execute(&mut tx)
        .await
        .expect("disable sequential scans");
    let plan: Vec<(String,)> = query_as(
        "EXPLAIN SELECT id, body, saved_at, written_by, count(*) OVER () FROM user_memories \
         WHERE owner = $1 AND ($2::text IS NULL OR position(lower($2::text) IN lower(body)) > 0) \
         ORDER BY ord DESC LIMIT $3",
    )
    .bind(owner.as_str())
    .bind(None::<&str>)
    .bind(5_i64)
    .fetch_all(&mut tx)
    .await
    .expect("explain");
    tx.rollback().await.expect("done");
    let plan: String = plan
        .into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("user_memories_owner_ord"),
        "the read does not use the owner's index:\n{plan}"
    );
}
