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
    NewOutbox, NewThreadRecord, OutboxId, OutboxKind, OutboxPayload, Parking, Received, StoreError,
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
        title: None,
        description: None,
        skip_unsent_delegates: false,
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
            mentions: Vec::new(),
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
                description: None,
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
                description: None,
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
                description: None,
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

/// Migration 0008 on a database that has run 0001 to 0007 and holds a thread with a log: the old
/// events stay, the old constraint refuses a `thread_titled`, the new one takes it and still refuses
/// a kind nobody knows, and a rename the core wrote reads back.
#[tokio::test]
async fn migration_0008_upgrades_a_database_that_holds_a_log() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
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
             VALUES ($1, $2, now(), $3, '{\"type\":\"user\",\"name\":\"alice@example.com\"}', $4::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
        .bind(data)
    };
    insert(1, "user_message", r#"{"text":"hi"}"#)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        insert(2, "thread_titled", "{}")
            .execute(&pool)
            .await
            .is_err(),
        "0007 has no `thread_titled`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    insert(
        2,
        "thread_titled",
        r#"{"title":"Fix the build","source":"user"}"#,
    )
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
        EventBody::ThreadTitled(t) if t.title == "Fix the build" && t.source == orch_core::TitledBy::User
    ));
}

/// Migration 0009 on a database that has run 0001 to 0008 and holds a thread with a log: the old
/// events stay, the old constraint refuses a `thread_titled`, the new one takes it and still refuses
/// a kind nobody knows, and a rename the core wrote reads back.
#[tokio::test]
async fn migration_0009_upgrades_a_database_that_holds_a_log() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
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
         updated_at) VALUES ($1, 'alice@example.com', 't', 'coder', 'working', 1, 0, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    let insert = |kind: &'static str, payload: &'static str| {
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, next_attempt_at, \
             created_at, updated_at) VALUES ($1, $2, $3, $4::jsonb, 'pending', 0, now(), now(), now())",
        )
        .bind(Uuid::now_v7())
        .bind(thread)
        .bind(kind)
        .bind(payload)
    };
    insert(
        "delegate",
        r#"{"delegate": {"text": "hi", "release": null}}"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        insert("title", r#"{"title": {"ask": 1}}"#)
            .execute(&pool)
            .await
            .is_err(),
        "0008 has no outbox kind `title`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    insert("title", r#"{"title": {"ask": 1}}"#)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        insert("nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    // the old row is untouched, and the title row reads back as a title request and is claimed at
    // once whatever delegation of its thread is in flight
    let claimed = store
        .claim_outbox("w", jiff::Timestamp::now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    let kinds: Vec<_> = claimed.iter().map(|r| r.kind).collect();
    assert!(kinds.contains(&OutboxKind::Delegate), "{kinds:?}");
    assert!(kinds.contains(&OutboxKind::Title), "{kinds:?}");
    let title = claimed
        .iter()
        .find(|r| r.kind == OutboxKind::Title)
        .unwrap();
    assert_eq!(title.payload, OutboxPayload::Title { ask: 1 });
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

/// Migration 0010 adds the fork columns and the `thread_forked` kind to a database that holds a
/// log: the old rows stay and read as threads that were not forked, the old constraint refuses the
/// new kind, the new one takes it and still refuses an unknown kind and a half-set fork origin.
#[tokio::test]
async fn migration_0010_upgrades_a_database_that_holds_a_log() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
        ),
        (
            "0009_title_requests.sql",
            include_str!("../migrations/0009_title_requests.sql"),
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
         updated_at) VALUES ($1, 'alice@example.com', 't', 'coder', 'done', 1, 1, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    let insert = |thread: Uuid, seq: i64, kind: &'static str| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, now(), $3, '{}'::jsonb, '{}'::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
    };
    insert(thread, 1, "user_message")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        insert(thread, 2, "thread_forked")
            .execute(&pool)
            .await
            .is_err(),
        "0009 has no event kind `thread_forked`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    insert(thread, 2, "thread_forked")
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        insert(thread, 3, "nonsense")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    // the old thread reads as one that was not forked, and its log is as it was
    let old = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.forked_from, None);
    let origin = |from: Option<Uuid>, at: Option<i64>, kind: Option<&'static str>| {
        sqlx::query(
            "UPDATE threads SET forked_from = $2, forked_at = $3, fork_kind = $4 WHERE id = $1",
        )
        .bind(thread)
        .bind(from)
        .bind(at)
        .bind(kind)
    };
    for (what, from, at, kind) in [
        ("a cut with no kind", None, Some(1_i64), None),
        ("a kind with no cut", None, None, Some("fork")),
        ("a parent with no cut", Some(Uuid::now_v7()), None, None),
        ("an unknown kind", None, Some(1), Some("branch")),
        ("a negative cut", None, Some(-1), Some("fork")),
    ] {
        assert!(
            origin(from, at, kind).execute(store.pool()).await.is_err(),
            "{what} is refused"
        );
    }
    origin(None, Some(1), Some("fork"))
        .execute(store.pool())
        .await
        .unwrap();
    let forked = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        forked.forked_from,
        Some(orch_core::ForkedFrom {
            thread_id: None,
            seq: 1,
            kind: orch_core::ForkKind::Fork
        })
    );
}

