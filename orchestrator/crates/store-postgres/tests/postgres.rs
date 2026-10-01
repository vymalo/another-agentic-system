//! Behaviour specific to the Postgres implementation. Skipped unless `ORCH_TEST_DATABASE_URL`
//! is set; each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, CiConclusion, CiProvider, CiReport, Classify, ErrorClass,
    EventBody, LiveChunk, LiveEnd, LiveText, ThreadId, ThreadState, UserId, UserMessageData,
    WatchKey,
};
use orch_ports::{
    BindingUpdate, Commit, CommitOutcome, InboxId, InboxPayload, InboxStatus, NewEvent, NewInbox,
    NewOutbox, NewThreadRecord, OutboxId, OutboxPayload, Parking, Received, StoreError,
    ThreadStore, Topic, Wakeup,
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
        body: EventBody::UserMessage(UserMessageData::new(text)),
        idempotency_key: key,
    }
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
    }
}

fn delegate() -> NewOutbox {
    NewOutbox {
        id: OutboxId(Uuid::now_v7()),
        payload: OutboxPayload::Delegate {
            text: "go".into(),
            release: None,
            new_job: false,
            ui_catalog: None,
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
                        Ok(CommitOutcome::Fenced) => panic!("no lease was given"),
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

/// How many migrations the crate embeds: each is recorded once, however often `migrate` runs.
fn embedded_migrations() -> i64 {
    i64::try_from(sqlx::migrate!("./migrations").iter().count()).unwrap()
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
    assert_eq!(applied, embedded_migrations());
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
    assert_eq!(applied, embedded_migrations());
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

fn live_piece(thread: ThreadId, offset: u64, text: &str, end: LiveEnd) -> LiveText {
    LiveText {
        thread,
        agent: AgentId::new("coder"),
        chunk: LiveChunk {
            message_id: "S".into(),
            offset,
            text: text.into(),
            end,
        },
    }
}

/// The next piece of `thread` (the channel is the database's, so another test's pieces may pass).
async fn next_live_of(
    sub: &mut (impl futures::Stream<Item = LiveText> + Unpin),
    thread: ThreadId,
) -> LiveText {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let got = sub.next().await.expect("live subscription ended");
            if got.thread == thread {
                return got;
            }
        }
    })
    .await
    .expect("timed out waiting for live text")
}

#[tokio::test]
async fn live_text_published_in_one_process_reaches_a_subscriber_in_another() {
    let db = db_or_skip!();
    let listener = PgWakeup::start(db.pool("orch-test-listener", 3).await);
    assert!(listener.wait_listening(Duration::from_secs(10)).await);
    let mut sub = listener.subscribe_live();
    let mut topics = listener.subscribe();

    // Another replica: its own pool and connections.
    let other_pool = db.pool("orch-test-other", 3).await;
    let other = PgWakeup::start(other_pool.clone());
    let thread = ThreadId(Uuid::now_v7());

    // A stray payload on the channel (another version, a hand-typed NOTIFY) is not live text and
    // does not disturb what follows.
    sqlx::query("SELECT pg_notify('orch_live', 'not json')")
        .execute(&other_pool)
        .await
        .unwrap();
    other
        .publish_live(live_piece(thread, 0, "Fib", LiveEnd::Open))
        .await
        .unwrap();
    other
        .publish_live(live_piece(thread, 3, "onacci", LiveEnd::Last))
        .await
        .unwrap();
    assert_eq!(
        next_live_of(&mut sub, thread).await,
        live_piece(thread, 0, "Fib", LiveEnd::Open)
    );
    assert_eq!(
        next_live_of(&mut sub, thread).await,
        live_piece(thread, 3, "onacci", LiveEnd::Last)
    );
    // Live text is not a hint: nobody was told to re-read anything.
    expect_silence(&mut topics, Topic::Resync).await;
}

#[tokio::test]
async fn live_text_works_again_after_a_listener_reconnect() {
    let db = db_or_skip!();
    let app = format!("orch-test-listener-{}", Uuid::now_v7().simple());
    let admin = db.pool("orch-test-admin", 2).await;
    let wakeup = PgWakeup::start(db.pool(&app, 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe_live();
    let mut topics = wakeup.subscribe();

    sqlx::query(
        "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity \
         WHERE application_name = $1 AND pid <> pg_backend_pid()",
    )
    .bind(&app)
    .fetch_one(&admin)
    .await
    .unwrap();
    // The hints are told (they may have been missed); live text is best effort and is not.
    expect(&mut topics, Topic::Resync).await;
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);

    let thread = ThreadId(Uuid::now_v7());
    wakeup
        .publish_live(live_piece(thread, 0, "again", LiveEnd::Open))
        .await
        .unwrap();
    assert_eq!(
        next_live_of(&mut sub, thread).await,
        live_piece(thread, 0, "again", LiveEnd::Open)
    );
}

#[tokio::test]
async fn a_slow_live_subscriber_loses_pieces_without_a_word_and_keeps_listening() {
    let db = db_or_skip!();
    let wakeup = PgWakeup::start(db.pool("orch-test-listener", 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe_live();
    let thread = ThreadId(Uuid::now_v7());
    for i in 0..2000u64 {
        wakeup
            .publish_live(live_piece(thread, i, "x", LiveEnd::Open))
            .await
            .unwrap();
    }
    // Nobody read while 2000 pieces arrived (capacity 1024): the oldest are gone, the stream is
    // not, and the newest is there.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let first = next_live_of(&mut sub, thread).await;
    assert!(first.chunk.offset > 0, "the oldest pieces were dropped");
    let mut last = first;
    while last.chunk.offset < 1999 {
        last = next_live_of(&mut sub, thread).await;
    }
    wakeup
        .publish_live(live_piece(thread, 2000, "!", LiveEnd::Last))
        .await
        .unwrap();
    assert_eq!(next_live_of(&mut sub, thread).await.chunk.offset, 2000);
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

/// The fence takes a share lock on the outbox row after the thread lock. Racing it against a
/// re-claim and a `mark_sent` (which lock the outbox row first) must neither deadlock nor
/// leave a half-written commit: every round ends with exactly the events of the commits that
/// were `Applied`.
#[tokio::test]
async fn commits_racing_a_reclaim_neither_deadlock_nor_half_write() {
    let db = db_or_skip!();
    let store = Arc::new(db.store().await);
    let lease_for = Duration::from_secs(30);
    let later = t0() + Duration::from_secs(31);
    for _ in 0..25 {
        let id = create(&store, vec![delegate()]).await;
        let held = store
            .claim_outbox("a", t0(), lease_for, 10)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.thread_id == id)
            .unwrap();
        let lease = held.lease().unwrap();
        let committer = {
            let (store, lease) = (Arc::clone(&store), lease.clone());
            tokio::spawn(async move {
                let mut c = commit(ThreadState::Working, vec![event("late", None)], vec![]);
                c.lease = Some(lease);
                c.binding = Some(BindingUpdate {
                    task_id: Some("t".into()),
                    ..BindingUpdate::default()
                });
                store.commit(id, 1, c).await
            })
        };
        let marker = {
            let (store, lease) = (Arc::clone(&store), lease.clone());
            tokio::spawn(async move {
                store
                    .mark_sent(&lease, BindingUpdate::default(), t0())
                    .await
            })
        };
        let claimer = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.claim_outbox("b", later, lease_for, 100).await })
        };
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            (
                committer.await.unwrap().unwrap(),
                marker.await.unwrap().unwrap(),
                claimer.await.unwrap().unwrap(),
            )
        })
        .await
        .expect("a deadlock");
        let (committed, _, _) = outcome;
        let applied = matches!(committed, CommitOutcome::Applied { .. });
        assert!(applied || committed == CommitOutcome::Fenced);
        let events = store.list_events(id, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1 + usize::from(applied));
        let thread = store.get_thread(None, id).await.unwrap().unwrap();
        assert_eq!(thread.version, 1 + i64::from(applied));
        // Whatever the interleaving, the row ends up claimable by its next owner.
        store
            .claim_outbox("b", later, lease_for, 100)
            .await
            .unwrap();
        let row = store.get_outbox(held.id).await.unwrap().unwrap();
        assert_eq!(row.lease_owner.as_deref(), Some("b"));
        assert_eq!(row.attempts, 2);
    }
}

