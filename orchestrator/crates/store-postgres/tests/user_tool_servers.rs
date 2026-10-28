//! `PgUserToolServers` against a real Postgres (ADR 0046): what the conformance suite
//! (`tests/conformance.rs`) cannot say about this adapter, which is how it is built. The lock of an
//! owner is held by a transaction of the test and the insert is seen to wait; the lock is in a key
//! space of its own; a wait is bounded; the writes do not depend on the isolation the database
//! defaults to; a row that is not a definition is `Corrupt` and says nothing of it. Skipped unless
//! `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::{
    CredentialRef, EgressPolicy, MAX_USER_TOOL_SERVERS, ServerRevision, UserAuth, UserToolServer,
    UserToolServerId, UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};
use orch_store_postgres::{LOCK_TIMEOUT, OWNER_LOCK_CLASS, PgStore, PgUserToolServers};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use support::TestDb;
use uuid::Uuid;

/// A person no other test has: the advisory lock of an owner is shared by every schema of the
/// database and every test of every process, so a test that waits on one needs an owner of its own.
fn owner(name: &str) -> UserId {
    UserId::new(&format!("{name}-{}@example.test", Uuid::now_v7().simple()))
}

fn definition(suffix: &str) -> UserToolServer {
    definition_at(
        suffix,
        &format!("Server {suffix}"),
        "https://mcp.example.com/mcp",
    )
}

fn definition_at(suffix: &str, name: &str, url: &str) -> UserToolServer {
    let target = EgressPolicy::new()
        .check_url(url)
        .expect("a URL the guard reads");
    UserToolServer::new(
        UserToolServerId::parse(&format!("u-{suffix}")).expect("an id"),
        name,
        &target,
        UserAuth::Bearer,
    )
    .expect("a definition")
}

fn id_of(n: usize) -> String {
    format!("{n:010}")
}

/// A migrated schema and the store over a pool of `max` connections that waits at most three
/// seconds for one.
async fn store_with(db: &TestDb, max: u32, settings: &[(&str, &str)]) -> PgUserToolServers {
    // the thread store's migrations make the table, on the direct connection: no pooled session
    // exists before the settings below are the database's
    PgStore::migrate_direct(&db.pinned_direct_url())
        .await
        .expect("migrate");
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(3))
        .connect_with(
            db.options_with(settings)
                .await
                .application_name("orch-user-tool-servers-test"),
        )
        .await
        .expect("connect pool");
    PgUserToolServers::from_pool(pool)
}

async fn store(db: &TestDb) -> PgUserToolServers {
    store_with(db, 8, &[]).await
}

/// A transaction that holds the lock of `owner`: what an insert in the middle of its work has.
async fn hold_owner_lock(pool: &PgPool, owner: &UserId) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(OWNER_LOCK_CLASS)
        .bind(owner.as_str())
        .execute(&mut *tx)
        .await
        .expect("take the owner's lock");
    tx
}

/// How many backends wait for the advisory lock of `owner`, as a two-`int4` lock (`objsubid` 2) of
/// the class of this store.
async fn waiting_for(pool: &PgPool, owner: &UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
         AND classid::bigint = $1 AND objsubid = 2 \
         AND objid::bigint = (hashtext($2)::bigint & 4294967295)",
    )
    .bind(i64::from(OWNER_LOCK_CLASS))
    .bind(owner.as_str())
    .fetch_one(pool)
    .await
    .expect("read pg_locks")
}

async fn wait_until_someone_waits_for(pool: &PgPool, owner: &UserId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while waiting_for(pool, owner).await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a writer waits for the lock of the owner");
}