/// Migration 0011 on a database that has run 0001 to 0010 and holds a thread with a log and a
/// `title` row: the old rows stay, the old thread has no description, the old constraints refuse a
/// `thread_described` event and a `description` row, the new ones take them and still refuse a kind
/// nobody knows, a description the core wrote reads back, and the column refuses an empty or an
/// over long one.
#[tokio::test]
async fn migration_0011_upgrades_a_database_that_holds_a_log() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
        ),
        (
            "0009_title_requests.sql",
            include_str!("../migrations/0009_title_requests.sql"),
        ),
        (
            "0010_thread_forks.sql",
            include_str!("../migrations/0010_thread_forks.sql"),
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
         updated_at) VALUES ($1, 'alice@example.com', 't', 'coder', 'done', 1, 1, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    let event = |seq: i64, kind: &'static str, data: &'static str| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, now(), $3, '{\"type\":\"system\",\"name\":\"orchestrator\"}', $4::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
        .bind(data)
    };
    let row = |kind: &'static str, payload: &'static str| {
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, next_attempt_at, \
             created_at, updated_at) VALUES ($1, $2, $3, $4::jsonb, 'pending', 0, now(), now(), now())",
        )
        .bind(Uuid::now_v7())
        .bind(thread)
        .bind(kind)
        .bind(payload)
    };
    event(1, "user_message", r#"{"text":"hi"}"#)
        .execute(&pool)
        .await
        .unwrap();
    row("title", r#"{"title": {"ask": 1}}"#)
        .execute(&pool)
        .await
        .unwrap();
    let description_data = r#"{"description":"Moving the build to Rust.","source":"model"}"#;
    assert!(
        event(2, "thread_described", description_data)
            .execute(&pool)
            .await
            .is_err(),
        "0010 has no event kind `thread_described`"
    );
    assert!(
        row("description", r#"{"description": {"job": 1}}"#)
            .execute(&pool)
            .await
            .is_err(),
        "0010 has no outbox kind `description`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    event(2, "thread_described", description_data)
        .execute(store.pool())
        .await
        .unwrap();
    row("description", r#"{"description": {"job": 1}}"#)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        event(3, "nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    assert!(
        row("nonsense", "{}").execute(store.pool()).await.is_err(),
        "the constraint still names the kinds"
    );
    // the old rows are as they were
    let events = store.list_events(ThreadId(thread), 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[1].body,
        EventBody::ThreadDescribed(d)
            if d.description == "Moving the build to Rust." && d.source == orch_core::DescribedBy::Model
    ));
    let old = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.description, None, "the old thread has no description");
    // the column takes a description and refuses an empty one or one over the limit
    let set = |text: String| {
        sqlx::query("UPDATE threads SET description = $2 WHERE id = $1")
            .bind(thread)
            .bind(text)
    };
    set("Moving the build to Rust.".to_owned())
        .execute(store.pool())
        .await
        .unwrap();
    assert!(set(String::new()).execute(store.pool()).await.is_err());
    assert!(set("x".repeat(501)).execute(store.pool()).await.is_err());
    set("é".repeat(500)).execute(store.pool()).await.unwrap();
    let described = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        described.description.as_deref(),
        Some("é".repeat(500).as_str())
    );
}