/// Migration 0003 on a database that has run 0001 and 0002 and holds a thread: the old row
/// reads back as a job with no gate, and the widened constraints take the new values.
#[tokio::test]
async fn migration_0003_upgrades_a_database_that_holds_threads() {
    let db = db_or_skip!();
    let pool = db.pool("orch-test-upgrade", 4).await;
    // Bring the schema to 0002 the way an older release did.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("0001_init.sql"),
        include_str!("../migrations/0001_init.sql"),
    )
    .unwrap();
    std::fs::write(
        dir.join("0002_ui_events.sql"),
        include_str!("../migrations/0002_ui_events.sql"),
    )
    .unwrap();
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, created_at, updated_at) \
         VALUES ($1, 'alice@example.com', 't', 'coder', 'working', 1, now(), now())",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    // The old code could not write these; the old constraints refuse them.
    let refused = sqlx::query("UPDATE threads SET state = 'verifying' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(refused.is_err(), "0002 has no `verifying`");

    // The new release starts against it.
    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    let thread = store.get_thread(None, ThreadId(id)).await.unwrap().unwrap();
    assert_eq!(thread.state, ThreadState::Working);
    assert_eq!(thread.job, orch_core::Job::default());
    sqlx::query("UPDATE threads SET state = 'verifying' WHERE id = $1")
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();
    for kind in ["ci_result", "check_result", "rework"] {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, (SELECT coalesce(max(seq), 0) + 1 FROM events WHERE thread_id = $1), \
                     now(), $2, '{}', '{}')",
        )
        .bind(id)
        .bind(kind)
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
    // The outbox takes a `verify` row with the verifier's task id, and still refuses nonsense.
    let insert_outbox = |kind: &'static str| {
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, next_attempt_at, \
             created_at, updated_at, task_id) \
             VALUES ($1, $2, $3, '{}', 'pending', now(), now(), now(), 'task-1')",
        )
        .bind(Uuid::now_v7())
        .bind(id)
        .bind(kind)
        .execute(store.pool())
    };
    insert_outbox("verify").await.unwrap();
    assert!(insert_outbox("nonsense").await.is_err());
}