/// The insert takes the owner's lock **before** it counts. A transaction holds the lock and, inside
/// it, writes the eighth definition; an insert that arrives waits; when the holder commits, the
/// insert counts eight and is refused. A count taken before the lock would read seven, and the
/// ninth would be added.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_insert_waits_for_the_lock_of_its_owner_and_counts_after_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let holder = db.pool("holder", 2).await;
    let owner = owner("alice");
    for n in 0..MAX_USER_TOOL_SERVERS - 1 {
        store
            .insert(&owner, &definition(&id_of(n)))
            .await
            .expect("insert");
    }

    let mut tx = hold_owner_lock(&holder, &owner).await;
    let eighth = definition(&id_of(MAX_USER_TOOL_SERVERS - 1));
    sqlx::query(
        "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, revision) \
         VALUES ($1, $2, 'by hand', $3, '{\"kind\":\"none\"}', 'added', nextval('user_tool_server_revisions'))",
    )
    .bind(owner.as_str())
    .bind(eighth.id().as_str())
    .bind(eighth.url())
    .execute(&mut *tx)
    .await
    .expect("the holder's own insert");

    let ninth = definition(&id_of(MAX_USER_TOOL_SERVERS));
    let waiting = {
        let (store, owner, ninth) = (store.clone(), owner.clone(), ninth.clone());
        tokio::spawn(async move { store.insert(&owner, &ninth).await })
    };
    wait_until_someone_waits_for(&holder, &owner).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !waiting.is_finished(),
        "the insert did not wait for the owner's lock"
    );

    // the lock is the owner's: another person's insert, and every read, go through
    let other = self::owner("bob");
    store
        .insert(&other, &definition(&id_of(0)))
        .await
        .expect("another owner is not blocked");
    assert_eq!(
        store
            .list(&owner)
            .await
            .expect("a read is not blocked")
            .len(),
        7
    );
    assert!(!waiting.is_finished());

    tx.commit().await.expect("the holder commits");
    let outcome = waiting.await.expect("the task ran");
    assert!(
        matches!(outcome, Err(UserToolServerStoreError::TooMany { limit: 8 })),
        "the insert counted before it had the lock: {outcome:?}"
    );
    assert_eq!(store.list(&owner).await.expect("list").len(), 8);
    assert!(store.get(&owner, ninth.id()).await.expect("get").is_none());
}

/// The same lock, released by a rollback: the waiting insert goes ahead and finds seven.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_insert_goes_ahead_when_the_holder_of_the_lock_rolls_back() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let holder = db.pool("holder", 2).await;
    let owner = owner("alice");

    let tx = hold_owner_lock(&holder, &owner).await;
    let waiting = {
        let (store, owner) = (store.clone(), owner.clone());
        tokio::spawn(async move { store.insert(&owner, &definition("0000000001")).await })
    };
    wait_until_someone_waits_for(&holder, &owner).await;
    tx.rollback().await.expect("rollback");
    waiting
        .await
        .expect("the task ran")
        .expect("the insert went ahead");
    assert_eq!(store.list(&owner).await.expect("list").len(), 1);
}

/// The lock is a **two-`int4`** advisory lock of a class of its own, so it can never be the lock of
/// a `bigint` key: the watch keys of the thread store, the secret store's, the migrators'. The proof:
/// a transaction holds a single-`bigint` lock whose 64 bits are this lock's two numbers side by side,
/// and an insert for that owner is not delayed by it; while the lock proper does delay it (the other
/// tests), and `pg_locks` says `objsubid` 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lock_of_an_owner_collides_with_no_lock_of_a_bigint_key() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let holder = db.pool("holder", 3).await;
    let owner = owner("alice");

    // class in the high 32 bits, the owner's hash in the low 32: the bigint that `classid` and
    // `objid` would show for the lock of the owner, were it a one-key lock
    let same_bits: i64 =
        sqlx::query_scalar("SELECT ($1::bigint << 32) | (hashtext($2)::bigint & 4294967295)")
            .bind(i64::from(OWNER_LOCK_CLASS))
            .bind(owner.as_str())
            .fetch_one(&holder)
            .await
            .expect("the bits");
    let mut bigint = holder.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(same_bits)
        .execute(&mut *bigint)
        .await
        .expect("a one-key lock");

    // no timeout of the test's own: a lock that delayed the insert would hold it for `LOCK_TIMEOUT`
    // and end it as `Unavailable`, which fails the `expect` just the same, and a slow runner cannot
    store
        .insert(&owner, &definition("0000000001"))
        .await
        .expect("a one-key lock with the same bits delayed the insert");

    // and the lock proper is what the insert waits for, and it is a two-key lock
    let tx = hold_owner_lock(&holder, &owner).await;
    let waiting = {
        let (store, owner) = (store.clone(), owner.clone());
        tokio::spawn(async move { store.insert(&owner, &definition("0000000002")).await })
    };
    wait_until_someone_waits_for(&holder, &owner).await;
    // both are granted at once, with the same class and object numbers: one is the test's one-key
    // lock (`objsubid` 1) and the other the owner's, which is a two-key lock (`objsubid` 2)
    let mut granted: Vec<i16> = sqlx::query_scalar(
        "SELECT objsubid FROM pg_locks WHERE locktype = 'advisory' AND granted \
         AND classid::bigint = $1 AND objid::bigint = (hashtext($2)::bigint & 4294967295)",
    )
    .bind(i64::from(OWNER_LOCK_CLASS))
    .bind(owner.as_str())
    .fetch_all(&holder)
    .await
    .expect("pg_locks");
    granted.sort_unstable();
    assert_eq!(granted, [1, 2], "the owner's lock is a two-int4 lock");
    tx.rollback().await.expect("rollback");
    waiting.await.expect("the task ran").expect("insert");
    bigint.rollback().await.expect("rollback");
}

