//! `PromptStore` on `PgStore` against a real Postgres (ADR 0070): what the conformance suite
//! (`tests/conformance.rs`) cannot say about this adapter, which is how it is built. The table's
//! CHECKs refuse a row the rules would never write; the lock of an owner is held by a transaction of
//! the test and an insert is seen to wait, and to count after it; the lock is in a key space of its
//! own; a wait is bounded and does not drain the pool; a row that is not a prompt is `Corrupt`, says
//! nothing of it and does not break anybody else's list; the shared query is one overlap with `*`
//! added to the roles. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;
use orch_store_postgres::PgPool;
use orch_store_postgres::sql::{SqlError, Tx};
use orch_store_postgres::sql::{query, query_as, query_scalar};

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use orch_core::UserId;
use orch_core::prompt::{PromptDefinition, PromptId, PromptInput};
use orch_ports::{
    MAX_PROMPTS, PromptScope, PromptStore, PromptStoreError, PromptViewer, StoredPrompt,
};
use orch_store_postgres::{
    OWNER_LOCK_CLASS, PROMPT_LOCK_TIMEOUT, PROMPT_OWNER_LOCK_CLASS, PgStore, prompt_list_sql,
};
use support::TestDb;
use uuid::Uuid;

/// A person no other test has: the advisory lock of an owner is shared by every schema of the
/// database and every test of every process, so a test that waits on one needs an owner of its own.
fn owner(name: &str) -> UserId {
    UserId::new(&format!("{name}-{}@example.test", Uuid::now_v7().simple()))
}

fn id() -> PromptId {
    PromptId::from_entropy(*Uuid::now_v7().as_bytes())
}

fn at() -> orch_core::Timestamp {
    orch_core::Timestamp::from_second(1_700_000_000).unwrap()
}

fn prompt(name: &str, audience: &[&str]) -> PromptDefinition {
    PromptDefinition::new(
        id(),
        PromptInput {
            name: name.to_owned(),
            title: format!("Title {name}"),
            description: String::new(),
            body: "Hello".to_owned(),
            variables: Vec::new(),
            audience: audience.iter().map(|s| (*s).to_owned()).collect(),
        },
    )
    .unwrap()
}

fn viewer(user: &UserId, roles: &[&str]) -> PromptViewer {
    PromptViewer {
        user: user.clone(),
        roles: roles
            .iter()
            .map(|r| (*r).to_owned())
            .collect::<BTreeSet<_>>(),
    }
}

async fn save(store: &PgStore, owner: &UserId, p: &PromptDefinition) -> StoredPrompt {
    store.insert_prompt(owner, p, at()).await.unwrap()
}

/// A transaction that holds a lock of `owner` in `class`: what an insert in the middle of its work
/// has.
async fn hold_lock(pool: &PgPool, class: i32, owner: &UserId) -> Tx {
    let mut tx = pool.begin().await.expect("begin");
    query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(class)
        .bind(owner.as_str())
        .execute(&mut *tx)
        .await
        .expect("take the owner's lock");
    tx
}

async fn waiting_for(pool: &PgPool, class: i32, owner: &UserId) -> i64 {
    query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
         AND classid::bigint = $1 AND objsubid = 2 \
         AND objid::bigint = (hashtext($2)::bigint & 4294967295)",
    )
    .bind(i64::from(class))
    .bind(owner.as_str())
    .fetch_one(pool)
    .await
    .expect("read pg_locks")
}

async fn wait_until_someone_waits_for(pool: &PgPool, owner: &UserId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while waiting_for(pool, PROMPT_OWNER_LOCK_CLASS, owner).await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a writer waits for the lock of the owner");
}