/// A CI report row whose correlation is the watch of commit `n`.
fn ci_row(n: u8) -> (NewInbox, WatchKey) {
    let sha = format!("{n:02x}").repeat(20);
    let key = WatchKey::ci("github.com/o/r", &sha);
    let row = NewInbox {
        id: InboxId(Uuid::now_v7()),
        source: "github".into(),
        idempotency_key: format!("delivery-{n}"),
        payload: InboxPayload::CiReport(CiReport {
            provider: CiProvider::Github,
            repository: "github.com/o/r".into(),
            sha,
            branch: None,
            name: "build".into(),
            conclusion: CiConclusion::Success,
            url: None,
            summary: None,
        }),
        correlation: Some(key.as_str().to_owned()),
    };
    (row, key)
}

#[tokio::test]
async fn receiving_and_rearming_wake_an_inbox_worker_in_another_process() {
    let db = db_or_skip!();
    let wakeup = PgWakeup::start(db.pool("orch-test-listener", 3).await);
    assert!(wakeup.wait_listening(Duration::from_secs(10)).await);
    let mut sub = wakeup.subscribe();
    let store = db.store().await;

    let (row, key) = ci_row(1);
    let duplicate = NewInbox {
        id: InboxId(Uuid::now_v7()),
        ..row.clone()
    };
    assert!(matches!(
        store.receive(row, t0()).await.unwrap(),
        Received::Stored { .. }
    ));
    expect(&mut sub, Topic::Inbox).await;
    // A worker parks it (no watch yet); a commit that adds the watch re-arms it and says so.
    let got = store
        .claim_inbox("a", t0(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    let lease = got[0].lease().unwrap();
    assert_eq!(
        store.park_inbox(&lease, t0()).await.unwrap(),
        Parking::Parked
    );
    while tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_ok()
    {}
    let id = create(&store, vec![]).await;
    let mut adds = commit(ThreadState::Working, vec![], vec![]);
    adds.watches = vec![key];
    store.commit(id, 1, adds).await.unwrap();
    expect(&mut sub, Topic::Inbox).await;
    // A redelivery is a duplicate and stores nothing.
    assert_eq!(
        store.receive(duplicate, t0()).await.unwrap(),
        Received::Duplicate
    );
}

/// A CI report row whose watch key is unique to the call, so that a test can tell its own
/// advisory lock from the ones other tests (in other schemas of the same database) take.
fn unique_ci_row() -> (NewInbox, WatchKey) {
    let hex = Uuid::now_v7().simple().to_string();
    let sha = format!("{hex}{}", &hex[..8]);
    let key = WatchKey::ci("github.com/o/r", &sha);
    let (mut row, _) = ci_row(0);
    if let InboxPayload::CiReport(report) = &mut row.payload {
        report.sha = sha;
    }
    row.idempotency_key = format!("delivery-{hex}");
    row.correlation = Some(key.as_str().to_owned());
    (row, key)
}

/// The SQL of the store's per-key lock (`lock_watch`), which this test takes by hand to hold the
/// place of one side of the race.
const LOCK_WATCH: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";

fn watch_lock_name(key: &WatchKey) -> String {
    format!("orch:watch:{key}")
}

/// Waits until some backend is blocked on the advisory lock of `key`, straight from
/// `pg_locks`: the proof that a call is waiting where the test wants it, with no sleep standing
/// in for it. (The sleeps below only pace the polling.)
async fn wait_until_blocked_on_watch(pool: &sqlx::PgPool, key: &WatchKey) {
    for _ in 0..500 {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
             AND ((classid::bigint << 32) | objid::bigint) = hashtextextended($1, 0))",
        )
        .bind(watch_lock_name(key))
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

/// The worker looked for a watch, found none, and parks; a commit that adds the watch is in
/// flight, holding the key. The park must wait for it and then see its watch: the row ends
/// `pending`, never `parked` behind a watch that nobody will re-arm it for.
#[tokio::test]
async fn a_park_waits_for_the_commit_that_is_adding_its_watch() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (row, key) = unique_ci_row();
    let inbox = row.id;
    store.receive(row, t0()).await.unwrap();
    let lease = store
        .claim_inbox("a", t0(), Duration::from_secs(30), 10)
        .await
        .unwrap()
        .iter()
        .find(|r| r.id == inbox)
        .and_then(orch_ports::InboxItem::lease)
        .unwrap();
    let thread = create(&store, vec![]).await;

    // The commit, as far as it has got: it holds the key and has inserted the watch.
    let mut commit_tx = store.pool().begin().await.unwrap();
    sqlx::query(LOCK_WATCH)
        .bind(watch_lock_name(&key))
        .execute(&mut *commit_tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO watches (key, thread_id, created_at) VALUES ($1, $2, now())")
        .bind(key.as_str())
        .bind(thread.0)
        .execute(&mut *commit_tx)
        .await
        .unwrap();

    let parker = {
        let (store, lease) = (store.clone(), lease.clone());
        tokio::spawn(async move { store.park_inbox(&lease, t0()).await })
    };
    wait_until_blocked_on_watch(store.pool(), &key).await;
    assert!(!parker.is_finished(), "the park is waiting for the key");
    assert_eq!(
        store.get_inbox(inbox).await.unwrap().unwrap().status,
        InboxStatus::Inflight,
        "and has written nothing"
    );

    commit_tx.commit().await.unwrap();
    let parking = tokio::time::timeout(Duration::from_secs(10), parker)
        .await
        .expect("the park never finished")
        .unwrap()
        .unwrap();
    assert_eq!(parking, Parking::Rearmed);
    let row = store.get_inbox(inbox).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);
    assert!(row.parked_at.is_none());
}

/// The other order: the worker's park holds the key and has set the row aside, and the commit
/// that adds the watch arrives. It must wait for the park and then find the row parked, and
/// re-arm it.
#[tokio::test]
async fn a_commit_adding_a_watch_waits_for_a_park_and_re_arms_its_row() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (row, key) = unique_ci_row();
    let inbox = row.id;
    store.receive(row, t0()).await.unwrap();
    store
        .claim_inbox("a", t0(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    let thread = create(&store, vec![]).await;

    // The park, as far as it has got: it holds the key and has parked the row.
    let mut park_tx = store.pool().begin().await.unwrap();
    sqlx::query(LOCK_WATCH)
        .bind(watch_lock_name(&key))
        .execute(&mut *park_tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE inbox SET status = 'parked', parked_at = now(), lease_owner = NULL, \
         lease_until = NULL WHERE id = $1",
    )
    .bind(inbox.0)
    .execute(&mut *park_tx)
    .await
    .unwrap();

    let committer = {
        let (store, key) = (store.clone(), key.clone());
        tokio::spawn(async move {
            let mut adds = commit(ThreadState::Working, vec![], vec![]);
            adds.watches = vec![key];
            store.commit(thread, 1, adds).await
        })
    };
    wait_until_blocked_on_watch(store.pool(), &key).await;
    assert!(
        !committer.is_finished(),
        "the commit is waiting for the key"
    );

    park_tx.commit().await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), committer)
        .await
        .expect("the commit never finished")
        .unwrap()
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Applied { .. }));
    let row = store.get_inbox(inbox).await.unwrap().unwrap();
    assert_eq!(
        row.status,
        InboxStatus::Pending,
        "the row the park set aside is re-armed by the commit that came second"
    );
    assert_eq!(store.get_watch(key.as_str()).await.unwrap(), Some(thread));
}