/// A wait for the owner's lock is bounded: after `LOCK_TIMEOUT` the insert is `Unavailable` (class
/// `Transient`), says it was locked, names nobody, and the connection is back in the pool (a pool of
/// one serves a read straight after).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_for_the_lock_of_the_owner_ends_as_unavailable_and_gives_the_connection_back() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 1, &[]).await;
    let holder = db.pool("holder", 2).await;
    let owner = owner("alice");
    let tx = hold_owner_lock(&holder, &owner).await;

    let started = std::time::Instant::now();
    // The lock is held for as long as the test wants, so only `lock_timeout` can end the wait: the
    // outer bound is far past it, for a runner that is slow and not for a wait that never ends.
    let error = tokio::time::timeout(
        LOCK_TIMEOUT * 15,
        store.insert(&owner, &definition("0000000001")),
    )
    .await
    .expect("the insert waited for the lock for longer than a write may")
    .expect_err("the lock is held for longer than a write waits");
    let waited = started.elapsed();
    // the lower bound is what proves a wait (a refusal at once would be no timeout); there is no
    // upper bound beyond the one above, because a loaded runner can make a two second wait take
    // much longer, and the wait being bounded is what the outer timeout shows
    assert!(
        waited >= LOCK_TIMEOUT.saturating_sub(Duration::from_millis(200)),
        "it waited {waited:?}, not about {LOCK_TIMEOUT:?}"
    );
    assert!(
        matches!(error, UserToolServerStoreError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Transient);
    assert!(error.to_string().contains("locked"), "{error}");
    assert!(!error.to_string().contains(owner.as_str()), "{error}");

    // the only connection of the pool was given back, with the transaction rolled back
    store
        .get(&owner, &UserToolServerId::parse("u-0000000001").unwrap())
        .await
        .expect("the pool serves a read after the timed out write");
    tx.rollback().await.expect("rollback");
    store
        .insert(&owner, &definition("0000000001"))
        .await
        .expect("and a write once the lock is free");
}

