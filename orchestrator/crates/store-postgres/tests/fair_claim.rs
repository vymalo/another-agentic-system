//! The fair claim over Postgres (ADR 0066, migration `0024`): what the conformance cases cannot say,
//! because it is about the table. Skipped unless `ORCH_TEST_DATABASE_URL` is set; each test runs in
//! its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use jiff::Timestamp;
use orch_ports::{ClaimOrder, ThreadStore};
use orch_store_postgres::PgStore;
use sqlx::{PgPool, Row};
use support::TestDb;
use uuid::Uuid;

macro_rules! db_or_skip {
    () => {
        match TestDb::new().await {
            Some(db) => db,
            None => {
                eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
                return;
            }
        }
    };
}

const PAYLOAD: &str = r#"{"delegate":{"text":"x","release":null}}"#;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
}

/// A thread of `owner`, as a build that knows nothing of the owner on the outbox wrote it.
async fn thread(pool: &PgPool, n: u128, owner: &str) {
    sqlx::query(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, created_at, updated_at, \
         rail_rank) VALUES ($1, $2, 't', 'coder', 'queued', 1, now(), now(), $3)",
    )
    .bind(id(n))
    .bind(owner)
    .bind(format!("{n:012x}h"))
    .execute(pool)
    .await
    .unwrap();
}

/// An outbox row for thread `n` as an older build inserts it: no owner, and a status.
async fn row(pool: &PgPool, n: u128, status: &str, lease_until: Option<&str>) {
    sqlx::query(
        "INSERT INTO outbox (id, thread_id, kind, payload, status, next_attempt_at, lease_until, \
         created_at, updated_at) VALUES ($1, $1, 'delegate', $2::jsonb, $3, now() - interval '1 minute', \
         $4::timestamptz, now(), now())",
    )
    .bind(id(n))
    .bind(PAYLOAD)
    .bind(status)
    .bind(lease_until)
    .execute(pool)
    .await
    .unwrap();
}

async fn owner_of_row(pool: &PgPool, n: u128) -> Option<String> {
    sqlx::query("SELECT owner FROM outbox WHERE id = $1")
        .bind(id(n))
        .fetch_one(pool)
        .await
        .unwrap()
        .get("owner")
}

async fn claim_ids(store: &PgStore, limit: u32) -> Vec<Uuid> {
    store
        .claim_outbox(
            "w",
            Timestamp::now(),
            Duration::from_secs(30),
            limit,
            ClaimOrder::Fair,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id.0)
        .collect()
}

/// A row an older build inserts, still running during a rolling update, has no owner in its
/// statement: the database gives it its thread's, and the fair claim finds it.
#[tokio::test]
async fn a_row_written_without_an_owner_gets_the_owner_of_its_thread_and_is_claimed() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = store.pool().clone();
    thread(&pool, 1, "alice@example.com").await;
    thread(&pool, 2, "bob@example.com").await;
    row(&pool, 1, "pending", None).await;
    row(&pool, 2, "pending", None).await;
    assert_eq!(
        owner_of_row(&pool, 1).await.as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(
        owner_of_row(&pool, 2).await.as_deref(),
        Some("bob@example.com")
    );
    assert_eq!(claim_ids(&store, 10).await, [id(1), id(2)]);
}

/// A row that can still be claimed always has an owner: one without would never be found.
#[tokio::test]
async fn an_open_row_cannot_be_left_without_an_owner() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = store.pool().clone();
    thread(&pool, 1, "alice@example.com").await;
    row(&pool, 1, "pending", None).await;
    let refused = sqlx::query("UPDATE outbox SET owner = NULL WHERE id = $1")
        .bind(id(1))
        .execute(&pool)
        .await
        .unwrap_err();
    let refused = refused.to_string();
    assert!(refused.contains("outbox_open_has_owner"), "{refused}");
    // a finished row needs none
    sqlx::query("UPDATE outbox SET status = 'delivered', owner = NULL WHERE id = $1")
        .bind(id(1))
        .execute(&pool)
        .await
        .unwrap();
}

/// Migration 0024 on a database that has run 0001 to 0023 and holds rows: the rows that can be
/// claimed get their thread's owner, the finished ones are left alone, and the fair claim then
/// serves the rows that were there (ADR 0066).
#[tokio::test]
async fn migration_0024_gives_the_open_rows_their_owner() {
    let db = db_or_skip!();
    let pool = db.pool("orch-test-upgrade", 4).await;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.as_str() < "0024" {
            std::fs::copy(&path, dir.join(&name)).unwrap();
        }
    }
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    // rows as the build before this one wrote them: pending, a lease that ran out, a lease that
    // has not, and one that is over
    for (n, owner) in [
        (1, "alice@example.com"),
        (2, "alice@example.com"),
        (3, "bob@example.com"),
        (4, "bob@example.com"),
    ] {
        thread(&pool, n, owner).await;
    }
    row(&pool, 1, "pending", None).await;
    row(&pool, 2, "inflight", Some("2000-01-01T00:00:00Z")).await;
    row(&pool, 3, "inflight", Some("2999-01-01T00:00:00Z")).await;
    row(&pool, 4, "delivered", None).await;

    let store = PgStore::from_pool(pool.clone());
    store.migrate().await.unwrap();
    assert_eq!(
        owner_of_row(&pool, 1).await.as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(
        owner_of_row(&pool, 2).await.as_deref(),
        Some("alice@example.com")
    );
    assert_eq!(
        owner_of_row(&pool, 3).await.as_deref(),
        Some("bob@example.com")
    );
    assert_eq!(
        owner_of_row(&pool, 4).await,
        None,
        "a finished row is not rewritten"
    );
    // the pending row and the one whose lease ran out are claimed, the running one is not
    assert_eq!(claim_ids(&store, 10).await, [id(1), id(2)]);
}

/// Rows that other claimers hold at this moment are skipped and the claim goes on to the next ones,
/// as the oldest-first claim does (the statement keeps `4 * limit` rows to choose among for this).
#[tokio::test]
async fn rows_another_claimer_holds_are_skipped_and_the_claim_takes_the_next() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = store.pool().clone();
    for n in 1..=20 {
        thread(&pool, n, "alice@example.com").await;
        row(&pool, n, "pending", None).await;
    }
    thread(&pool, 21, "bob@example.com").await;
    row(&pool, 21, "pending", None).await;
    // another claimer is in the middle of taking the first two
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM outbox WHERE id = ANY($1) FOR UPDATE")
        .bind(vec![id(1), id(2)])
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    // alice's loads are 1, 2, 3, ...; bob's is 1; rows 1 and 2 are held
    assert_eq!(claim_ids(&store, 2).await, [id(3), id(21)]);
    tx.rollback().await.unwrap();
}

/// A person's long queue is not ranked past what a claim can take, and neither hides another
/// person's row nor delays it.
#[tokio::test]
async fn a_long_queue_does_not_hide_another_persons_row() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = store.pool().clone();
    for n in 1..=200 {
        thread(&pool, n, "alice@example.com").await;
        row(&pool, n, "pending", None).await;
    }
    thread(&pool, 201, "bob@example.com").await;
    row(&pool, 201, "pending", None).await;
    assert_eq!(claim_ids(&store, 2).await, [id(1), id(201)]);
    assert_eq!(claim_ids(&store, 2).await, [id(2), id(3)]);
}