async fn insert_row(
    pool: &PgPool,
    id: &str,
    name: &str,
    body: &str,
    audience: &[&str],
) -> Result<(), SqlError> {
    query(
        "INSERT INTO user_prompts (id, owner, name, title, body, variables, audience, revision, created_at, updated_at) \
         VALUES ($1, 'by-hand@example.test', $2, 'T', $3, '[]'::jsonb, $4, nextval('user_prompt_revisions'), now(), now())",
    )
    .bind(id)
    .bind(name)
    .bind(body)
    .bind(audience.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    .execute(pool)
    .await
    .map(|_| ())
}

/// The table's CHECKs refuse a row the rules never write: an id that is not one, a name with a
/// capital, `*` beside a role, a body of 16385 bytes; and a good row goes in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_table_refuses_what_the_rules_never_write() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let pool = store.pool();
    for (what, result) in [
        (
            "an id that is not one",
            insert_row(pool, "x", "ok", "b", &[]).await,
        ),
        (
            "a name with a capital",
            insert_row(pool, "p-aaaaaaaaaaaa", "A", "b", &[]).await,
        ),
        (
            "* beside a role",
            insert_row(pool, "p-aaaaaaaaaaab", "ok", "b", &["*", "x"]).await,
        ),
        (
            "a body of 16385 bytes",
            insert_row(pool, "p-aaaaaaaaaaac", "ok", &"a".repeat(16385), &[]).await,
        ),
        (
            "an empty body",
            insert_row(pool, "p-aaaaaaaaaaad", "ok", "", &[]).await,
        ),
        (
            // 8,193 characters, 16,386 bytes: the limit is in bytes
            "a body of 16386 bytes in 8193 characters",
            insert_row(pool, "p-aaaaaaaaaaaf", "ok", &"é".repeat(8193), &[]).await,
        ),
    ] {
        let error = result.expect_err(what);
        let code = error.code().map(str::to_owned);
        assert_eq!(code.as_deref(), Some("23514"), "{what}: {error}");
    }
    insert_row(pool, "p-aaaaaaaaaaae", "ok", &"a".repeat(16384), &["*"])
        .await
        .expect("a body of exactly 16384 bytes and `*` alone");
}

/// The insert takes the owner's lock **before** it counts: a transaction holds the lock and writes
/// the hundredth prompt; an insert that arrives waits; when the holder commits it counts a hundred.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_insert_waits_for_the_lock_of_its_owner_and_counts_after_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let holder = db.pool("holder", 2).await;
    let alice = owner("alice");
    for n in 0..MAX_PROMPTS - 1 {
        save(&store, &alice, &prompt(&format!("p{n}"), &[])).await;
    }
    let mut tx = hold_lock(&holder, PROMPT_OWNER_LOCK_CLASS, &alice).await;
    let last = prompt("last", &[]);
    query(
        "INSERT INTO user_prompts (id, owner, name, title, body, variables, audience, revision, created_at, updated_at) \
         VALUES ($1, $2, 'by-hand', 'T', 'b', '[]'::jsonb, '{}', nextval('user_prompt_revisions'), now(), now())",
    )
    .bind(last.id().as_str())
    .bind(alice.as_str())
    .execute(&mut *tx)
    .await
    .expect("the holder's own insert");

    let waiting = {
        let (store, alice) = (store.clone(), alice.clone());
        tokio::spawn(async move {
            store
                .insert_prompt(&alice, &prompt("over", &[]), at())
                .await
        })
    };
    wait_until_someone_waits_for(&holder, &alice).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiting.is_finished(),
        "the insert did not wait for the owner's lock"
    );
    // the lock is the owner's: another person's insert goes through
    save(&store, &owner("bob"), &prompt("fine", &[])).await;
    tx.commit().await.expect("the holder commits");
    let outcome = waiting.await.expect("the task ran");
    assert!(
        matches!(outcome, Err(PromptStoreError::TooMany { limit: 100 })),
        "the insert counted before it had the lock: {outcome:?}"
    );
}