/// Expiry takes the rows it changes in id order and passes by one that another transaction
/// holds instead of waiting for it, so it cannot be a link in a chain of waits.
#[tokio::test]
async fn expiry_passes_by_a_row_somebody_holds_instead_of_waiting() {
    let db = db_or_skip!();
    let store = db.store().await;
    let mut ids = Vec::new();
    for n in 1..=2_u8 {
        let (row, _) = ci_row(n);
        ids.push(row.id);
        store.receive(row, t0()).await.unwrap();
    }
    for lease in store
        .claim_inbox("a", t0(), Duration::from_secs(30), 10)
        .await
        .unwrap()
        .iter()
        .filter_map(orch_ports::InboxItem::lease)
    {
        assert_eq!(
            store.park_inbox(&lease, t0()).await.unwrap(),
            Parking::Parked
        );
    }

    let mut holder = store.pool().begin().await.unwrap();
    sqlx::query("SELECT 1 FROM inbox WHERE id = $1 FOR UPDATE")
        .bind(ids[0].0)
        .execute(&mut *holder)
        .await
        .unwrap();
    let expired = tokio::time::timeout(
        Duration::from_secs(10),
        store.expire_parked_inbox(t0(), t0()),
    )
    .await
    .expect("expiry waited for a row lock")
    .unwrap();
    assert_eq!(expired, 1, "only the row nobody holds");
    assert_eq!(
        store.get_inbox(ids[0]).await.unwrap().unwrap().status,
        InboxStatus::Parked
    );
    assert_eq!(
        store.get_inbox(ids[1]).await.unwrap().unwrap().status,
        InboxStatus::Expired
    );

    holder.rollback().await.unwrap();
    assert_eq!(store.expire_parked_inbox(t0(), t0()).await.unwrap(), 1);
    assert_eq!(
        store.get_inbox(ids[0]).await.unwrap().unwrap().status,
        InboxStatus::Expired
    );
}

