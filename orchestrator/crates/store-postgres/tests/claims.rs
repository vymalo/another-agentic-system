//! Claims, the inbox and watches (ADR 0107 slice P3, migration `0040`): what the store testkit
//! cannot say, because it is about the functions, the statements and the database's settings.
//! Skipped unless `ORCH_TEST_DATABASE_URL` is set; each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{AgentId, AgentTarget, ThreadId, ThreadState, UserId, WatchKey};
use orch_ports::{
    BindingUpdate, ClaimOrder, Commit, InboxId, InboxLease, InboxPayload, InboxStatus, NewInbox,
    NewOutbox, NewThreadRecord, OutboxId, OutboxPayload, Parking, ThreadStore,
};
use orch_store_postgres::sql::{query, query_as, query_scalar, raw_sql};
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

/// How long a call that must not wait for ever is given.
const BOUND: Duration = Duration::from_secs(10);
const LEASE: Duration = Duration::from_secs(30);

/// The lock of a watch key, as every build of the store has taken it.
const LOCK_WATCH: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

fn lock_name(key: &WatchKey) -> String {
    format!("orch:watch:{key}")
}

/// A key of this run only: an advisory lock is the whole database's, not one schema's.
fn a_key() -> WatchKey {
    let hex = Uuid::now_v7().simple().to_string();
    WatchKey::ci("github.com/o/r", &format!("{hex}{}", &hex[..8]))
}

