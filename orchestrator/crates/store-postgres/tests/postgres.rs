//! Behaviour specific to the Postgres implementation. Skipped unless `ORCH_TEST_DATABASE_URL`
//! is set; each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, Classify, ErrorClass, EventBody, ThreadId, ThreadState, UserId,
    UserMessageData,
};
use orch_ports::{
    Commit, CommitOutcome, NewEvent, NewOutbox, NewThreadRecord, OutboxId, OutboxPayload,
    StoreError, ThreadStore, Topic, Wakeup,
};
use orch_store_postgres::{PgStore, PgWakeup};
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

const T0: &str = "2026-01-01T00:00:00Z";

fn t0() -> Timestamp {
    T0.parse().unwrap()
}

fn user() -> UserId {
    UserId::new("alice@example.com")
}

fn event(text: &str, key: Option<String>) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(&user()),
        body: EventBody::UserMessage(UserMessageData { text: text.into() }),
        idempotency_key: key,
    }
}

fn commit(state: ThreadState, events: Vec<NewEvent>, outbox: Vec<NewOutbox>) -> Commit {
    Commit {
        new_state: state,
        events,
        outbox,
        binding: None,
        now: t0(),
    }
}

fn delegate() -> NewOutbox {
    NewOutbox {
        id: OutboxId(Uuid::now_v7()),
        payload: OutboxPayload::Delegate {
            text: "go".into(),
            release: None,
        },
    }
}

async fn create(store: &PgStore, outbox: Vec<NewOutbox>) -> ThreadId {
    let id = ThreadId(Uuid::now_v7());
    store
        .create_thread(
            NewThreadRecord {
                id,
                owner: user(),
                title: "t".into(),
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: Some("stable".into()),
                },
                context_id: format!("ctx-{id}"),
                now: t0(),
            },
            commit(ThreadState::Queued, vec![event("hi", None)], outbox),
        )
        .await
        .unwrap();
    id
}

async fn expect(sub: &mut (impl futures::Stream<Item = Topic> + Unpin), want: Topic) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(got) = sub.next().await {
            if got == want {
                return;
            }
        }
        panic!("subscription ended");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {want:?}"));
}

async fn expect_silence(sub: &mut (impl futures::Stream<Item = Topic> + Unpin), unwanted: Topic) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while let Ok(Some(got)) = tokio::time::timeout_at(deadline, sub.next()).await {
        assert_ne!(
            got, unwanted,
            "received a notification that must not be sent"
        );
    }
}

#[tokio::test]
async fn sixteen_concurrent_appenders_get_contiguous_seq() {
    let db = db_or_skip!();
    let store = Arc::new(db.store().await);
    let id = create(&store, vec![]).await;
    const TASKS: usize = 16;
    const ROUNDS: usize = 5;
    const PER_COMMIT: usize = 3;

    let mut tasks = Vec::new();
    for task in 0..TASKS {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            for round in 0..ROUNDS {
                loop {
                    let thread = store.get_thread(None, id).await.unwrap().unwrap();
                    let events = (0..PER_COMMIT)
                        .map(|i| {
                            event(
                                &format!("{task}-{round}-{i}"),
                                Some(format!("k-{task}-{round}-{i}")),
                            )
                        })
                        .collect();
                    let c = commit(ThreadState::Working, events, vec![]);
                    match store.commit(id, thread.version, c).await {
                        Ok(CommitOutcome::Applied { events, .. }) => {
                            let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
                            assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
                            break;
                        }
                        Ok(CommitOutcome::Duplicate) => panic!("distinct keys collided"),
                        Err(StoreError::VersionConflict) => tokio::task::yield_now().await,
                        Err(e) => panic!("{e}"),
                    }
                }
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    let total = 1 + TASKS * ROUNDS * PER_COMMIT;
    let events = store.list_events(id, 0, 10_000).await.unwrap();
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(
        seqs,
        (1..=i64::try_from(total).unwrap()).collect::<Vec<_>>(),
        "seq must be contiguous, without gaps or duplicates"
    );
    let thread = store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(thread.last_seq, i64::try_from(total).unwrap());
    assert_eq!(thread.version, i64::try_from(1 + TASKS * ROUNDS).unwrap());
    let mut texts: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::UserMessage(m) => Some(m.text.clone()),
            _ => None,
        })
        .collect();
    texts.sort();
    texts.dedup();
    assert_eq!(texts.len(), total, "no event written twice");
}