/// A write waits for a row that somebody's transaction holds no longer than for an owner's lock:
/// `replace`, `set_state` and `delete` of a locked row are `Unavailable` after `LOCK_TIMEOUT`,
/// nothing is written, and when the lock is let go they work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_that_waits_for_a_row_gives_up_as_unavailable_and_changes_nothing() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let holder = db.pool("holder", 2).await;
    let owner = owner("alice");
    let first = definition("0000000001");
    let revision = store.insert(&owner, &first).await.expect("insert");

    let mut tx = holder.begin().await.expect("begin");
    sqlx::query("SELECT 1 FROM user_tool_servers WHERE owner = $1 AND id = $2 FOR UPDATE")
        .bind(owner.as_str())
        .bind(first.id().as_str())
        .execute(&mut *tx)
        .await
        .expect("lock the row");

    let edit = definition_at("0000000001", "Edited", "https://mcp.example.com/mcp");
    let (replaced, stated, deleted) = tokio::time::timeout(LOCK_TIMEOUT * 5, async {
        tokio::join!(
            store.replace(&owner, &edit, revision),
            store.set_state(&owner, first.id(), UserToolServerState::Connected, revision),
            store.delete(&owner, first.id()),
        )
    })
    .await
    .expect("a write waited for a row for longer than a write may");
    for outcome in [replaced.map(|_| ()), stated.map(|_| ()), deleted] {
        let error = outcome.expect_err("the row is locked for longer than a write waits");
        assert!(
            matches!(error, UserToolServerStoreError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
        assert!(error.to_string().contains("locked"), "{error}");
    }
    tx.rollback().await.expect("rollback");

    let kept = store.get(&owner, first.id()).await.unwrap().unwrap();
    assert_eq!(
        (kept.server(), kept.revision()),
        (&first, revision),
        "nothing was written"
    );
    store
        .set_state(&owner, first.id(), UserToolServerState::Connected, revision)
        .await
        .expect("the row is free");
}

/// The writes do not depend on what the database defaults to. With `REPEATABLE READ` as the default
/// (`ALTER ROLE ... SET default_transaction_isolation`), the snapshot would be taken before a wait
/// for a lock and a writer that waited would fail with SQLSTATE `40001`, which reads as
/// `Unavailable`. Every race of the conformance suite is run here, and every loser must be told what
/// it lost to: `Conflict`, `Exists`, `TooMany`, `NotFound`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writers_do_not_depend_on_the_isolation_the_database_defaults_to() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(
        &db,
        24,
        &[("default_transaction_isolation", "repeatable read")],
    )
    .await;
    let level: String = sqlx::query_scalar("SHOW default_transaction_isolation")
        .fetch_one(store.pool())
        .await
        .expect("show");
    assert_eq!(
        level, "repeatable read",
        "the pool's sessions default to it"
    );

    let store = std::sync::Arc::new(store);
    for round in 0..10 {
        // sixteen adds for eight places
        let limit_owner = owner("limit");
        let tasks: Vec<_> = (0..2 * MAX_USER_TOOL_SERVERS)
            .map(|n| {
                let (store, owner) = (store.clone(), limit_owner.clone());
                tokio::spawn(async move { store.insert(&owner, &definition(&id_of(n))).await })
            })
            .collect();
        let (mut added, mut refused) = (0, 0);
        for task in tasks {
            match task.await.expect("the task ran") {
                Ok(_) => added += 1,
                Err(UserToolServerStoreError::TooMany { .. }) => refused += 1,
                Err(other) => panic!("round {round}: neither added nor too many: {other:?}"),
            }
        }
        assert_eq!((added, refused), (8, 8), "round {round}");

        // eight edits from one revision
        let edit_owner = owner("edit");
        let first = definition("0000000001");
        let r1 = store.insert(&edit_owner, &first).await.expect("insert");
        let tasks: Vec<_> = (0..8)
            .map(|n| {
                let (store, who) = (store.clone(), edit_owner.clone());
                tokio::spawn(async move {
                    let mine = definition_at(
                        "0000000001",
                        &format!("Writer {n}"),
                        "https://mcp.example.com/mcp",
                    );
                    store.replace(&who, &mine, r1).await
                })
            })
            .collect();
        let mut winners = 0;
        for task in tasks {
            match task.await.expect("the task ran") {
                Ok(_) => winners += 1,
                Err(UserToolServerStoreError::Conflict) => {}
                Err(other) => panic!("round {round}: neither replaced nor a conflict: {other:?}"),
            }
        }
        assert_eq!(winners, 1, "round {round}");

        // edits and a delete of one revision
        let r2 = store
            .get(&edit_owner, first.id())
            .await
            .unwrap()
            .unwrap()
            .revision();
        let (a, b, c) = tokio::join!(
            store.set_state(&edit_owner, first.id(), UserToolServerState::Connected, r2),
            store.replace(&edit_owner, &first, r2),
            store.delete(&edit_owner, first.id()),
        );
        for outcome in [a.map(|_| ()), b.map(|_| ())] {
            assert!(
                matches!(
                    outcome,
                    Ok(())
                        | Err(
                            UserToolServerStoreError::Conflict | UserToolServerStoreError::NotFound
                        )
                ),
                "round {round}: {outcome:?}"
            );
        }
        c.expect("delete");
        assert!(
            store.get(&edit_owner, first.id()).await.unwrap().is_none(),
            "round {round}"
        );
    }
}

/// A revision comes from the database's sequence, not from the process: a second instance over the
/// same database (another replica, or this one after a restart) goes on from where the first left
/// off, and never from the beginning.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revisions_go_on_across_instances() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let first = store(&db).await;
    let second = PgUserToolServers::from_pool(db.pool("another-replica", 2).await);
    let (alice, bob) = (owner("alice"), owner("bob"));

    let r1 = first
        .insert(&alice, &definition("0000000001"))
        .await
        .unwrap();
    let r2 = second
        .insert(&bob, &definition("0000000001"))
        .await
        .unwrap();
    let r3 = second
        .set_state(
            &alice,
            definition("0000000001").id(),
            UserToolServerState::Connected,
            r1,
        )
        .await
        .expect("a replica writes what another read");
    let r4 = first
        .set_state(
            &alice,
            definition("0000000001").id(),
            UserToolServerState::Revoked,
            r3,
        )
        .await
        .unwrap();
    assert!(r1 < r2 && r2 < r3 && r3 < r4, "{r1} {r2} {r3} {r4}");
    // the sequence is the table's, not an owner's
    assert_ne!(r1, r2);
}

/// A row by hand: what a repair, or another build, would write.
struct Row<'a> {
    id: &'a str,
    name: &'a str,
    auth: &'a str,
    state: &'a str,
    credential: Option<&'a str>,
    revision: i64,
}

impl Default for Row<'_> {
    fn default() -> Self {
        Row {
            id: "u-0000000002",
            name: "by hand",
            auth: r#"{"kind":"none"}"#,
            state: "added",
            credential: None,
            revision: 2,
        }
    }
}

