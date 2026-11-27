//! `orch_commit_open` and the append of the log (ADR 0107 slice P1, migration `0037`), asked
//! directly: what the store testkit cannot say because it is about the function. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set; each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use orch_store_postgres::PgPool;
use orch_store_postgres::sql::{Row, query};
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

/// A lease's id, owner and attempts, each as the caller may leave it out.
type LeaseArgs = (Option<Uuid>, Option<String>, Option<i32>);
/// What is wrong, the thread, the version, the outbox lease and the inbox lease.
type Case = (
    &'static str,
    Option<Uuid>,
    Option<i64>,
    LeaseArgs,
    LeaseArgs,
);

const THREAD: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001);
const OUTBOX: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0002);
const INBOX: Uuid = Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0003);

/// A thread at version 3 with five events' worth of `last_seq`, a claimed outbox row (`w`, attempt
/// 1) and a claimed inbox row (`i`, attempt 2), and one event with the key `k1`.
async fn seed(pool: &PgPool) {
    query(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, last_seq, created_at, \
         updated_at, rail_rank) VALUES ($1, 'alice@example.com', 't', 'coder', 'working', 3, 5, \
         now(), now(), 'h')",
    )
    .bind(THREAD)
    .execute(pool)
    .await
    .unwrap();
    query(
        "INSERT INTO events (thread_id, seq, at, kind, actor, data, idempotency_key) \
         VALUES ($1, 5, now(), 'agent_status', '{\"type\":\"agent\",\"name\":\"coder\"}', \
         '{\"status\":\"working\"}', 'k1')",
    )
    .bind(THREAD)
    .execute(pool)
    .await
    .unwrap();
    query(
        "INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, lease_owner, \
         lease_until, next_attempt_at, created_at, updated_at) VALUES ($1, $2, 'delegate', \
         '{\"delegate\":{\"text\":\"x\",\"release\":null}}'::jsonb, 'inflight', 1, 'w', \
         now() + interval '1 hour', now(), now(), now())",
    )
    .bind(OUTBOX)
    .bind(THREAD)
    .execute(pool)
    .await
    .unwrap();
    query(
        "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, available_at, \
         attempts, lease_owner, lease_until, created_at, updated_at) VALUES ($1, 's', 'k', \
         'timer', '{}'::jsonb, 'inflight', now(), 2, 'i', now() + interval '1 hour', now(), now())",
    )
    .bind(INBOX)
    .execute(pool)
    .await
    .unwrap();
}

/// What `orch_commit_open` answered for these arguments.
async fn open(
    pool: &PgPool,
    thread: Uuid,
    version: i64,
    outbox: Option<(&str, i32)>,
    inbox: Option<(&str, i32)>,
    keys: &[&str],
) -> Row {
    query("SELECT * FROM orch_commit_open($1, $2, $3, $4, $5, $6, $7, $8, $9)")
        .bind(thread)
        .bind(version)
        .bind(outbox.map(|_| OUTBOX))
        .bind(outbox.map(|(owner, _)| owner.to_owned()))
        .bind(outbox.map(|(_, attempts)| attempts))
        .bind(inbox.map(|_| INBOX))
        .bind(inbox.map(|(owner, _)| owner.to_owned()))
        .bind(inbox.map(|(_, attempts)| attempts))
        .bind(keys.to_vec())
        .fetch_one(pool)
        .await
        .unwrap()
}

fn outcome(row: &Row) -> String {
    row.get::<_, String>("outcome")
}

#[tokio::test]
async fn each_outcome_and_the_order_in_which_they_win() {
    let db = db_or_skip!();
    let pool = db.pool("commit-open", 4).await;
    drop(db.store().await);
    seed(&pool).await;

    let a = open(&pool, THREAD, 3, Some(("w", 1)), Some(("i", 2)), &["k2"]).await;
    assert_eq!(outcome(&a), "applied");
    // the locked row's values, and the ledger's watermark through the one definition
    assert_eq!(a.get::<_, i64>("cur_version"), 3);
    assert_eq!(a.get::<_, i64>("cur_last_seq"), 5);
    assert_eq!(a.get::<_, String>("cur_state"), "working");
    assert_eq!(a.get::<_, String>("cur_owner"), "alice@example.com");
    assert_eq!(a.get::<_, i64>("cur_counted_through"), 0);
    assert_eq!(a.get::<_, i32>("cur_search_fold"), 0);

    let missing = open(&pool, Uuid::from_u128(9), 3, Some(("w", 1)), None, &[]).await;
    assert_eq!(outcome(&missing), "not_found");

    // a claim that is not this commit's: the wrong owner, the wrong attempt
    for outbox in [("other", 1), ("w", 2)] {
        let r = open(&pool, THREAD, 3, Some(outbox), None, &[]).await;
        assert_eq!(outcome(&r), "fenced", "{outbox:?}");
    }
    for inbox in [("other", 2), ("i", 1)] {
        let r = open(&pool, THREAD, 3, None, Some(inbox), &[]).await;
        assert_eq!(outcome(&r), "fenced", "{inbox:?}");
    }
    // a claim that is no longer in flight
    query("UPDATE outbox SET status = 'pending' WHERE id = $1")
        .bind(OUTBOX)
        .execute(&pool)
        .await
        .unwrap();
    let r = open(&pool, THREAD, 3, Some(("w", 1)), None, &[]).await;
    assert_eq!(outcome(&r), "fenced");

    // the order: fenced beats a version that is wrong, a wrong version beats a duplicate key
    let r = open(&pool, THREAD, 2, Some(("w", 1)), None, &["k1"]).await;
    assert_eq!(outcome(&r), "fenced");
    let r = open(&pool, THREAD, 2, None, None, &["k1"]).await;
    assert_eq!(outcome(&r), "conflict");
    let r = open(&pool, THREAD, 3, None, None, &["k0", "k1"]).await;
    assert_eq!(outcome(&r), "duplicate");
    let r = open(&pool, THREAD, 3, None, None, &[]).await;
    assert_eq!(outcome(&r), "applied");
}

