//! The search index over Postgres (ADR 0068, migration `0025`): what the port's conformance cases cannot
//! say, because it is about the tables and the database. Skipped unless `ORCH_TEST_DATABASE_URL` is set;
//! each test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::time::Duration;

use jiff::Timestamp;
use orch_core::search::SearchTerms;
use orch_core::{
    Actor, AgentId, AgentTarget, Classify, ErrorClass, EventBody, ThreadId, ThreadState, UserId,
    UserMessageData,
};
use orch_ports::{
    ArchivedFilter, Commit, NewEvent, NewThreadRecord, SearchRequest, StoreError, ThreadSearch,
    ThreadStore,
};
use orch_store_postgres::PgStore;
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

fn t0() -> Timestamp {
    "2026-01-15T10:00:00Z".parse().unwrap()
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn new_thread(n: u128, title: &str) -> NewThreadRecord {
    NewThreadRecord {
        id: id(n),
        owner: alice(),
        title: title.to_owned(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        context_id: None,
        rail_parent: None,
        now: t0(),
    }
}

fn say(text: &str) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UserMessage(UserMessageData::new(text)),
        idempotency_key: None,
    }
}

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Working,
        job: None,
        events,
        outbox: Vec::new(),
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

fn request(q: &str) -> SearchRequest {
    SearchRequest {
        terms: SearchTerms::parse(q).unwrap(),
        archived: ArchivedFilter::Include,
        include_edits: false,
        after: None,
        limit: 20,
    }
}

async fn rows(store: &PgStore, table: &str) -> i64 {
    sqlx::query_scalar(match table {
        "message_terms" => "SELECT count(*) FROM message_terms",
        _ => "SELECT count(*) FROM threads WHERE title_terms IS NOT NULL",
    })
    .fetch_one(store.pool())
    .await
    .unwrap()
}

/// The words of an erased thread are gone when the delete commits, in the same transaction: no row of
/// the index is left, and none is left for a thread that was never erased.
#[tokio::test]
async fn erasing_a_thread_leaves_no_row_of_the_index() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, _) = store
        .create_thread(
            new_thread(1, "Walrus"),
            commit(vec![say("walrus one"), say("walrus two")]),
        )
        .await
        .unwrap();
    store
        .create_thread(new_thread(2, "Keeper"), commit(vec![say("keeper words")]))
        .await
        .unwrap();
    assert_eq!(rows(&store, "message_terms").await, 3);
    store
        .delete_threads(&alice(), &[(id(1), a.version)], t0())
        .await
        .unwrap();
    assert_eq!(rows(&store, "message_terms").await, 1);
    assert_eq!(rows(&store, "threads").await, 1);
    let found = store
        .search_threads(&alice(), &request("walrus"))
        .await
        .unwrap();
    assert!(found.rows.is_empty());
}

/// A commit that is refused (a version that moved) indexes nothing: the words are written with the
/// event or not at all.
#[tokio::test]
async fn a_refused_commit_indexes_nothing() {
    let db = db_or_skip!();
    let store = db.store().await;
    store
        .create_thread(new_thread(1, "Notes"), commit(vec![say("first")]))
        .await
        .unwrap();
    let refused = store
        .commit(id(1), 7, commit(vec![say("zebra never written")]))
        .await
        .unwrap_err();
    assert!(matches!(refused, StoreError::VersionConflict));
    assert_eq!(rows(&store, "message_terms").await, 1);
    assert!(
        store
            .search_threads(&alice(), &request("zebra"))
            .await
            .unwrap()
            .rows
            .is_empty()
    );
}