async fn insert_by_hand(pool: &PgPool, row: Row<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, credential, revision) \
         VALUES ('x@example.test', $1, $2, 'https://mcp.example.com/mcp', $3::jsonb, $4, $5, $6)",
    )
    .bind(row.id)
    .bind(row.name)
    .bind(row.auth)
    .bind(row.state)
    .bind(row.credential)
    .bind(row.revision)
    .execute(pool)
    .await
    .map(|_| ())
}

/// What the table refuses by itself: a row written by hand (a repair, another build) that is not
/// shaped as a definition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_table_refuses_a_row_that_cannot_be_a_definition() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let pool = store.pool();
    // the baseline is accepted, so that each refusal below is about the one thing it changes
    insert_by_hand(
        pool,
        Row {
            id: "u-0000000001",
            revision: 1,
            ..Row::default()
        },
    )
    .await
    .expect("a definition");
    insert_by_hand(pool, Row::default())
        .await
        .expect("the baseline of the refusals");
    sqlx::query("DELETE FROM user_tool_servers WHERE id = 'u-0000000002'")
        .execute(pool)
        .await
        .unwrap();
    let long_name = "n".repeat(81);
    for (what, row) in [
        (
            "an id that is not u- and ten characters",
            Row {
                id: "u-short",
                ..Row::default()
            },
        ),
        (
            "an id in capitals",
            Row {
                id: "u-ABCDEFGHIJ",
                ..Row::default()
            },
        ),
        (
            "a state that is none",
            Row {
                state: "unavailable",
                ..Row::default()
            },
        ),
        (
            "a name that is empty",
            Row {
                name: "",
                ..Row::default()
            },
        ),
        (
            "a name of 81 characters",
            Row {
                name: &long_name,
                ..Row::default()
            },
        ),
        (
            "a kind that is not an object",
            Row {
                auth: "\"none\"",
                ..Row::default()
            },
        ),
        (
            "an empty reference",
            Row {
                credential: Some(""),
                ..Row::default()
            },
        ),
        (
            "a revision of zero",
            Row {
                revision: 0,
                ..Row::default()
            },
        ),
        (
            "the same id twice for one owner",
            Row {
                id: "u-0000000001",
                ..Row::default()
            },
        ),
    ] {
        assert!(
            insert_by_hand(pool, row).await.is_err(),
            "the table accepted {what}"
        );
    }
}

/// A row that the table accepts and that is not a definition is `Corrupt` for `get` and for `list`,
/// and the sentence holds nothing of it: not the URL (which can carry a secret in its path or a
/// query), not the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_that_is_not_a_definition_is_corrupt_and_the_message_holds_nothing_of_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let secret = "s3cr3t-in-the-url";
    let cases: [(&str, String, &str, &str, Option<&str>); 8] = [
        (
            "a URL with a query",
            format!("https://mcp.example.com/mcp?token={secret}"),
            r#"{"kind":"none"}"#,
            "added",
            None,
        ),
        (
            "a URL that is not the normal form",
            format!("https://MCP.example.com/{secret}/../x"),
            r#"{"kind":"none"}"#,
            "added",
            None,
        ),
        (
            "a URL with user information",
            format!("https://user:{secret}@mcp.example.com/mcp"),
            r#"{"kind":"none"}"#,
            "added",
            None,
        ),
        (
            "a URL of another scheme",
            format!("ftp://mcp.example.com/{secret}"),
            r#"{"kind":"none"}"#,
            "added",
            None,
        ),
        (
            "a kind nobody knows",
            "https://mcp.example.com/mcp".to_owned(),
            r#"{"kind":"carrier_pigeon"}"#,
            "added",
            None,
        ),
        (
            "a reserved header",
            "https://mcp.example.com/mcp".to_owned(),
            r#"{"kind":"api_key","header":"Host"}"#,
            "added",
            None,
        ),
        (
            "a reference with a space",
            "https://mcp.example.com/mcp".to_owned(),
            r#"{"kind":"bearer"}"#,
            "connected",
            Some("not a ref"),
        ),
        (
            "a scope with a quote",
            "https://mcp.example.com/mcp".to_owned(),
            r#"{"kind":"oauth2","scopes":["a\"b"],"client_id":null}"#,
            "added",
            None,
        ),
    ];
    for (n, (what, url, auth, state, credential)) in cases.into_iter().enumerate() {
        let owner = owner("corrupt");
        let id = format!("u-{}", id_of(n));
        sqlx::query(
            "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, credential, revision) \
             VALUES ($1, $2, 'Hand made', $3, $4::jsonb, $5, $6, nextval('user_tool_server_revisions'))",
        )
        .bind(owner.as_str())
        .bind(&id)
        .bind(&url)
        .bind(auth)
        .bind(state)
        .bind(credential)
        .execute(store.pool())
        .await
        .unwrap_or_else(|e| panic!("the table refused {what}: {e}"));

        let id = UserToolServerId::parse(&id).unwrap();
        let from_get = store.get(&owner, &id).await.expect_err(what);
        let from_list = store.list(&owner).await.expect_err(what);
        for error in [from_get, from_list] {
            assert!(
                matches!(error, UserToolServerStoreError::Corrupt(_)),
                "{what}: {error:?}"
            );
            assert_eq!(error.class(), ErrorClass::Corrupt, "{what}");
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains(secret), "{what}: {shown}");
            assert!(!shown.contains(owner.as_str()), "{what}: {shown}");
            assert!(!shown.contains("mcp.example.com"), "{what}: {shown}");
        }
        // and it can still be removed
        store
            .delete(&owner, &id)
            .await
            .expect("a corrupt row can be deleted");
        assert!(store.get(&owner, &id).await.expect("gone").is_none());
    }
}