/// The lock is in a key space of its own: the tool servers' lock of the same owner does not block a
/// prompt insert.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tool_servers_lock_of_the_same_owner_does_not_block_a_prompt() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let holder = db.pool("holder", 2).await;
    let alice = owner("alice");
    let tx = hold_lock(&holder, OWNER_LOCK_CLASS, &alice).await;
    let started = Instant::now();
    save(&store, &alice, &prompt("free", &[])).await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the insert waited for another class"
    );
    tx.rollback().await.unwrap();
}

/// A lock held for longer than a write waits makes the insert `Unavailable` in about two seconds,
/// and does not leave the pool without a connection: the next insert works once it is released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lock_held_too_long_is_unavailable_and_does_not_drain_the_pool() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let holder = db.pool("holder", 2).await;
    let alice = owner("alice");
    let tx = hold_lock(&holder, PROMPT_OWNER_LOCK_CLASS, &alice).await;
    let started = Instant::now();
    let outcome = store
        .insert_prompt(&alice, &prompt("late", &[]), at())
        .await;
    let waited = started.elapsed();
    assert!(
        matches!(outcome, Err(PromptStoreError::Unavailable { .. })),
        "{outcome:?}"
    );
    assert!(
        waited >= PROMPT_LOCK_TIMEOUT - Duration::from_millis(300),
        "{waited:?}"
    );
    assert!(
        waited < PROMPT_LOCK_TIMEOUT + Duration::from_secs(3),
        "{waited:?}"
    );
    tx.rollback().await.unwrap();
    save(&store, &alice, &prompt("after", &[])).await;
}

/// A row whose variables are not questions is `Corrupt`, and the error holds neither the owner nor the
/// body; the owner's list (summaries, which do not parse the variables) and another person's still answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_that_is_not_a_prompt_is_corrupt_and_says_nothing_of_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let alice = owner("alice");
    let saved = save(&store, &alice, &prompt("broken", &[])).await;
    save(&store, &alice, &prompt("fine", &[])).await;
    query("UPDATE user_prompts SET body = 'SENTINEL {{ not a reference }}' WHERE id = $1")
        .bind(saved.prompt().id().as_str())
        .execute(store.pool())
        .await
        .unwrap();
    let who = viewer(&alice, &[]);
    let error = store
        .get_prompt(&who, saved.prompt().id())
        .await
        .expect_err("it does not read back");
    let PromptStoreError::Corrupt(message) = &error else {
        panic!("{error:?}");
    };
    for leaked in [alice.as_str(), "SENTINEL", "not a reference"] {
        assert!(!message.contains(leaked), "{leaked} is in {message}");
    }
    let page = store
        .list_prompts(&who, PromptScope::Mine, None, 100)
        .await
        .expect("a list does not read the body of a row");
    assert_eq!(page.items.len(), 2);
    // a variable that is not a question is corrupt too, and the list still counts it; one that is not
    // an array is refused by the table itself
    query("UPDATE user_prompts SET body = 'Hello', variables = '[1]'::jsonb WHERE id = $1")
        .bind(saved.prompt().id().as_str())
        .execute(store.pool())
        .await
        .unwrap();
    assert!(matches!(
        store.get_prompt(&who, saved.prompt().id()).await,
        Err(PromptStoreError::Corrupt(_))
    ));
    assert_eq!(
        store
            .list_prompts(&who, PromptScope::Mine, None, 100)
            .await
            .unwrap()
            .items
            .iter()
            .map(|i| i.variable_count)
            .max(),
        Some(1)
    );
    query("UPDATE user_prompts SET variables = '{}'::jsonb WHERE id = $1")
        .bind(saved.prompt().id().as_str())
        .execute(store.pool())
        .await
        .expect_err("the table's check refuses a variables that is not an array");
}

