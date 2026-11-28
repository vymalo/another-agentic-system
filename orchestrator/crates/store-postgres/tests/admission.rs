//! Admission (ADR 0081; ADR 0107 slice P2, migration `0038`): `orch_admit`, asked directly, and that
//! `admit_commit` runs it in the commit's own transaction, which the store testkit cannot see.
//! Skipped unless `ORCH_TEST_DATABASE_URL` is set; each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use orch_ports::{
    Admission, Commit, CommitOutcome, NewEvent, NewOutbox, NewThreadRecord, OutboxId,
    OutboxPayload, ThreadStore,
};
use orch_store_postgres::sql::{Tx, query, query_scalar};
use orch_store_postgres::{PgPool, PgStore};
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

/// The class of the admission lock, as every build of the store has taken it (`ADMISSION_LOCK_CLASS`).
const LOCK_CLASS: i32 = 0x6164_6d74;

/// How long a call that must not wait for ever is given.
const BOUND: Duration = Duration::from_secs(10);

/// A person of this run only: an advisory lock is the whole database's, not one schema's.
fn a_person() -> UserId {
    UserId::new(&format!("{}@example.com", Uuid::now_v7().simple()))
}

fn t0() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn commit(state: ThreadState, events: Vec<NewEvent>, outbox: Vec<NewOutbox>) -> Commit {
    Commit {
        new_state: state,
        job: None,
        events,
        outbox,
        binding: None,
        now: t0(),
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

/// A commit that starts a job: one message and one delegation.
fn a_job(owner: &UserId) -> Commit {
    let message = NewEvent {
        at: t0(),
        actor: Actor::user(owner),
        body: EventBody::UserMessage(UserMessageData::new("go")),
        idempotency_key: None,
    };
    let delegate = NewOutbox {
        id: OutboxId(Uuid::now_v7()),
        payload: OutboxPayload::Delegate {
            text: "go".into(),
            release: None,
            new_job: false,
            ui_catalog: None,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick: None,
            signed_prompt: None,
        },
    };
    commit(ThreadState::Queued, vec![message], vec![delegate])
}

/// A thread of `owner` at version 1 with no open job.
async fn idle_thread(store: &PgStore, owner: &UserId) -> ThreadId {
    let id = ThreadId(Uuid::now_v7());
    store
        .create_thread(
            NewThreadRecord {
                id,
                owner: owner.clone(),
                title: "t".into(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                child_of: None,
                now: t0(),
            },
            commit(ThreadState::Done, Vec::new(), Vec::new()),
        )
        .await
        .unwrap();
    id
}

/// An open delegation of `thread`, written by hand in `tx`; the trigger gives it the thread's owner.
async fn open_delegation(tx: &mut Tx, thread: ThreadId) {
    query(
        "INSERT INTO outbox (id, thread_id, kind, payload, status, next_attempt_at, created_at, \
         updated_at) VALUES ($1, $2, 'delegate', '{}'::jsonb, 'pending', now(), now(), now())",
    )
    .bind(Uuid::now_v7())
    .bind(thread.0)
    .execute(&mut **tx)
    .await
    .unwrap();
}

/// What `orch_admit` answered in `tx`: the outcome and the count.
async fn admit(tx: &mut Tx, owner: &UserId, limit: i32) -> (String, i32) {
    let row = query("SELECT * FROM orch_admit($1, $2)")
        .bind(owner.as_str())
        .bind(limit)
        .fetch_one(&mut **tx)
        .await
        .unwrap();
    (row.get("outcome"), row.get("running"))
}

/// The calls that wait for the admission lock of a person (`$1` the class, `$2` the owner).
const WAITING: &str = "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
     AND classid = $1::oid AND objid = hashtext($2)::oid AND objsubid = 2";

/// Waits until `count`, a `count(*)` over `pg_locks` for the admission lock of `owner` (`$1` the
/// class, `$2` the owner), is at least `at_least`.
async fn until_seen(pool: &PgPool, count: &'static str, owner: &UserId, at_least: i64) {
    tokio::time::timeout(BOUND, async {
        loop {
            let n: i64 = query_scalar(count)
                .bind(LOCK_CLASS)
                .bind(owner.as_str())
                .fetch_one(pool)
                .await
                .unwrap();
            if n >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("never seen in pg_locks: {count}"));
}

/// The decisive case of P2: the admission and the commit are one transaction on one connection. A
/// store whose pool holds one connection answers two admissions of one person with one place: one is
/// admitted, the other is told there is no room. A store that holds the lock on a connection of its
/// own while the commit asks the pool for another never gets it, and this runs out of time.
#[tokio::test]
async fn a_pool_of_one_admits_and_commits() {
    let db = db_or_skip!();
    drop(db.store().await);
    let store = PgStore::from_pool(db.pool("admission-one", 1).await);
    let owner = a_person();
    let (a, b) = (
        idle_thread(&store, &owner).await,
        idle_thread(&store, &owner).await,
    );
    let (x, y) = tokio::time::timeout(BOUND, async {
        tokio::join!(
            store.admit_commit(&owner, 1, a, 1, a_job(&owner)),
            store.admit_commit(&owner, 1, b, 1, a_job(&owner)),
        )
    })
    .await
    .expect("an admission needs one connection of the pool, not two");
    let answers = [x.unwrap(), y.unwrap()];
    let admitted = answers
        .iter()
        .filter(|r| matches!(r, Admission::Admitted(CommitOutcome::Applied { .. })))
        .count();
    assert_eq!(admitted, 1, "{answers:?}");
    assert!(
        answers.contains(&Admission::NoRoom { running: 1 }),
        "{answers:?}"
    );
}

/// The transaction that holds the person's admission lock is the one that commits: while the commit
/// waits for the thread's row, which the test holds, the session that waits is the session that holds
/// the admission lock. A lock taken in a transaction that ended before the commit, or held by another
/// one, is never seen so.
#[tokio::test]
async fn the_admission_lock_is_held_by_the_transaction_that_commits() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = db.pool("admission-test", 4).await;
    let owner = a_person();
    let thread = idle_thread(&store, &owner).await;

    let mut blocker = pool.begin().await.unwrap();
    query("SELECT 1 FROM threads WHERE id = $1 FOR UPDATE")
        .bind(thread.0)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let admission = {
        let (store, owner) = (store.clone(), owner.clone());
        tokio::spawn(async move {
            store
                .admit_commit(&owner, 1, thread, 1, a_job(&owner))
                .await
        })
    };
    until_seen(
        &pool,
        "SELECT count(*) FROM pg_locks a JOIN pg_locks w ON w.pid = a.pid AND NOT w.granted \
         WHERE a.locktype = 'advisory' AND a.granted AND a.classid = $1::oid \
         AND a.objid = hashtext($2)::oid AND a.objsubid = 2",
        &owner,
        1,
    )
    .await;
    blocker.rollback().await.unwrap();
    let answer = tokio::time::timeout(BOUND, admission)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(answer, Admission::Admitted(CommitOutcome::Applied { .. })),
        "{answer:?}"
    );
}

/// The count follows the lock: a second admission waits for the transaction that holds the lock and
/// then counts the job that transaction committed. A count taken before the lock would not hold it.
#[tokio::test]
async fn the_count_follows_the_lock() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = db.pool("admission-test", 4).await;
    let owner = a_person();
    let thread = idle_thread(&store, &owner).await;

    let mut first = pool.begin().await.unwrap();
    assert_eq!(admit(&mut first, &owner, 1).await, ("admitted".into(), 0));
    open_delegation(&mut first, thread).await;
    // the count is a fresh one at each call: the same transaction sees what it wrote since
    assert_eq!(admit(&mut first, &owner, 1).await, ("no_room".into(), 1));

    let second = {
        let (pool, owner) = (pool.clone(), owner.clone());
        tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let answer = admit(&mut tx, &owner, 1).await;
            tx.rollback().await.unwrap();
            answer
        })
    };
    until_seen(&pool, WAITING, &owner, 1).await;
    first.commit().await.unwrap();
    let answer = tokio::time::timeout(BOUND, second).await.unwrap().unwrap();
    assert_eq!(answer, ("no_room".into(), 1));
}