/// What the logs of the thread that runs a test say.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The error says no id (the port's has none), so an operator is told it in the log: one warning per
/// row that does not read back, by its id, for a `get` and for every such row of a `list`, and the
/// warning holds nothing else of the row. A current-thread runtime, so that the subscriber of this
/// thread is the one the store logs to.
#[tokio::test]
async fn a_row_that_does_not_read_back_is_warned_of_by_its_id_and_by_nothing_else() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("warned");
    let secret = "s3cr3t-in-the-url";
    let url_with_secret = format!("https://mcp.example.com/mcp?token={secret}");
    // (id, url, kind): one that reads back, one with a URL that is not a definition's, one with a
    // kind nobody knows
    for (n, (url, auth)) in [
        ("https://mcp.example.com/mcp", r#"{"kind":"none"}"#),
        (url_with_secret.as_str(), r#"{"kind":"none"}"#),
        (
            "https://mcp.example.com/mcp",
            r#"{"kind":"carrier_pigeon"}"#,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, revision) \
             VALUES ($1, $2, 'Hand made', $3, $4::jsonb, 'added', nextval('user_tool_server_revisions'))",
        )
        .bind(owner.as_str())
        .bind(format!("u-{}", id_of(n)))
        .bind(url)
        .bind(auth)
        .execute(store.pool())
        .await
        .expect("a row");
    }
    let id = |n| UserToolServerId::parse(&format!("u-{}", id_of(n))).unwrap();

    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // a row that reads back says nothing
    store.get(&owner, &id(0)).await.expect("a definition");
    assert!(captured.text().is_empty(), "{}", captured.text());

    // a `get` of a row that does not: one warning, by its id
    let error = store.get(&owner, &id(1)).await.expect_err("corrupt");
    assert!(
        matches!(error, UserToolServerStoreError::Corrupt(_)),
        "{error:?}"
    );
    let log = captured.text();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.contains("WARN") && log.contains(id(1).as_str()),
        "{log}"
    );

    // a `list` fails as a whole for the first such row and warns of every one
    let before = captured.text().lines().count();
    let error = store.list(&owner).await.expect_err("corrupt");
    assert!(
        matches!(error, UserToolServerStoreError::Corrupt(_)),
        "{error:?}"
    );
    let log = captured.text();
    let listed: Vec<&str> = log.lines().skip(before).collect();
    assert_eq!(listed.len(), 2, "{log}");
    assert!(listed[0].contains(id(1).as_str()), "{log}");
    assert!(listed[1].contains(id(2).as_str()), "{log}");
    assert!(
        listed.iter().all(|line| !line.contains(id(0).as_str())),
        "the row that reads back is named: {log}"
    );

    // and nothing else of any row is in the log: not the owner, the name, the URL or the secret
    for leaked in [owner.as_str(), "Hand made", "mcp.example.com", secret] {
        assert!(!log.contains(leaked), "{leaked} is in the log: {log}");
    }
}

/// A row that today's lists would refuse is still a definition: the policy changes, and a person must
/// still be able to list the server, see that it is unavailable, and delete it. (The connection
/// applies the lists every time.) A private address, a plain `http` URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_todays_lists_refuse_still_reads_back() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("alice");
    for (n, url) in [
        "https://10.0.0.5/mcp",
        "http://mcp.example.com/mcp",
        "https://localhost/mcp",
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, revision) \
             VALUES ($1, $2, 'Older', $3, '{\"kind\":\"bearer\"}', 'connected', nextval('user_tool_server_revisions'))",
        )
        .bind(owner.as_str())
        .bind(format!("u-{}", id_of(n)))
        .bind(url)
        .execute(store.pool())
        .await
        .expect("a row");
    }
    let listed = store
        .list(&owner)
        .await
        .expect("a policy that moved does not make a row corrupt");
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].server().url(), "https://10.0.0.5/mcp");
    for stored in &listed {
        store
            .delete(&owner, stored.server().id())
            .await
            .expect("delete");
    }
    assert!(store.list(&owner).await.unwrap().is_empty());
}