/// Migration 0012 on a database that has run 0001 to 0011 and holds a thread with a log: the old rows
/// stay, the old constraint refuses a `tools_attached` and a `tools_detached` event, the new one takes
/// them and still refuses a kind nobody knows, an event the core wrote reads back, the thread's set of
/// servers (inside `threads.job`, so no column) survives a read, and the outbox is untouched (attaching
/// writes no delegation).
#[tokio::test]
async fn migration_0012_upgrades_a_database_that_holds_a_log() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
        ),
        (
            "0009_title_requests.sql",
            include_str!("../migrations/0009_title_requests.sql"),
        ),
        (
            "0010_thread_forks.sql",
            include_str!("../migrations/0010_thread_forks.sql"),
        ),
        (
            "0011_thread_description.sql",
            include_str!("../migrations/0011_thread_description.sql"),
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
         updated_at) VALUES ($1, 'alice@example.com', 't', 'coder', 'done', 1, 1, now(), now())",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    let event = |seq: i64, kind: &'static str, data: &'static str| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, now(), $3, '{\"type\":\"user\",\"name\":\"alice@example.com\"}', $4::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
        .bind(data)
    };
    event(1, "user_message", r#"{"text":"hi"}"#)
        .execute(&pool)
        .await
        .unwrap();
    for kind in ["tools_attached", "tools_detached"] {
        assert!(
            event(2, kind, r#"{"servers":["websearch"]}"#)
                .execute(&pool)
                .await
                .is_err(),
            "0011 has no event kind `{kind}`"
        );
    }

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    event(2, "tools_attached", r#"{"servers":["docs","websearch"]}"#)
        .execute(store.pool())
        .await
        .unwrap();
    event(3, "tools_detached", r#"{"servers":["docs"]}"#)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        event(4, "tools_nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    // the old rows are as they were, and the new ones read back as the core reads them
    let events = store.list_events(ThreadId(thread), 0, 10).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0].body, EventBody::UserMessage(m) if m.text == "hi"));
    assert!(matches!(
        &events[1].body,
        EventBody::ToolsAttached(d) if d.servers == ["docs", "websearch"]
    ));
    assert!(matches!(
        &events[2].body,
        EventBody::ToolsDetached(d) if d.servers == ["docs"]
    ));
    // the old thread has no servers, and a job ledger that names some reads back
    let old = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert!(old.job.tools.is_empty());
    sqlx::query("UPDATE threads SET job = $2::jsonb WHERE id = $1")
        .bind(thread)
        .bind(r#"{"tools":["websearch"]}"#)
        .execute(store.pool())
        .await
        .unwrap();
    let set = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(set.job.tools, ["websearch"]);
    // the outbox kinds are the ones 0011 left
    let kinds: Vec<(String,)> = sqlx::query_as(
        // This test's own table: every test has a schema of its own in one database, and
        // another test may be migrating (or dropping) its copy of the constraint right now.
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint \
         WHERE conname = 'outbox_kind_check' AND conrelid = 'outbox'::regclass",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert!(
        kinds[0].0.contains("'description'") && !kinds[0].0.contains("tools"),
        "{kinds:?}"
    );
}

/// Migration 0014 on a database that has run 0001 to 0013 and holds a thread with a log and an
/// outbox: the old rows stay and read back, the old constraints refuse the events `ask_started` and
/// `ask_finished` and an `ask` outbox row, the new ones take them and still refuse a kind nobody
/// knows, the events read back as the core reads them, a job ledger with asks in `threads.job` reads
/// back (an older one has none), and an `ask` row is claimed beside the delegation in flight with its
/// task kept on the row.
#[tokio::test]
async fn migration_0014_upgrades_a_database_that_holds_a_log_and_an_outbox() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
        ),
        (
            "0009_title_requests.sql",
            include_str!("../migrations/0009_title_requests.sql"),
        ),
        (
            "0010_thread_forks.sql",
            include_str!("../migrations/0010_thread_forks.sql"),
        ),
        (
            "0011_thread_description.sql",
            include_str!("../migrations/0011_thread_description.sql"),
        ),
        (
            "0012_tools.sql",
            include_str!("../migrations/0012_tools.sql"),
        ),
        (
            "0013_steer.sql",
            include_str!("../migrations/0013_steer.sql"),
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
    let event = |seq: i64, kind: &'static str, actor: &'static str, data: &'static str| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, now(), $3, $4::jsonb, $5::jsonb)",
        )
        .bind(thread)
        .bind(seq)
        .bind(kind)
        .bind(actor)
        .bind(data)
    };
    const USER: &str = r#"{"type":"user","name":"alice@example.com"}"#;
    const CODER: &str = r#"{"type":"agent","name":"coder"}"#;
    const SYSTEM: &str = r#"{"type":"system","name":"orchestrator"}"#;
    const STARTED: &str =
        r#"{"ask":1,"agent":"researcher","by":"main","depth":1,"text":"find it","stepId":"ask-1"}"#;
    const FINISHED: &str =
        r#"{"ask":1,"state":"timed_out","error":"the asked agent did not answer in time"}"#;
    event(1, "user_message", USER, r#"{"text":"hi @researcher"}"#)
        .execute(&pool)
        .await
        .unwrap();
    for (kind, data) in [("ask_started", STARTED), ("ask_finished", FINISHED)] {
        assert!(
            event(2, kind, CODER, data).execute(&pool).await.is_err(),
            "0013 has no event kind `{kind}`"
        );
    }
    let delegate = Uuid::now_v7();
    let ask = Uuid::now_v7();
    let row = |id: Uuid, kind: &'static str, payload: &'static str| {
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, next_attempt_at, \
             created_at, updated_at) VALUES ($1, $2, $3, $4::jsonb, 'pending', 0, now(), now(), now())",
        )
        .bind(id)
        .bind(thread)
        .bind(kind)
        .bind(payload)
    };
    row(
        delegate,
        "delegate",
        r#"{"delegate": {"text": "hi", "release": null}}"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    const ASK: &str =
        r#"{"ask": {"job": 1, "ask": 1, "agent": "researcher", "depth": 1, "text": "find it"}}"#;
    assert!(
        row(ask, "ask", ASK).execute(&pool).await.is_err(),
        "0013 has no outbox kind `ask`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    event(2, "ask_started", CODER, STARTED)
        .execute(store.pool())
        .await
        .unwrap();
    event(3, "ask_finished", SYSTEM, FINISHED)
        .execute(store.pool())
        .await
        .unwrap();
    row(ask, "ask", ASK).execute(store.pool()).await.unwrap();
    assert!(
        event(4, "ask_nonsense", SYSTEM, "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the events constraint still names the kinds"
    );
    assert!(
        row(Uuid::now_v7(), "nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the outbox constraint still names the kinds"
    );
    // Each constraint holds every kind there is. This test's own table: every test has a schema of
    // its own in one database, and another test may be migrating its copy right now.
    for (constraint, table, kinds) in [
        (
            "events_kind_check",
            "events",
            &[
                "user_message",
                "agent_message",
                "agent_status",
                "artifact",
                "thread_state",
                "error",
                "ui_surface",
                "ui_action",
                "ci_result",
                "check_result",
                "rework",
                "job_started",
                "ui_catalog",
                "agent_step",
                "thread_titled",
                "thread_forked",
                "thread_described",
                "tools_attached",
                "tools_detached",
                "ask_started",
                "ask_finished",
            ][..],
        ),
        (
            "outbox_kind_check",
            "outbox",
            &[
                "delegate",
                "cancel",
                "verify",
                "title",
                "description",
                "steer",
                "ask",
            ][..],
        ),
    ] {
        let (def,): (String,) = sqlx::query_as(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint \
             WHERE conname = $1 AND conrelid = $2::regclass",
        )
        .bind(constraint)
        .bind(table)
        .fetch_one(store.pool())
        .await
        .unwrap();
        for kind in kinds {
            assert!(
                def.contains(&format!("'{kind}'")),
                "{constraint}: {kind}: {def}"
            );
        }
    }

    // the old row is as it was, and the new ones read as the core reads them
    let events = store.list_events(ThreadId(thread), 0, 10).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(&events[0].body, EventBody::UserMessage(m) if m.text == "hi @researcher"));
    let EventBody::AskStarted(started) = &events[1].body else {
        panic!("an ask_started: {:?}", events[1]);
    };
    assert_eq!(
        (
            started.ask,
            started.agent.as_str(),
            started.depth,
            started.step_id.as_str()
        ),
        (1, "researcher", 1, "ask-1")
    );
    assert_eq!(started.by, orch_core::Caller::Main);
    assert_eq!(events[1].actor, Actor::agent(&AgentId::new("coder"), None));
    let EventBody::AskFinished(finished) = &events[2].body else {
        panic!("an ask_finished: {:?}", events[2]);
    };
    assert_eq!(finished.state, orch_core::AskOutcome::TimedOut);
    assert_eq!(events[2].actor, Actor::system());

    // the old thread has no asks, and a job ledger that names some reads back
    let old = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert!(old.job.asks.is_empty());
    sqlx::query("UPDATE threads SET job = $2::jsonb WHERE id = $1")
        .bind(thread)
        .bind(
            r#"{"mentioned":["researcher"],"asks":[{"n":1,"by":"main","agent":"researcher","depth":1,"outcome":"timed_out"}]}"#,
        )
        .execute(store.pool())
        .await
        .unwrap();
    let set = store
        .get_thread(None, ThreadId(thread))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        set.job.asks,
        [orch_core::Ask {
            n: 1,
            by: orch_core::Caller::Main,
            agent: AgentId::new("researcher"),
            depth: 1,
            call_key: None,
            fingerprint: None,
            task_id: None,
            outcome: Some(orch_core::AskOutcome::TimedOut),
        }]
    );

    // the delegation is claimed and in flight; the ask is claimed beside it, reads as the core
    // writes it, and keeps its task on the row
    let first = store
        .claim_outbox("w", jiff::Timestamp::now(), Duration::from_secs(30), 1)
        .await
        .unwrap();
    assert_eq!(first[0].kind, OutboxKind::Delegate);
    let claimed = store
        .claim_outbox("w", jiff::Timestamp::now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1, "the ask, and not the delegation again");
    let new = &claimed[0];
    assert_eq!((new.id, new.kind), (OutboxId(ask), OutboxKind::Ask));
    assert_eq!(
        new.payload,
        OutboxPayload::Ask {
            job: 1,
            ask: 1,
            agent: AgentId::new("researcher"),
            depth: 1,
            text: "find it".to_owned(),
            continue_task: None,
            reference_task_ids: Vec::new(),
        }
    );
    assert!(
        store
            .mark_verify_sent(
                &new.lease().unwrap(),
                "t-ask".to_owned(),
                jiff::Timestamp::now()
            )
            .await
            .unwrap()
    );
    let back = store.get_outbox(OutboxId(ask)).await.unwrap().unwrap();
    assert_eq!(back.task_id.as_deref(), Some("t-ask"));
}

/// A fork outlives its parent: deleting the parent leaves the fork whole, with its own copy of the
/// log and an origin that no longer names a thread, and a family of edits starts again at the
/// edit whose parent is gone.
#[tokio::test]
async fn a_fork_survives_the_deletion_of_its_parent() {
    use orch_core::{ForkKind, ForkSource, ForkedFrom, ThreadForkedData};
    let db = db_or_skip!();
    let store = db.store().await;
    let parent = create(&store, vec![delegate()]).await;
    store
        .commit(
            parent,
            1,
            commit(
                ThreadState::Working,
                vec![event("one", None), event("two", None)],
                vec![],
            ),
        )
        .await
        .unwrap();
    let make = |n: u8, parent: ThreadId, kind: ForkKind| {
        let store = store.clone();
        async move {
            let id = ThreadId(Uuid::now_v7());
            let new = NewThreadRecord {
                id,
                owner: user(),
                title: format!("fork {n}"),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: id.to_string(),
                now: t0(),
            };
            let data = ThreadForkedData {
                from: ForkSource {
                    thread_id: parent,
                    seq: 2,
                },
                kind,
                title: "t".to_owned(),
                description: None,
                target: new.target.clone(),
            };
            let forked = NewEvent {
                at: t0(),
                actor: Actor::user(&user()),
                body: EventBody::ThreadForked(data),
                idempotency_key: None,
            };
            store
                .fork_thread(
                    new,
                    orch_ports::ForkOrigin {
                        parent,
                        cut: 2,
                        kind,
                    },
                    commit(ThreadState::Done, vec![forked, event("mine", None)], vec![]),
                )
                .await
                .unwrap();
            id
        }
    };
    let child = make(1, parent, ForkKind::Edit).await;
    let grandchild = make(2, child, ForkKind::Edit).await;
    assert_eq!(
        store.fork_family(&user(), grandchild).await.unwrap().len(),
        3
    );

    sqlx::query("DELETE FROM threads WHERE id = $1")
        .bind(parent.0)
        .execute(store.pool())
        .await
        .unwrap();
    let child_after = store.get_thread(None, child).await.unwrap().unwrap();
    assert_eq!(
        child_after.forked_from,
        Some(ForkedFrom {
            thread_id: None,
            seq: 2,
            kind: ForkKind::Edit
        })
    );
    // its copy of the log is its own
    let events = store.list_events(child, 0, 100).await.unwrap();
    assert_eq!(events.len(), 4, "two copied, the fork's event and its own");
    // the family starts at the edit whose parent is gone, and its edit is still linked to it
    let family = store.fork_family(&user(), grandchild).await.unwrap();
    assert_eq!(
        family
            .iter()
            .map(|n| (n.id, n.link.is_some()))
            .collect::<Vec<_>>(),
        vec![(child, false), (grandchild, true)]
    );
}

/// The copy is one transaction with the thread, and sixteen forks of one thread made while the
/// parent goes on being written to each hold exactly the events up to their cut.
#[tokio::test]
async fn forks_made_while_the_parent_is_written_to_copy_exactly_their_cut() {
    use orch_core::{ForkKind, ForkSource, ThreadForkedData};
    let db = db_or_skip!();
    let store = db.store().await;
    let parent = create(&store, vec![delegate()]).await;
    let mut version = 1;
    for round in 0..8 {
        let (outcome, _) = match store
            .commit(
                parent,
                version,
                commit(
                    ThreadState::Working,
                    vec![event(&format!("e{round}"), None)],
                    vec![],
                ),
            )
            .await
            .unwrap()
        {
            CommitOutcome::Applied { thread, events } => (thread, events),
            other => panic!("{other:?}"),
        };
        version = outcome.version;
    }
    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            let mut version = version;
            for round in 0..20 {
                match store
                    .commit(
                        parent,
                        version,
                        commit(
                            ThreadState::Working,
                            vec![event(&format!("w{round}"), None)],
                            vec![],
                        ),
                    )
                    .await
                {
                    Ok(CommitOutcome::Applied { thread, .. }) => version = thread.version,
                    other => panic!("{other:?}"),
                }
            }
        })
    };
    let mut forks = Vec::new();
    for cut in 1..=9_i64 {
        let store = store.clone();
        forks.push(tokio::spawn(async move {
            let id = ThreadId(Uuid::now_v7());
            let new = NewThreadRecord {
                id,
                owner: user(),
                title: "f".to_owned(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: id.to_string(),
                now: t0(),
            };
            let forked = NewEvent {
                at: t0(),
                actor: Actor::user(&user()),
                body: EventBody::ThreadForked(ThreadForkedData {
                    from: ForkSource {
                        thread_id: parent,
                        seq: cut,
                    },
                    kind: ForkKind::Fork,
                    title: "f".to_owned(),
                    description: None,
                    target: new.target.clone(),
                }),
                idempotency_key: None,
            };
            store
                .fork_thread(
                    new,
                    orch_ports::ForkOrigin {
                        parent,
                        cut,
                        kind: ForkKind::Fork,
                    },
                    commit(ThreadState::Done, vec![forked], vec![]),
                )
                .await
                .unwrap();
            (id, cut)
        }));
    }
    writer.await.unwrap();
    let parent_events = store.list_events(parent, 0, 1000).await.unwrap();
    for fork in forks {
        let (id, cut) = fork.await.unwrap();
        let events = store.list_events(id, 0, 1000).await.unwrap();
        let cut_len = usize::try_from(cut).unwrap();
        assert_eq!(events.len(), cut_len + 1, "cut {cut}");
        for (copy, original) in events.iter().zip(&parent_events).take(cut_len) {
            assert_eq!(
                (copy.seq, copy.at, &copy.body),
                (original.seq, original.at, &original.body)
            );
        }
        assert_eq!(events[cut_len].seq, cut + 1);
    }
}