/// An admission does not depend on what the database defaults to. With `REPEATABLE READ` as the
/// default (`ALTER ROLE ... SET default_transaction_isolation`), a transaction that did not say its
/// level would take its snapshot as it calls `orch_admit`, before the wait for the lock. The test
/// holds the lock until both admissions of a person with one place wait for it, so both have begun
/// before either commits: one is admitted, and the other counts that job.
#[tokio::test]
async fn an_admission_does_not_depend_on_the_isolation_the_database_defaults_to() {
    let db = db_or_skip!();
    // migrated on the direct connection: no pooled session exists before the settings below are
    // the database's
    PgStore::migrate_direct(&db.pinned_direct_url())
        .await
        .unwrap();
    let store = PgStore::from_pool(
        db.pool_with(
            "admission-isolation",
            4,
            &[("default_transaction_isolation", "repeatable read")],
        )
        .await,
    );
    let level: String = query_scalar("SHOW default_transaction_isolation")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(
        level, "repeatable read",
        "the pool's sessions default to it"
    );
    let pool = db.pool("admission-test", 4).await;
    let owner = a_person();
    let threads = [
        idle_thread(&store, &owner).await,
        idle_thread(&store, &owner).await,
    ];

    let mut holder = pool.begin().await.unwrap();
    query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(LOCK_CLASS)
        .bind(owner.as_str())
        .execute(&mut *holder)
        .await
        .unwrap();
    let admissions = threads.map(|thread| {
        let (store, owner) = (store.clone(), owner.clone());
        tokio::spawn(async move {
            store
                .admit_commit(&owner, 1, thread, 1, a_job(&owner))
                .await
        })
    });
    until_seen(&pool, WAITING, &owner, 2).await;
    holder.rollback().await.unwrap();
    let mut answers = Vec::new();
    for admission in admissions {
        let answer = tokio::time::timeout(BOUND, admission).await.unwrap();
        answers.push(answer.unwrap().unwrap());
    }
    let admitted = answers
        .iter()
        .filter(|r| matches!(r, Admission::Admitted(CommitOutcome::Applied { .. })))
        .count();
    assert_eq!(admitted, 1, "{answers:?}");
    assert!(
        answers.contains(&Admission::NoRoom { running: 1 }),
        "{answers:?}"
    );
}