/// The credential reference, the kind with its parts and every state are stored and come back whole,
/// in the columns the migration lays out (and `created_at` stays while `updated_at` moves).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_columns_hold_what_the_port_writes() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("alice");
    let oauth = UserToolServer::new(
        UserToolServerId::parse("u-0000000001").unwrap(),
        "Notion",
        &EgressPolicy::new()
            .check_url("https://mcp.notion.example/v1/mcp")
            .unwrap(),
        UserAuth::oauth2(vec!["read".into(), "write".into()], Some("client-1".into())).unwrap(),
    )
    .unwrap()
    .with_state(UserToolServerState::Connected)
    .with_credential(CredentialRef::new("cred-u-0000000001-v7").unwrap());
    let revision = store.insert(&owner, &oauth).await.unwrap();

    let (kind, credential, state, created, updated): (
        String,
        Option<String>,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT auth->>'kind', credential, state, created_at::text, updated_at::text \
             FROM user_tool_servers WHERE owner = $1 AND id = $2",
    )
    .bind(owner.as_str())
    .bind("u-0000000001")
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), credential.as_deref(), state.as_str()),
        ("oauth2", Some("cred-u-0000000001-v7"), "connected")
    );

    tokio::time::sleep(Duration::from_millis(20)).await;
    store
        .set_state(
            &owner,
            oauth.id(),
            UserToolServerState::NeedsReauth,
            revision,
        )
        .await
        .unwrap();
    let (created_after, updated_after): (String, String) = sqlx::query_as(
        "SELECT created_at::text, updated_at::text FROM user_tool_servers WHERE owner = $1 AND id = $2",
    )
    .bind(owner.as_str())
    .bind("u-0000000001")
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(created, created_after, "a write does not touch created_at");
    assert_ne!(updated, updated_after, "a write moves updated_at");
    let got = store.get(&owner, oauth.id()).await.unwrap().unwrap();
    assert_eq!(
        got.server(),
        &oauth.clone().with_state(UserToolServerState::NeedsReauth)
    );
}

/// A store whose pool is closed fails every call as a database that cannot be reached does
/// (`Unavailable`, class `Transient`), and the sentence says nothing of the definition: no owner, id
/// or URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_pool_is_unavailable_and_says_nothing_of_the_definition() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("alice");
    let first = definition_at(
        "0000000001",
        "Private name",
        "https://mcp.example.com/path-with-a-secret",
    );
    let revision = store.insert(&owner, &first).await.unwrap();
    store.pool().close().await;

    let results = [
        store.insert(&owner, &first).await.map(|_| ()),
        store.get(&owner, first.id()).await.map(|_| ()),
        store.list(&owner).await.map(|_| ()),
        store.replace(&owner, &first, revision).await.map(|_| ()),
        store
            .set_state(&owner, first.id(), UserToolServerState::Revoked, revision)
            .await
            .map(|_| ()),
        store.delete(&owner, first.id()).await,
    ];
    for result in results {
        let error = result.expect_err("the pool is closed");
        assert!(
            matches!(error, UserToolServerStoreError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
        let shown = format!("{error} {error:?}");
        for private in [
            owner.as_str(),
            "u-0000000001",
            "Private name",
            "path-with-a-secret",
        ] {
            assert!(!shown.contains(private), "{shown}");
        }
    }
}

/// A revision the port cannot have given (it does not fit the column) matches nothing: `Conflict`
/// for a definition that is there, `NotFound` for one that is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revision_that_does_not_fit_the_column_is_a_conflict() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("alice");
    let first = definition("0000000001");
    store.insert(&owner, &first).await.unwrap();
    let huge = ServerRevision::new(u64::MAX);
    let error = store
        .replace(&owner, &first, huge)
        .await
        .expect_err("no such revision");
    assert!(
        matches!(error, UserToolServerStoreError::Conflict),
        "{error:?}"
    );
    let error = store
        .set_state(&owner, first.id(), UserToolServerState::Revoked, huge)
        .await
        .expect_err("no such revision");
    assert!(
        matches!(error, UserToolServerStoreError::Conflict),
        "{error:?}"
    );
    let error = store
        .set_state(
            &owner,
            &UserToolServerId::parse("u-00000000zz").unwrap(),
            UserToolServerState::Revoked,
            huge,
        )
        .await
        .expect_err("no such server");
    assert!(
        matches!(error, UserToolServerStoreError::NotFound),
        "{error:?}"
    );
}