/// Migration 0013 on a database that has run 0001 to 0012 and holds a thread with an outbox: the old
/// rows stay and read back, the old constraint refuses a `steer` row, the new one takes it and still
/// refuses a kind nobody knows, a steer row is claimed beside the delegation in flight and rewritten
/// as the delegation it stands for, and the rewritten row reads back as a delegation.
#[tokio::test]
async fn migration_0013_upgrades_a_database_that_holds_an_outbox() {
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
        (
            "0007_agent_step.sql",
            include_str!("../migrations/0007_agent_step.sql"),
        ),
        (
            "0008_thread_titled.sql",
            include_str!("../migrations/0008_thread_titled.sql"),
        ),
        (
            "0009_title_requests.sql",
            include_str!("../migrations/0009_title_requests.sql"),
        ),
        (
            "0010_thread_forks.sql",
            include_str!("../migrations/0010_thread_forks.sql"),
        ),
        (
            "0011_thread_description.sql",
            include_str!("../migrations/0011_thread_description.sql"),
        ),
        (
            "0012_tools.sql",
            include_str!("../migrations/0012_tools.sql"),
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
    let delegate = Uuid::now_v7();
    let steer = Uuid::now_v7();
    let row = |id: Uuid, kind: &'static str, payload: &'static str| {
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, attempts, next_attempt_at, \
             created_at, updated_at) VALUES ($1, $2, $3, $4::jsonb, 'pending', 0, now(), now(), now())",
        )
        .bind(id)
        .bind(thread)
        .bind(kind)
        .bind(payload)
    };
    row(
        delegate,
        "delegate",
        r#"{"delegate": {"text": "hi", "release": null}}"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let steer_payload = r#"{"steer": {"text": "you were wrong", "release": null}}"#;
    assert!(
        row(steer, "steer", steer_payload)
            .execute(&pool)
            .await
            .is_err(),
        "0012 has no outbox kind `steer`"
    );

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    row(steer, "steer", steer_payload)
        .execute(store.pool())
        .await
        .unwrap();
    assert!(
        row(Uuid::now_v7(), "nonsense", "{}")
            .execute(store.pool())
            .await
            .is_err(),
        "the constraint still names the kinds"
    );
    let kinds: Vec<(String,)> = sqlx::query_as(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'outbox_kind_check' \
         AND conrelid = 'outbox'::regclass",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    for kind in [
        "delegate",
        "cancel",
        "verify",
        "title",
        "description",
        "steer",
    ] {
        assert!(
            kinds[0].0.contains(&format!("'{kind}'")),
            "{kind}: {kinds:?}"
        );
    }

    // the old row is as it was, and the new one reads as the core writes it; both are claimed (a
    // steer does not wait for the delegation) and the steer becomes the delegation it stands for
    let claimed = store
        .claim_outbox("w", jiff::Timestamp::now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    let old = claimed.iter().find(|r| r.id == OutboxId(delegate)).unwrap();
    assert_eq!(old.kind, OutboxKind::Delegate);
    let new = claimed.iter().find(|r| r.id == OutboxId(steer)).unwrap();
    assert_eq!(new.kind, OutboxKind::Steer);
    assert_eq!(
        new.payload,
        OutboxPayload::Steer {
            text: "you were wrong".to_owned(),
            release: None,
            ui_catalog: None,
            mentions: Vec::new(),
        }
    );
    assert!(
        store
            .requeue_as_delegate(&new.lease().unwrap(), jiff::Timestamp::now())
            .await
            .unwrap()
    );
    let back = store.get_outbox(OutboxId(steer)).await.unwrap().unwrap();
    assert_eq!(back.kind, OutboxKind::Delegate);
    assert_eq!(
        back.payload,
        OutboxPayload::Delegate {
            text: "you were wrong".to_owned(),
            release: None,
            new_job: false,
            ui_catalog: None,
            mentions: Vec::new(),
        }
    );
}