/// A search that runs out of time is a transient failure (the API's 503 `search_unavailable`), not a
/// hang and not an internal error: a lock held on the index makes the search wait for its two seconds.
#[tokio::test]
async fn a_search_that_runs_out_of_time_is_unavailable() {
    let db = db_or_skip!();
    let store = db.store().await;
    store
        .create_thread(new_thread(1, "Notes"), commit(vec![say("zebra")]))
        .await
        .unwrap();
    let mut blocker = store.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE message_terms IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let err = store
        .search_threads(&alice(), &request("zebra"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Unavailable { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Transient);
    // the timeout is the search's own: it gave up, and well before anything else would
    assert!(started.elapsed() < Duration::from_secs(30));
    blocker.rollback().await.unwrap();
    // and the store is as it was
    assert_eq!(
        store
            .search_threads(&alice(), &request("zebra"))
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
}

/// The index holds the keys the Rust rules made, as lexemes exactly as given: a key is a lexeme of its
/// own (no stemming, no stop word, no case folding by the database), and what is stored is what
/// `orch_core::search` says.
#[tokio::test]
async fn the_index_holds_the_rust_keys_as_they_are() {
    let db = db_or_skip!();
    let store = db.store().await;
    store
        .create_thread(
            new_thread(
                1,
                "The Caf\u{e9} of the \u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}",
            ),
            commit(vec![say(
                "Running runs, and a C\u{153}ur! 2026 \u{6771}\u{4eac}",
            )]),
        )
        .await
        .unwrap();
    let lexemes = |sql: &'static str| {
        let pool = store.pool().clone();
        async move {
            let mut got: Vec<String> = sqlx::query_scalar::<_, Vec<String>>(sql)
                .fetch_one(&pool)
                .await
                .unwrap();
            got.sort();
            got
        }
    };
    // 'the' is not dropped as a stop word, 'cafe' is not stemmed, case and accents are Rust's
    assert_eq!(
        lexemes("SELECT tsvector_to_array(title_terms) FROM threads").await,
        ["cafe", "of", "the", "привет"]
    );
    assert_eq!(
        lexemes("SELECT tsvector_to_array(terms) FROM message_terms").await,
        ["2026", "a", "and", "coeur", "running", "runs", "東京"]
    );
}

/// A thread of an old log (a build without the index) is not found until it is indexed, and the next
/// commit to it indexes the whole log before its own events: the watermark of the thread moves with it.
#[tokio::test]
async fn the_watermark_follows_the_log() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (rec, _) = store
        .create_thread(new_thread(1, "Notes"), commit(vec![say("one"), say("two")]))
        .await
        .unwrap();
    let through = |store: &PgStore| {
        let pool = store.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT search_through FROM threads")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(through(&store).await, 2);
    store
        .commit(id(1), rec.version, commit(vec![say("three")]))
        .await
        .unwrap();
    assert_eq!(through(&store).await, 3);
    // a fork's watermark is its last seq: it indexed what it copied from its own log
    let (fork, _) = store
        .fork_thread(
            new_thread(2, "Fork"),
            orch_ports::ForkOrigin {
                parent: id(1),
                cut: 3,
                kind: orch_core::ForkKind::Fork,
            },
            commit(vec![say("four")]),
        )
        .await
        .unwrap();
    assert_eq!(fork.last_seq, 4);
    let rows: Vec<(uuid::Uuid, i64)> =
        sqlx::query_as("SELECT thread_id, count(*)::bigint FROM message_terms GROUP BY thread_id ORDER BY thread_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(rows, vec![(id(1).0, 3), (id(2).0, 4)]);
}

/// A commit that fails **after** its index rows were written, for a reason that is not a version conflict
/// (a trigger of the test raises when the thread row is updated, the last statement of the commit), leaves
/// no row of the index and no event: the words are written in the transaction of the event or not at all.
#[tokio::test]
async fn a_commit_that_fails_late_leaves_no_index_row() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (rec, _) = store
        .create_thread(new_thread(1, "Notes"), commit(vec![say("walrus first")]))
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION fail_late() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
         RAISE EXCEPTION 'a test made this commit fail late'; END $$",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_late BEFORE UPDATE ON threads FOR EACH ROW \
         WHEN (NEW.last_seq = 2) EXECUTE FUNCTION fail_late()",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let err = store
        .commit(id(1), rec.version, commit(vec![say("otter second")]))
        .await
        .unwrap_err();
    assert!(
        !matches!(err, StoreError::VersionConflict),
        "it failed late, not on the version: {err:?}"
    );
    assert_eq!(
        rows(&store, "message_terms").await,
        1,
        "no row of the failed commit"
    );
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(events, 1);
    assert!(
        store
            .search_threads(&alice(), &request("otter"))
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    // and without the failure it is searched
    sqlx::query("DROP TRIGGER fail_late ON threads")
        .execute(store.pool())
        .await
        .unwrap();
    store
        .commit(id(1), rec.version, commit(vec![say("otter second")]))
        .await
        .unwrap();
    assert_eq!(
        store
            .search_threads(&alice(), &request("otter"))
            .await
            .unwrap()
            .rows
            .len(),
        1
    );
}

/// The watermark moves with a commit and the commit indexes its own events only: the rows of the events
/// before it are not written again (same transaction ids), and the row of its own is new.
#[tokio::test]
async fn a_later_commit_indexes_only_its_own_events() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (rec, _) = store
        .create_thread(
            new_thread(1, "Notes"),
            commit(vec![say("one"), say("two"), say("three")]),
        )
        .await
        .unwrap();
    let snapshot = |store: &PgStore| {
        let pool = store.pool().clone();
        async move {
            sqlx::query_as::<_, (i64, String)>(
                "SELECT seq, xmin::text FROM message_terms ORDER BY seq",
            )
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let before = snapshot(&store).await;
    assert_eq!(before.len(), 3);
    store
        .commit(id(1), rec.version, commit(vec![say("four")]))
        .await
        .unwrap();
    let after = snapshot(&store).await;
    assert_eq!(after.len(), 4);
    assert_eq!(after[..3], before[..], "the old rows were not rewritten");
    assert_eq!(after[3].0, 4);
    let through: i64 = sqlx::query_scalar("SELECT search_through FROM threads")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(through, 4);
    let fold: i32 = sqlx::query_scalar("SELECT search_fold FROM threads")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(fold, orch_core::search::FOLD_VERSION);
}

/// A commit to a thread with a long old log moves the watermark by exactly the bound and not past it, and
/// its own events are in the index above the watermark.
#[tokio::test]
async fn a_commit_moves_the_watermark_by_the_bound_and_no_further() {
    let db = db_or_skip!();
    let store = db.store().await;
    let events: Vec<NewEvent> = (1..=1200).map(|n| say(&format!("filler {n}"))).collect();
    let (rec, _) = store
        .create_thread(new_thread(1, "Old"), commit(events))
        .await
        .unwrap();
    sqlx::query("DELETE FROM message_terms")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET search_through = 0, title_terms = NULL")
        .execute(store.pool())
        .await
        .unwrap();
    store
        .commit(id(1), rec.version, commit(vec![say("walrus own")]))
        .await
        .unwrap();
    let through: i64 = sqlx::query_scalar("SELECT search_through FROM threads")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(through, orch_core::search::COMMIT_CATCH_UP_EVENTS);
    // 500 old rows, and the commit's own at seq 1201, nothing between
    let seqs: Vec<i64> = sqlx::query_scalar("SELECT seq FROM message_terms ORDER BY seq")
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert_eq!(seqs.len(), 501);
    assert_eq!(seqs.first(), Some(&1));
    assert_eq!(seqs[499], 500);
    assert_eq!(seqs[500], 1201);
}