/// Migration 0004 on a database that has run 0001 to 0003 and holds a thread: the old thread
/// is untouched, the new tables take rows, refuse nonsense and dedupe, and a thread takes its
/// watches with it.
#[tokio::test]
async fn migration_0004_upgrades_a_database_that_holds_threads() {
    let db = db_or_skip!();
    let pool = db.pool("orch-test-upgrade", 4).await;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, sql) in [
        ("0001_init.sql", include_str!("../migrations/0001_init.sql")),
        (
            "0002_ui_events.sql",
            include_str!("../migrations/0002_ui_events.sql"),
        ),
        (
            "0003_job_ledger.sql",
            include_str!("../migrations/0003_job_ledger.sql"),
        ),
    ] {
        std::fs::write(dir.join(name), sql).unwrap();
    }
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let thread = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, created_at, updated_at) \
         VALUES ($1, 'alice@example.com', 't', 'coder', 'working', 1, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query("SELECT 1 FROM inbox")
            .execute(&pool)
            .await
            .is_err()
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    let old = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.state, ThreadState::Working);

    let insert = |id: Uuid, kind: &'static str, status: &'static str, key: &'static str| {
        sqlx::query(
            "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, available_at, \
             created_at, updated_at) VALUES ($1, 'github', $4, $2, '{}', $3, now(), now(), now())",
        )
        .bind(id)
        .bind(kind)
        .bind(status)
        .bind(key)
        .execute(store.pool())
    };
    insert(Uuid::now_v7(), "ci_report", "pending", "d1")
        .await
        .unwrap();
    assert!(
        insert(Uuid::now_v7(), "ci_report", "pending", "d1")
            .await
            .is_err(),
        "unique"
    );
    assert!(
        insert(Uuid::now_v7(), "nonsense", "pending", "d2")
            .await
            .is_err(),
        "kind"
    );
    assert!(
        insert(Uuid::now_v7(), "timer", "nonsense", "d3")
            .await
            .is_err(),
        "status"
    );
    for status in ["inflight", "parked", "applied", "expired", "dead"] {
        insert(Uuid::now_v7(), "timer", status, status)
            .await
            .unwrap();
    }

    sqlx::query("INSERT INTO watches (key, thread_id, created_at) VALUES ('ci:x@y', $1, now())")
        .bind(thread)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM threads WHERE id = $1")
        .bind(thread)
        .execute(store.pool())
        .await
        .unwrap();
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM watches")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(left, 0, "a thread takes its watches with it");
}

