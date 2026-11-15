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