/// The shared query is one overlap with `*` added to the viewer's roles: a prompt for everybody is
/// seen by a viewer with no roles, one for a role by those who have it, and nothing leaks across.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shared_query_matches_everybody_and_the_roles_a_viewer_has() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let author = owner("author");
    let everybody = save(&store, &author, &prompt("all", &["*"])).await;
    let team = save(&store, &author, &prompt("team", &["team"])).await;
    save(&store, &author, &prompt("other", &["other"])).await;
    save(&store, &author, &prompt("private", &[])).await;
    let ids = |page: orch_ports::PromptPage| -> BTreeSet<PromptId> {
        page.items.into_iter().map(|i| i.id).collect()
    };
    let nobody = viewer(&owner("n"), &[]);
    let seen = ids(store
        .list_prompts(&nobody, PromptScope::Shared, None, 100)
        .await
        .unwrap());
    assert_eq!(seen, BTreeSet::from([everybody.prompt().id().clone()]));
    let member = viewer(&owner("m"), &["team", "user"]);
    let seen = ids(store
        .list_prompts(&member, PromptScope::All, None, 100)
        .await
        .unwrap());
    assert_eq!(
        seen,
        BTreeSet::from([everybody.prompt().id().clone(), team.prompt().id().clone()])
    );
    // the author's own listing has all four, as theirs, and the shared scope none
    let own = viewer(&author, &["team"]);
    assert_eq!(
        ids(store
            .list_prompts(&own, PromptScope::Mine, None, 100)
            .await
            .unwrap())
        .len(),
        4
    );
    assert_eq!(
        ids(store
            .list_prompts(&own, PromptScope::Shared, None, 100)
            .await
            .unwrap())
        .len(),
        0
    );
}

/// The plan Postgres chooses for a listing, as text, with sequential scans forbidden.
///
/// A table of a few rows is read by a sequential scan whatever the indexes are, so the plan of a
/// small table says nothing; with `enable_seqscan = off` the planner takes an index when one is
/// usable at all, and says which, and that is what this asks: *can* the statement use the index. It
/// is the question that the first version of the shared query got wrong (`audience && $2` does not
/// imply the partial index's predicate, so the GIN index could not be used however big the table).
async fn plan_of(store: &PgStore, scope: PromptScope) -> String {
    let mut tx = store.pool().begin().await.unwrap();
    query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let sql = format!("EXPLAIN {}", prompt_list_sql(scope));
    let rows: Vec<(String,)> = query_as(sql)
        .bind("somebody@example.test")
        .bind(vec!["team".to_owned(), "*".to_owned()])
        .bind(None::<String>)
        .bind(101_i64)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    rows.into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The shared listing can use the partial GIN index, and the other two can use the owner's index:
/// the statements state the index's own predicate beside the overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listings_can_use_their_indexes() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let shared = plan_of(&store, PromptScope::Shared).await;
    assert!(shared.contains("user_prompts_audience"), "{shared}");
    let all = plan_of(&store, PromptScope::All).await;
    assert!(all.contains("user_prompts_audience"), "{all}");
    assert!(all.contains("user_prompts_owner_id"), "{all}");
    let mine = plan_of(&store, PromptScope::Mine).await;
    assert!(mine.contains("user_prompts_owner_id"), "{mine}");
}

/// A role that cannot be in an audience (a NUL, a space, over 64 bytes) is not sent to the
/// database: Postgres cannot hold the NUL and would refuse the statement. 5,000 roles are one array.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn roles_that_cannot_be_an_audience_are_dropped_before_the_query() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store().await;
    let author = owner("author");
    let team = save(&store, &author, &prompt("team", &["team"])).await;
    let mut roles: Vec<String> = (0..5000).map(|n| format!("role-{n}")).collect();
    roles.extend([
        "nul\u{0}role".to_owned(),
        "with space".to_owned(),
        "x".repeat(65),
        "é".to_owned(),
        String::new(),
        "team".to_owned(),
    ]);
    let who = PromptViewer {
        user: owner("reader"),
        roles: roles.into_iter().collect(),
    };
    let page = store
        .list_prompts(&who, PromptScope::All, None, 100)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(&page.items[0].id, team.prompt().id());
    let got = store.get_prompt(&who, team.prompt().id()).await.unwrap();
    assert!(got.is_some());
}