fn t0() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0().checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn commit(outbox: Vec<NewOutbox>, watches: Vec<WatchKey>) -> Commit {
    Commit {
        new_state: ThreadState::Queued,
        job: None,
        events: Vec::new(),
        outbox,
        binding: None,
        now: t0(),
        lease: None,
        watches,
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

fn delegate(id: Uuid) -> NewOutbox {
    NewOutbox {
        id: OutboxId(id),
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
            mode: None,
            prompt_signature: None,
        },
    }
}

fn a_thread() -> NewThreadRecord {
    NewThreadRecord {
        id: ThreadId(Uuid::now_v7()),
        owner: UserId::new("alice@example.com"),
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
    }
}

/// A report about `key`, received at `now`, which is when it is due.
async fn receive(store: &PgStore, id: Uuid, key: &WatchKey, now: Timestamp) {
    let row = NewInbox {
        id: InboxId(id),
        source: "github".into(),
        idempotency_key: id.to_string(),
        payload: InboxPayload::CiReport(orch_core::CiReport {
            provider: orch_core::CiProvider::Github,
            repository: "github.com/o/r".into(),
            sha: "ab".repeat(20),
            branch: None,
            name: "build".into(),
            conclusion: orch_core::CiConclusion::Success,
            url: None,
            summary: None,
        }),
        correlation: Some(key.as_str().to_owned()),
    };
    store.receive(row, now).await.unwrap();
}

/// A received report about `key` and the claim on it.
async fn a_claimed_report(store: &PgStore, key: &WatchKey) -> InboxLease {
    receive(store, Uuid::now_v7(), key, t0()).await;
    let claimed = store.claim_inbox("a", t0(), LEASE, 1).await.unwrap();
    claimed[0].lease().unwrap()
}

/// Waits until some backend is blocked on the lock of `key`, straight from `pg_locks`: the proof
/// that a call is waiting where the test wants it. (The sleeps only pace the polling.)
async fn until_blocked_on(pool: &PgPool, key: &WatchKey) {
    for _ in 0..500 {
        let blocked: bool = query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
             AND ((classid::bigint << 32) | objid::bigint) = hashtextextended($1, 0))",
        )
        .bind(lock_name(key))
        .fetch_one(pool)
        .await
        .unwrap();
        if blocked {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("nothing ever blocked on the watch lock of {key}");
}

/// A store whose sessions default to `REPEATABLE READ` (what `ALTER ROLE ... SET` does).
async fn repeatable_read_store(db: &TestDb) -> PgStore {
    // migrated on the direct connection: no pooled session exists before the setting is made
    PgStore::migrate_direct(&db.pinned_direct_url())
        .await
        .unwrap();
    let store = PgStore::from_pool(
        db.pool_with(
            "claims-isolation",
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
    store
}

/// A park waits for the commit that is adding its watch and then sees it, whatever the database
/// defaults to. In a `REPEATABLE READ` transaction the park's first read would fix its snapshot
/// before the wait, and the row would stay parked behind a committed watch.
#[tokio::test]
async fn a_park_sees_the_watch_it_waited_for_whatever_the_default_isolation() {
    let db = db_or_skip!();
    let store = repeatable_read_store(&db).await;
    let key = a_key();
    let lease = a_claimed_report(&store, &key).await;
    let (thread, _) = store
        .create_thread(a_thread(), commit(Vec::new(), Vec::new()))
        .await
        .unwrap();

    // the commit, as far as it has got: it holds the key and has inserted the watch
    let pool = db.pool("claims-test", 4).await;
    let mut adding = pool.begin().await.unwrap();
    query(LOCK_WATCH)
        .bind(lock_name(&key))
        .execute(&mut *adding)
        .await
        .unwrap();
    query("INSERT INTO watches (key, thread_id, created_at) VALUES ($1, $2, now())")
        .bind(key.as_str())
        .bind(thread.id.0)
        .execute(&mut *adding)
        .await
        .unwrap();
    let parker = {
        let (store, lease) = (store.clone(), lease.clone());
        tokio::spawn(async move { store.park_inbox(&lease, t0()).await })
    };
    until_blocked_on(&pool, &key).await;
    adding.commit().await.unwrap();

    let parking = tokio::time::timeout(BOUND, parker).await.unwrap().unwrap();
    assert_eq!(parking.unwrap(), Parking::Rearmed);
    let row = store.get_inbox(lease.id).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);
}

/// The other way round, through the opening of a thread, whose first commit the port lets arm
/// watches: it waits for a park that holds the key and then re-arms the row that park set aside,
/// whatever the database defaults to. The opening reads before it reaches the key, so under
/// `REPEATABLE READ` its snapshot would be from before the wait and would not hold the parked row.
#[tokio::test]
async fn opening_a_thread_re_arms_the_row_a_park_set_aside_whatever_the_default_isolation() {
    let db = db_or_skip!();
    let store = repeatable_read_store(&db).await;
    let key = a_key();
    let lease = a_claimed_report(&store, &key).await;

    // the park, as far as it has got: it holds the key and has set the row aside
    let pool = db.pool("claims-test", 4).await;
    let mut parking = pool.begin().await.unwrap();
    query(LOCK_WATCH)
        .bind(lock_name(&key))
        .execute(&mut *parking)
        .await
        .unwrap();
    query(
        "UPDATE inbox SET status = 'parked', parked_at = now(), lease_owner = NULL, \
         lease_until = NULL WHERE id = $1",
    )
    .bind(lease.id.0)
    .execute(&mut *parking)
    .await
    .unwrap();
    let opener = {
        let (store, key) = (store.clone(), key.clone());
        tokio::spawn(async move {
            store
                .create_thread(a_thread(), commit(Vec::new(), vec![key]))
                .await
        })
    };
    until_blocked_on(&pool, &key).await;
    parking.commit().await.unwrap();

    tokio::time::timeout(BOUND, opener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let row = store.get_inbox(lease.id).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);
}

/// `claim_outbox` gives its rows oldest first from the statement itself. The two oldest rows are
/// rewritten first, so that they lie last in the table, and their ids run against their age.
#[tokio::test]
async fn both_outbox_claims_come_back_oldest_first_wherever_the_rows_lie() {
    let db = db_or_skip!();
    let store = db.store().await;
    for order in [ClaimOrder::Fifo, ClaimOrder::Fair] {
        // falling ids, rising age
        let ids: Vec<Uuid> = {
            let mut ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
            ids.reverse();
            ids
        };
        for id in &ids {
            store
                .create_thread(a_thread(), commit(vec![delegate(*id)], Vec::new()))
                .await
                .unwrap();
        }
        for id in &ids[..3] {
            query("UPDATE outbox SET updated_at = now() WHERE id = $1")
                .bind(*id)
                .execute(store.pool())
                .await
                .unwrap();
        }
        let claimed = store
            .claim_outbox("w", t0(), LEASE, 6, order)
            .await
            .unwrap();
        let claimed: Vec<Uuid> = claimed.iter().map(|r| r.id.0).collect();
        assert_eq!(claimed, ids, "{order:?}");
        // out of the next order's way
        query("UPDATE outbox SET status = 'delivered' WHERE id = ANY($1)")
            .bind(ids)
            .execute(store.pool())
            .await
            .unwrap();
    }
}

/// `claim_inbox` gives its rows earliest first, then by id, from the statement itself: the rows
/// are written latest first, and their ids run against the order they are due in.
#[tokio::test]
async fn the_inbox_claim_comes_back_earliest_first_then_by_id() {
    let db = db_or_skip!();
    let store = db.store().await;
    let mut ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
    ids.sort_unstable();
    // written in this order: (id, due). The two due at 10 are written greater id first.
    let rows = [
        (ids[0], 50),
        (ids[1], 40),
        (ids[2], 30),
        (ids[3], 20),
        (ids[5], 10),
        (ids[4], 10),
    ];
    for (id, due) in rows {
        receive(&store, id, &a_key(), at(due)).await;
    }
    let claimed = store.claim_inbox("w", at(60), LEASE, 6).await.unwrap();
    let claimed: Vec<Uuid> = claimed.iter().map(|r| r.id.0).collect();
    assert_eq!(claimed, [ids[4], ids[5], ids[3], ids[2], ids[1], ids[0]]);
}

/// The planner puts the expression of `orch_outbox_due_at` in place of the call. Were it not
/// inlined (a `SET`, `STRICT` or `SECURITY DEFINER` on a later version), `CLAIM_FIFO`, which names
/// no status in its due rows' `WHERE` beyond the call, could not use the partial indexes of the open
/// rows for them and would read the whole outbox, which keeps every row.
#[tokio::test]
async fn the_due_function_is_inlined_into_the_statement() {
    let db = db_or_skip!();
    let store = db.store().await;
    let plan: Vec<String> = query_scalar(
        "EXPLAIN (VERBOSE, COSTS OFF) SELECT o.id FROM outbox o \
         WHERE orch_outbox_due_at(o.status, o.next_attempt_at, o.lease_until, now())",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    let plan = plan.join("\n");
    assert!(plan.contains("next_attempt_at"), "{plan}");
    assert!(!plan.contains("orch_outbox_due_at"), "{plan}");
}

/// The partial indexes of the open rows still serve the claims' due predicate. Inlined is not
/// enough: the planner has to prove `status IN ('pending', 'inflight')` from the expression the
/// function leaves behind, and `CLAIM_FIFO` says nothing else about the status. A body of another
/// shape (a `CASE` on the status) is inlined too and proves nothing, so the claim would read the
/// whole outbox. With sequential scans forbidden, in one session, a plan names an `outbox_open…`
/// index only if the predicate implies the index's own; the `CASE` is the control that shows this
/// assertion can fail. The predicate is the claims' without their `ORDER BY`, which would offer
/// the planner an index of every row, and without the per-thread rule, whose subquery names an
/// open index whatever the due predicate is.
#[tokio::test]
async fn the_due_predicate_uses_a_partial_index_of_the_open_rows() {
    let db = db_or_skip!();
    let store = db.store().await;
    let mut tx = store.pool().begin().await.unwrap();
    query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let due: Vec<String> = query_scalar(
        "EXPLAIN (COSTS OFF) SELECT o.id FROM outbox o \
         WHERE orch_outbox_due_at(o.status, o.next_attempt_at, o.lease_until, now())",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let due = due.join("\n");
    assert!(due.contains("outbox_open"), "{due}");
    let by_case: Vec<String> = query_scalar(
        "EXPLAIN (COSTS OFF) SELECT o.id FROM outbox o \
         WHERE CASE o.status WHEN 'pending' THEN o.next_attempt_at <= now() \
                             WHEN 'inflight' THEN o.lease_until <= now() ELSE false END",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let by_case = by_case.join("\n");
    assert!(by_case.contains("Seq Scan on outbox"), "{by_case}");
    assert!(!by_case.contains("outbox_open"), "{by_case}");
    tx.rollback().await.unwrap();
}

/// The keys are locked, and the held ones told, once each in bytewise order whatever the database
/// collates by: the order earlier builds sorted them in.
#[tokio::test]
async fn watch_keys_are_taken_once_each_in_bytewise_order() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (first, _) = store
        .create_thread(a_thread(), commit(Vec::new(), Vec::new()))
        .await
        .unwrap();
    let (second, _) = store
        .create_thread(a_thread(), commit(Vec::new(), Vec::new()))
        .await
        .unwrap();
    let run = Uuid::now_v7().simple().to_string();
    // bytewise, `B` comes before `a`; a language's collation has it the other way round
    let keys = vec![format!("{run}:a"), format!("{run}:B"), format!("{run}:a")];
    let arm = "SELECT array_to_string(held_keys, ','), rearmed FROM orch_watch_arm($1, $2, now())";
    let answer = async |thread: Uuid| -> (String, bool) {
        query_as(arm)
            .bind(thread)
            .bind(keys.clone())
            .fetch_one(store.pool())
            .await
            .unwrap()
    };
    assert_eq!(answer(first.id.0).await, (String::new(), false), "new keys");

    // Its own keys again, with a report parked behind one of them: nothing is held by another
    // thread (every insert conflicts, so this is where the holder is compared), and the row is
    // re-armed all the same.
    let report = Uuid::now_v7();
    receive(&store, report, &a_key(), t0()).await;
    query("UPDATE inbox SET status = 'parked', parked_at = now(), correlation = $2 WHERE id = $1")
        .bind(report)
        .bind(format!("{run}:a"))
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(
        answer(first.id.0).await,
        (String::new(), true),
        "its own keys"
    );
    let row = store.get_inbox(InboxId(report)).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);

    assert_eq!(
        answer(second.id.0).await,
        (format!("{run}:B,{run}:a"), false),
        "another thread's keys"
    );
}

/// A caller's bug is `OR002`, before anything is written.
#[tokio::test]
async fn the_functions_refuse_missing_arguments() {
    let db = db_or_skip!();
    let store = db.store().await;
    for call in [
        "SELECT * FROM orch_watch_arm(NULL, '{k}', now())",
        "SELECT * FROM orch_watch_arm(gen_random_uuid(), NULL, now())",
        "SELECT * FROM orch_watch_arm(gen_random_uuid(), ARRAY['k', NULL], now())",
        "SELECT * FROM orch_watch_arm(gen_random_uuid(), '{k}', NULL)",
        "SELECT * FROM orch_inbox_park(NULL, 'w', 1, now())",
        "SELECT * FROM orch_inbox_park(gen_random_uuid(), NULL, 1, now())",
        "SELECT * FROM orch_inbox_park(gen_random_uuid(), 'w', NULL, now())",
        "SELECT * FROM orch_inbox_park(gen_random_uuid(), 'w', 1, NULL)",
    ] {
        let err = query(call).fetch_all(store.pool()).await.unwrap_err();
        assert_eq!(err.code(), Some("OR002"), "{call}: {err:?}");
    }
}

/// `mark_sent` writes the outbox row and the binding in one statement (ADR 0107 row 38): when the
/// binding cannot be written, the row is not marked either. And a row that is not `inflight` has no
/// claim, even one that still names the owner and the attempt (every way the store ends a claim also
/// clears the owner, so the row is made by hand: only the status can refuse it).
#[tokio::test]
async fn mark_sent_writes_the_row_and_the_binding_together_or_neither() {
    let db = db_or_skip!();
    let store = db.store().await;
    let id = Uuid::now_v7();
    let (thread, _) = store
        .create_thread(a_thread(), commit(vec![delegate(id)], Vec::new()))
        .await
        .unwrap();
    let claimed = store
        .claim_outbox("w", t0(), LEASE, 1, ClaimOrder::Fifo)
        .await
        .unwrap();
    let lease = claimed[0].lease().unwrap();
    let update = BindingUpdate {
        context_id: Some("agent-ctx".into()),
        task_id: Some("task-1".into()),
        ..BindingUpdate::default()
    };
    let conn = support::connect_with(&db.options()).await;
    raw_sql(
        "ALTER TABLE a2a_bindings ADD CONSTRAINT no_task CHECK (task_id IS NULL)",
        &conn,
    )
    .await
    .unwrap();
    store
        .mark_sent(&lease, update.clone(), at(1))
        .await
        .unwrap_err();
    let row = store.get_outbox(OutboxId(id)).await.unwrap().unwrap();
    assert_eq!(
        row.sent_at, None,
        "the row is not marked without its binding"
    );

    raw_sql("ALTER TABLE a2a_bindings DROP CONSTRAINT no_task", &conn)
        .await
        .unwrap();
    assert!(store.mark_sent(&lease, update, at(2)).await.unwrap());
    let row = store.get_outbox(OutboxId(id)).await.unwrap().unwrap();
    assert_eq!(row.sent_at, Some(at(2)));
    let binding = store.get_binding(thread.id).await.unwrap().unwrap();
    assert_eq!(binding.task_id.as_deref(), Some("task-1"));
    assert_eq!(binding.context_id.as_deref(), Some("agent-ctx"));

    query("UPDATE outbox SET status = 'delivered' WHERE id = $1")
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();
    let late = BindingUpdate {
        task_id: Some("late-task".into()),
        ..BindingUpdate::default()
    };
    assert!(!store.mark_sent(&lease, late, at(4)).await.unwrap());
    let row = store.get_outbox(OutboxId(id)).await.unwrap().unwrap();
    assert_eq!(row.sent_at, Some(at(2)));
    assert_eq!(
        store.get_binding(thread.id).await.unwrap().unwrap(),
        binding
    );
}