/// The ninth server of a person is refused whichever way the eight came to be, and a person whose
/// eight include one that is corrupt still gets the limit from the count (the count reads no
/// definition).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_limit_counts_rows_and_not_readable_definitions() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    let owner = owner("alice");
    for n in 0..MAX_USER_TOOL_SERVERS - 1 {
        store.insert(&owner, &definition(&id_of(n))).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO user_tool_servers (owner, id, name, url, auth, state, revision) \
         VALUES ($1, $2, 'Hand made', 'ftp://nowhere/', '{\"kind\":\"none\"}', 'added', nextval('user_tool_server_revisions'))",
    )
    .bind(owner.as_str())
    .bind(format!("u-{}", id_of(MAX_USER_TOOL_SERVERS - 1)))
    .execute(store.pool())
    .await
    .unwrap();
    let error = store
        .insert(&owner, &definition(&id_of(MAX_USER_TOOL_SERVERS)))
        .await
        .expect_err("eight rows are eight");
    assert!(
        matches!(error, UserToolServerStoreError::TooMany { limit: 8 }),
        "{error:?}"
    );
}

/// A definition with the broker's reference to a credential, as the broker's `store` leaves it.
fn with_credential(suffix: &str) -> UserToolServer {
    definition(suffix)
        .with_credential(CredentialRef::new(&format!("cred-u-{suffix}-v1")).expect("a reference"))
}

/// The count that boot reads in a deployment that took the `secrets` section away: how many
/// definitions hold a credential, over every owner, and only that number. Not a port method: the
/// port does not change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_definitions_that_hold_a_credential_are_counted_over_every_owner() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    assert_eq!(store.count_with_credential().await.expect("count"), 0);

    let (alice, bob) = (owner("alice"), owner("bob"));
    store
        .insert(&alice, &definition("0000000001"))
        .await
        .expect("insert");
    assert_eq!(
        store.count_with_credential().await.expect("count"),
        0,
        "a definition that holds none is not counted"
    );
    store
        .insert(&alice, &with_credential("0000000002"))
        .await
        .expect("insert");
    store
        .insert(&alice, &with_credential("0000000003"))
        .await
        .expect("insert");
    store
        .insert(&bob, &with_credential("0000000002"))
        .await
        .expect("the same id for another owner");
    store
        .insert(&bob, &definition("0000000004"))
        .await
        .expect("insert");
    assert_eq!(
        store.count_with_credential().await.expect("count"),
        3,
        "three hold a credential, over both owners"
    );

    // a credential that is set by an edit is counted, one that is gone with its definition is not
    let first = store.get(&alice, definition("0000000001").id()).await;
    let first = first.expect("get").expect("there");
    store
        .replace(
            &alice,
            &first
                .server()
                .clone()
                .with_credential(CredentialRef::new("cred-u-0000000001-v1").expect("a reference")),
            first.revision(),
        )
        .await
        .expect("replace");
    assert_eq!(store.count_with_credential().await.expect("count"), 4);
    store
        .delete(&alice, definition("0000000002").id())
        .await
        .expect("delete");
    assert_eq!(store.count_with_credential().await.expect("count"), 3);
}

/// A closed pool is `Unavailable`, as for every other call: the count is a read like the others.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_count_of_credentials_is_unavailable_when_the_pool_is_closed() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    store.pool().close().await;
    let error = store.count_with_credential().await.expect_err("closed");
    assert!(
        matches!(error, UserToolServerStoreError::Unavailable { .. }),
        "{error:?}"
    );
}

/// The owner is the person's e-mail, never empty: `UserId::new` trims, and both authenticators refuse
/// an identity that is empty after it, so a row with none is a bug or a repair gone wrong, and the
/// table says so. (`UserId` has no length cap and no rule about case that the database could
/// repeat: see the migration.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_table_refuses_an_owner_that_is_empty() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store(&db).await;
    // what `UserId::new` makes of white space alone
    let nobody = UserId::new("  \t ");
    assert_eq!(nobody.as_str(), "");
    store
        .insert(&nobody, &definition("0000000001"))
        .await
        .expect_err("a definition of nobody");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM user_tool_servers WHERE owner = ''")
        .fetch_one(store.pool())
        .await
        .expect("count");
    assert_eq!(rows, 0, "nothing was written");
    // and a person is still a person
    store
        .insert(&owner("alice"), &definition("0000000001"))
        .await
        .expect("insert");
}