/// Migration 0007 on a database that has run 0001 to 0006 and holds a thread with a log: the old
/// events stay, the old constraint refuses an `agent_step`, the new one takes it and still refuses
/// a kind nobody knows, and a step the core wrote reads back.
#[tokio::test]
async fn migration_0007_upgrades_a_database_that_holds_a_log() {
    let db = db_or_skip!();
    let pool = db.pool("orch-test-upgrade", 4).await;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, sql) in [
        ("0001_init.sql", include_str!("../migrations/0001_init.sql")),
        (
            "0002_ui_events.sql",
            include_str!("../migrations/0002_ui_events.sql"),
        ),
        (
            "0003_job_ledger.sql",
            include_str!("../migrations/0003_job_ledger.sql"),
        ),
        (
            "0004_inbox.sql",
            include_str!("../migrations/0004_inbox.sql"),
        ),
        (
            "0005_job_started.sql",
            include_str!("../migrations/0005_job_started.sql"),
        ),
        (
            "0006_ui_catalog.sql",
            include_str!("../migrations/0006_ui_catalog.sql"),
        ),
    ] {
        std::fs::write(dir.join(name), sql).unwrap();
    }
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let thread = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO threads (id, owner, title, agent_id, state, version, last_seq, created_at, \
         updated_at) VALUES ($1, 'alice@example.com', 't', 'coder', 'working', 1, 1, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    let insert = |seq: i64, kind: &'static str, data: &'static str| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, now(), $3, '{\"type\":\"agent\",\"name\":\"coder\"}', $4::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
        .bind(data)
    };
    insert(1, "agent_status", r#"{"status":"working"}"#)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        insert(2, "agent_step", "{}").execute(&pool).await.is_err(),
        "0006 has no `agent_step`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    let step = r#"{"id":"t/tool:c1","path":[],"kind":"subagent","label":"OpenCode","state":"running","phase":"start","icon":"agent"}"#;
    insert(2, "agent_step", step)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        insert(3, "nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    let events = store.list_events(ThreadId(thread), 0, 10).await.unwrap();
    assert_eq!(events.len(), 2, "the old event is untouched");
    assert!(matches!(
        &events[1].body,
        EventBody::AgentStep(s) if s.id == "t/tool:c1" && s.label == "OpenCode"
    ));
}

/// A row this build cannot read (written by a newer build, or damaged) is handed out like any
/// other and fails on its own when decoded: it does not fail the claim of the rows beside it.
#[tokio::test]
async fn a_row_this_build_cannot_read_does_not_spoil_its_batch() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (good, _) = ci_row(1);
    let good_id = good.id;
    store.receive(good, t0()).await.unwrap();
    sqlx::query(
        "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, available_at, \
         created_at, updated_at) \
         VALUES ($1, 'github', 'odd', 'ci_report', '{\"kind\": \"from_the_future\"}', 'pending', \
         $2, $2, $2)",
    )
    .bind(Uuid::now_v7())
    .bind(jiff_sqlx::Timestamp::from(t0()))
    .execute(store.pool())
    .await
    .unwrap();

    let claimed = store
        .claim_inbox("a", t0(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 2);
    let (readable, odd): (Vec<_>, Vec<_>) = claimed.iter().partition(|r| r.decode().is_ok());
    assert_eq!(readable.len(), 1);
    assert_eq!(readable[0].id, good_id);
    let err = odd[0].decode().unwrap_err();
    assert_eq!(err.id, odd[0].id);
    assert!(
        !err.to_string().contains("from_the_future"),
        "the payload is not echoed: {err}"
    );
}

/// A cancel row written before it named its job is stored as the bare string `"cancel"`: it reads
/// as a cancel of the current job (`job: None`), and a row that names its job reads back with it
/// (ADR 0020).
#[tokio::test]
async fn a_legacy_cancel_row_reads_as_the_current_jobs_and_a_new_one_keeps_its_job() {
    let db = db_or_skip!();
    let store = db.store().await;
    let pool = db.pool("orch-test", 4).await;
    let row = |n: u128, payload: OutboxPayload| NewOutbox {
        id: OutboxId(Uuid::from_u128(
            0x0190_0000_0000_7000_a000_0000_0000_0000 + n,
        )),
        payload,
    };
    let id = create(
        &store,
        vec![
            row(1, OutboxPayload::Cancel { job: Some(3) }),
            row(2, OutboxPayload::Cancel { job: None }),
        ],
    )
    .await;
    // what an older build wrote: the unit variant, a bare JSON string
    sqlx::query(
        "INSERT INTO outbox (id, thread_id, kind, payload, status, next_attempt_at, created_at, \
         updated_at) VALUES ($1, $2, 'cancel', '\"cancel\"'::jsonb, 'pending', now(), now(), now())",
    )
    .bind(Uuid::from_u128(0x0190_0000_0000_7000_a000_0000_0000_0009))
    .bind(id.0)
    .execute(&pool)
    .await
    .unwrap();

    let open = store.list_open_outbox(id).await.unwrap();
    let job_of = |n: u128| {
        open.iter()
            .find(|r| r.id.0 == Uuid::from_u128(0x0190_0000_0000_7000_a000_0000_0000_0000 + n))
            .map(|r| r.payload.clone())
    };
    assert_eq!(job_of(1), Some(OutboxPayload::Cancel { job: Some(3) }));
    assert_eq!(job_of(2), Some(OutboxPayload::Cancel { job: None }));
    assert_eq!(job_of(9), Some(OutboxPayload::Cancel { job: None }));
    // a row that names its job stores it
    let (stored,): (serde_json::Value,) =
        sqlx::query_as("SELECT payload FROM outbox WHERE id = $1")
            .bind(Uuid::from_u128(0x0190_0000_0000_7000_a000_0000_0000_0001))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, serde_json::json!({"cancel": {"job": 3}}));
}