#[tokio::test]
async fn migrate_twice_is_a_no_op() {
    let db = db_or_skip!();
    let store = PgStore::from_pool(db.pool("orch-test", 4).await);
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    let (applied,): (i64,) = sqlx::query_as("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(applied, 1);
    // Data survives a re-run.
    let id = create(&store, vec![delegate()]).await;
    store.migrate().await.unwrap();
    assert!(store.get_thread(None, id).await.unwrap().is_some());
}

#[tokio::test]
async fn migrate_from_many_replicas_at_once() {
    let db = db_or_skip!();
    // Eight "replicas": separate pools on the same, still empty, schema.
    let mut tasks = Vec::new();
    for replica in 0..8 {
        let pool = db.pool(&format!("orch-test-replica-{replica}"), 2).await;
        tasks.push(tokio::spawn(async move {
            PgStore::from_pool(pool).migrate().await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let store = PgStore::from_pool(db.pool("orch-test", 2).await);
    let (applied,): (i64,) = sqlx::query_as("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(applied, 1);
    store.ping().await.unwrap();
    create(&store, vec![]).await;
}

#[tokio::test]
async fn commit_in_one_process_wakes_a_subscriber_in_another() {
    let db = db_or_skip!();
    let listener_pool = db.pool("orch-test-listener", 3).await;
    let wakeup = PgWakeup::start(listener_pool);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe();

    // A second pool, i.e. another replica.
    let store = db.store().await;
    let id = create(&store, vec![delegate()]).await;
    expect(&mut sub, Topic::Thread(id)).await;
    expect(&mut sub, Topic::Outbox).await;

    let thread = store.get_thread(None, id).await.unwrap().unwrap();
    store
        .commit(
            id,
            thread.version,
            commit(ThreadState::Working, vec![], vec![]),
        )
        .await
        .unwrap();
    expect(&mut sub, Topic::Thread(id)).await;

    // An explicit notify from yet another connection arrives too.
    let other = PgWakeup::start(db.pool("orch-test-other", 3).await);
    other.notify(Topic::Outbox).await.unwrap();
    expect(&mut sub, Topic::Outbox).await;
}

#[tokio::test]
async fn a_failed_commit_notifies_nobody() {
    let db = db_or_skip!();
    let wakeup = PgWakeup::start(db.pool("orch-test-listener", 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let store = db.store().await;
    let id = create(&store, vec![]).await;
    let mut sub = wakeup.subscribe();
    // A successful commit does notify (and gives us a key to collide with).
    let ok = store
        .commit(
            id,
            1,
            commit(
                ThreadState::Working,
                vec![event("a", Some("k".into()))],
                vec![],
            ),
        )
        .await
        .unwrap();
    assert!(matches!(ok, CommitOutcome::Applied { .. }));
    expect(&mut sub, Topic::Thread(id)).await;
    // Let the notifications of `create` and of the commit above drain.
    while tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_ok()
    {}

    let conflict = store
        .commit(id, 99, commit(ThreadState::Done, vec![], vec![delegate()]))
        .await;
    assert!(matches!(conflict, Err(StoreError::VersionConflict)));
    let dup = store
        .commit(
            id,
            2,
            commit(
                ThreadState::Done,
                vec![event("b", Some("k".into()))],
                vec![delegate()],
            ),
        )
        .await
        .unwrap();
    assert_eq!(dup, CommitOutcome::Duplicate);
    // NOTIFY is database-wide, so other tests' `Outbox` hints may show up; this thread's id
    // is unique to this test.
    expect_silence(&mut sub, Topic::Thread(id)).await;
}

#[tokio::test]
async fn listener_reconnect_emits_resync_and_keeps_working() {
    let db = db_or_skip!();
    let app = format!("orch-test-listener-{}", Uuid::now_v7().simple());
    let admin = db.pool("orch-test-admin", 2).await;
    let wakeup = PgWakeup::start(db.pool(&app, 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe();

    // Kill the listener's backend, as a failover or a network cut would.
    let (killed,): (i64,) = sqlx::query_as(
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity \
         WHERE application_name = $1 AND pid <> pg_backend_pid()",
    )
    .bind(&app)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(killed >= 1);
    expect(&mut sub, Topic::Resync).await;

    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let id = ThreadId(Uuid::now_v7());
    wakeup.notify(Topic::Thread(id)).await.unwrap();
    expect(&mut sub, Topic::Thread(id)).await;
}

#[tokio::test]
async fn slow_subscriber_gets_resync() {
    let db = db_or_skip!();
    let wakeup = PgWakeup::start(db.pool("orch-test-listener", 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe();
    for _ in 0..3000 {
        wakeup.notify(Topic::Outbox).await.unwrap();
    }
    // The subscriber never read while 3000 hints arrived (capacity 1024): it must be told.
    tokio::time::sleep(Duration::from_millis(500)).await;
    expect(&mut sub, Topic::Resync).await;
}

#[tokio::test]
async fn timestamps_round_trip_at_microsecond_precision() {
    let db = db_or_skip!();
    let store = db.store().await;
    let precise: Timestamp = "2026-03-04T05:06:07.123456789Z".parse().unwrap();
    let id = ThreadId(Uuid::now_v7());
    let mut first = commit(
        ThreadState::Queued,
        vec![NewEvent {
            at: precise,
            ..event("x", None)
        }],
        vec![],
    );
    first.now = precise;
    let (record, events) = store
        .create_thread(
            NewThreadRecord {
                id,
                owner: user(),
                title: "t".into(),
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: "c".into(),
                now: precise,
            },
            first,
        )
        .await
        .unwrap();
    let rounded: Timestamp = "2026-03-04T05:06:07.123457Z".parse().unwrap();
    assert_eq!(record.created_at, rounded);
    assert_eq!(events[0].at, rounded);
    assert_eq!(store.get_thread(None, id).await.unwrap().unwrap(), record);
    assert_eq!(store.list_events(id, 0, 10).await.unwrap(), events);
}

#[tokio::test]
async fn creating_the_same_thread_twice_is_refused_and_writes_nothing() {
    let db = db_or_skip!();
    let store = db.store().await;
    let id = create(&store, vec![]).await;
    let again = store
        .create_thread(
            NewThreadRecord {
                id,
                owner: user(),
                title: "other".into(),
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: "c".into(),
                now: t0(),
            },
            commit(
                ThreadState::Queued,
                vec![event("dup", None)],
                vec![delegate()],
            ),
        )
        .await;
    assert!(
        matches!(again, Err(StoreError::Corrupt { .. })),
        "{again:?}"
    );
    assert_eq!(store.list_events(id, 0, 10).await.unwrap().len(), 1);
    assert!(store.list_open_outbox(id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rejected_statement_is_internal_and_never_retried() {
    let db = db_or_skip!();
    let store = db.store().await;
    let id = create(&store, vec![]).await;
    // The deployment lost a table: SQLSTATE 42P01, which waiting does not cure.
    sqlx::query("DROP TABLE events CASCADE")
        .execute(store.pool())
        .await
        .unwrap();
    let err = store.list_events(id, 0, 10).await.unwrap_err();
    assert!(matches!(err, StoreError::Internal { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Internal);
    assert!(!err.is_retryable());
    let source = std::error::Error::source(&err).unwrap();
    assert!(source.downcast_ref::<sqlx::Error>().is_some());
}

#[tokio::test]
async fn a_closed_pool_is_unavailable_and_keeps_the_driver_error() {
    let db = db_or_skip!();
    let store = db.store().await;
    store.pool().close().await;
    let err = store.ping().await.unwrap_err();
    assert!(matches!(err, StoreError::Unavailable { .. }), "{err:?}");
    assert!(err.is_retryable());
    let source = std::error::Error::source(&err).unwrap();
    assert!(source.downcast_ref::<sqlx::Error>().is_some());
}

#[tokio::test]
async fn a_row_that_does_not_parse_is_corrupt() {
    let db = db_or_skip!();
    let store = db.store().await;
    let id = create(&store, vec![]).await;
    // A poisoned row: written by hand, valid for the schema and not for this code (a user
    // message without its text).
    sqlx::query(
        "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
         VALUES ($1, 2, now(), 'user_message', '{}', '{\"unexpected\": true}')",
    )
    .bind(id.0)
    .execute(store.pool())
    .await
    .unwrap();
    let err = store.list_events(id, 0, 10).await.unwrap_err();
    assert!(matches!(err, StoreError::Corrupt { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Corrupt);
    assert!(!err.is_retryable(), "a poisoned row is not retried forever");
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn an_unreachable_database_is_unavailable_without_a_database() {
    // Nothing listens on port 1; the pool gives up quickly and the store reports it as a
    // transient failure whose source is the driver's error.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(300))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .unwrap();
    let store = PgStore::from_pool(pool);
    let err = store.ping().await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err:?}");
    assert!(std::error::Error::source(&err).is_some());
}