#[tokio::test]
async fn arguments_that_disagree_are_refused_before_a_lock_is_taken() {
    let db = db_or_skip!();
    let pool = db.pool("commit-open", 4).await;
    drop(db.store().await);
    seed(&pool).await;
    // a lease with a part missing (the outbox's, then the inbox's), a NULL thread, a NULL version:
    // a caller's bug, `OR002`
    let lease = |id: Option<Uuid>, owner: Option<&str>, attempts: Option<i32>| {
        (id, owner.map(str::to_owned), attempts)
    };
    let cases: Vec<Case> = vec![
        (
            "outbox owner",
            Some(THREAD),
            Some(3),
            lease(Some(OUTBOX), None, Some(1)),
            lease(None, None, None),
        ),
        (
            "outbox id",
            Some(THREAD),
            Some(3),
            lease(None, Some("w"), Some(1)),
            lease(None, None, None),
        ),
        (
            "outbox attempts",
            Some(THREAD),
            Some(3),
            lease(Some(OUTBOX), Some("w"), None),
            lease(None, None, None),
        ),
        (
            "inbox owner",
            Some(THREAD),
            Some(3),
            lease(None, None, None),
            lease(Some(INBOX), None, Some(2)),
        ),
        (
            "inbox id",
            Some(THREAD),
            Some(3),
            lease(None, None, None),
            lease(None, Some("i"), Some(2)),
        ),
        (
            "inbox attempts",
            Some(THREAD),
            Some(3),
            lease(None, None, None),
            lease(Some(INBOX), Some("i"), None),
        ),
        (
            "thread",
            None,
            Some(3),
            lease(None, None, None),
            lease(None, None, None),
        ),
        (
            "version",
            Some(THREAD),
            None,
            lease(None, None, None),
            lease(None, None, None),
        ),
    ];
    for (what, thread, version, outbox, inbox) in cases {
        let err = query("SELECT * FROM orch_commit_open($1, $2, $3, $4, $5, $6, $7, $8, '{}')")
            .bind(thread)
            .bind(version)
            .bind(outbox.0)
            .bind(outbox.1)
            .bind(outbox.2)
            .bind(inbox.0)
            .bind(inbox.1)
            .bind(inbox.2)
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("OR002"), "{what}: {err:?}");
    }
}

#[tokio::test]
async fn the_thread_stays_locked_until_the_transaction_ends() {
    let db = db_or_skip!();
    let pool = db.pool("commit-open", 4).await;
    drop(db.store().await);
    seed(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let row = query("SELECT * FROM orch_commit_open($1, 3, $2, 'w', 1, $3, 'i', 2, '{}')")
        .bind(THREAD)
        .bind(OUTBOX)
        .bind(INBOX)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(outcome(&row), "applied");
    // another connection that wants any of the three rows waits (here: gives up)
    for table_and_id in [("threads", THREAD), ("outbox", OUTBOX), ("inbox", INBOX)] {
        let mut other = pool.begin().await.unwrap();
        query("SET LOCAL lock_timeout = '300ms'")
            .execute(&mut *other)
            .await
            .unwrap();
        let sql = format!("SELECT 1 FROM {} WHERE id = $1 FOR UPDATE", table_and_id.0);
        let err = query(&sql)
            .bind(table_and_id.1)
            .fetch_optional(&mut *other)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("55P03"), "{}: {err:?}", table_and_id.0);
    }
    tx.commit().await.unwrap();
    // and free after it
    let mut other = pool.begin().await.unwrap();
    query("SET LOCAL lock_timeout = '300ms'")
        .execute(&mut *other)
        .await
        .unwrap();
    query("SELECT 1 FROM threads WHERE id = $1 FOR UPDATE")
        .bind(THREAD)
        .fetch_one(&mut *other)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_ledger_has_counted_the_greater_of_the_watermark_and_the_cut() {
    let db = db_or_skip!();
    let pool = db.pool("commit-open", 4).await;
    drop(db.store().await);
    for (through, cut) in [
        (0_i64, None),
        (4, None),
        (3, Some(0_i64)),
        (3, Some(7)),
        (9, Some(7)),
        (0, Some(7)),
    ] {
        let got: i64 = query("SELECT orch_usage_counted_through($1, $2)")
            .bind(through)
            .bind(cut)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            got,
            orch_core::usage_counted_through(through, cut),
            "{through} {cut:?}"
        );
    }
}