/// A replica of a build before migration `0038` takes the lock in a statement of its own, with the
/// class as a parameter: `orch_admit` waits for it, so the two exclude each other during a rollout.
#[tokio::test]
async fn an_older_builds_lock_holds_the_function_back() {
    let db = db_or_skip!();
    drop(db.store().await);
    let pool = db.pool("admission-test", 4).await;
    let owner = a_person();

    let mut older = pool.begin().await.unwrap();
    query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(LOCK_CLASS)
        .bind(owner.as_str())
        .execute(&mut *older)
        .await
        .unwrap();
    let mut newer = pool.begin().await.unwrap();
    query("SET LOCAL lock_timeout = '300ms'")
        .execute(&mut *newer)
        .await
        .unwrap();
    let err = query("SELECT * FROM orch_admit($1, 1)")
        .bind(owner.as_str())
        .fetch_one(&mut *newer)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("55P03"), "{err:?}");
    // another person's admission is not held back
    newer.rollback().await.unwrap();
    let mut other = pool.begin().await.unwrap();
    query("SET LOCAL lock_timeout = '300ms'")
        .execute(&mut *other)
        .await
        .unwrap();
    assert_eq!(
        admit(&mut other, &a_person(), 1).await,
        ("admitted".into(), 0)
    );
}

/// A person at the limit is an answer, and the call writes nothing: its transaction has not even
/// been given an id, which PostgreSQL gives at the first write.
#[tokio::test]
async fn a_person_at_the_limit_is_no_room_and_nothing_is_written() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = db.pool("admission-test", 4).await;
    let owner = a_person();
    let thread = idle_thread(&store, &owner).await;
    // bounded: an admission lock that outlived its transaction must fail here, not hang
    let admitted = tokio::time::timeout(
        BOUND,
        store.admit_commit(&owner, 1, thread, 1, a_job(&owner)),
    )
    .await
    .expect("nobody holds the admission lock")
    .unwrap();
    assert!(
        matches!(admitted, Admission::Admitted(CommitOutcome::Applied { .. })),
        "{admitted:?}"
    );

    let mut tx = pool.begin().await.unwrap();
    query("SET LOCAL lock_timeout = '10s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(admit(&mut tx, &owner, 1).await, ("no_room".into(), 1));
    assert_eq!(admit(&mut tx, &owner, 0).await, ("no_room".into(), 1));
    assert_eq!(admit(&mut tx, &owner, 2).await, ("admitted".into(), 1));
    let wrote_nothing: bool = query_scalar("SELECT pg_current_xact_id_if_assigned() IS NULL")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(wrote_nothing);
}

#[tokio::test]
async fn arguments_that_are_a_callers_bug_are_refused() {
    let db = db_or_skip!();
    drop(db.store().await);
    let pool = db.pool("admission-test", 4).await;
    for (what, owner, limit) in [
        ("no owner", None, Some(1)),
        ("an empty owner", Some(""), Some(1)),
        ("no limit", Some("alice@example.com"), None),
        ("a negative limit", Some("alice@example.com"), Some(-1)),
    ] {
        let err = query("SELECT * FROM orch_admit($1, $2)")
            .bind(owner)
            .bind(limit)
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("OR002"), "{what}: {err:?}");
    }
}
