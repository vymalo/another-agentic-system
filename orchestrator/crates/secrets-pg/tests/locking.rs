//! The lock that serializes the writers of a key, and what it costs when it is held: every write
//! waits for it, a wait is bounded, and the writes do not depend on the database's defaults.
//! Skipped unless `ORCH_TEST_DATABASE_URL` is set.
//!
//! "Another writer is in the middle of its write" is a raw transaction that holds the key's
//! advisory lock. The tests of this file take their owners from their database (`TestDb::owner`),
//! so the key's lock is theirs alone, and they wait for a session blocked **on that lock** in
//! `pg_locks`, not for a sleep.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::{Duration, Instant};

use orch_core::{Classify as _, ErrorClass};
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore as _, UserToolServerId};
use orch_secrets_pg::{LOCK_TIMEOUT, PgSecretStore, advisory_lock_key};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use support::{
    TestDb, hold_lock, holders, ring, ring_k1, wait_for_a_waiter, wait_for_waiters, waiters,
};
use tokio::task::JoinHandle;

fn credential(n: u32) -> SecretServer {
    SecretServer::credential(&UserToolServerId::parse(&format!("u-{n:010}")).unwrap())
}

fn material(text: &str) -> SecretMaterial {
    SecretMaterial::new(text.as_bytes()).unwrap()
}

/// The first thing a test checks about a write: that it queues behind the lock and stays there.
async fn queues_and_stays_queued<T>(raw: &PgPool, key: i64, call: &JoinHandle<T>) {
    wait_for_a_waiter(raw, key).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !call.is_finished(),
        "the call went on without the lock of its key"
    );
}

/// A row as an in-flight `put` has just written it, in the transaction that holds the lock and has
/// not committed.
async fn write_a_row_in(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    owner: &str,
    server: &SecretServer,
    version: i64,
) {
    sqlx::query(
        "INSERT INTO user_secrets (owner, server, version, v, kid, nonce, ciphertext) \
         VALUES ($1, $2, $3, 1, 'k1', $4, $5)",
    )
    .bind(owner)
    .bind(server.to_string())
    .bind(version)
    .bind(vec![0_u8; 12])
    .bind(vec![0_u8; 16])
    .execute(&mut **tx)
    .await
    .unwrap();
}

async fn rows(raw: &PgPool) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_secrets")
        .fetch_one(raw)
        .await
        .unwrap();
    count
}

/// What makes a revoke linearizable against a `put` that is under way: without the lock, the delete
/// finds no row, succeeds, and the put commits the credential back.
#[tokio::test]
async fn a_delete_waits_for_the_lock_of_its_key_and_removes_what_the_writer_ahead_of_it_wrote() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store(ring_k1()).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    let (alice, server) = (db.owner("alice"), credential(1));
    let key = advisory_lock_key(&alice, &server);

    // a `put` of the first secret of this key is under way: it holds the lock, its row is not
    // committed
    let mut writer = hold_lock(&raw, key).await;
    write_a_row_in(&mut writer, alice.as_str(), &server, 5).await;

    let delete = {
        let (store, alice, server) = (store.clone(), alice.clone(), server.clone());
        tokio::spawn(async move { store.delete(&alice, &server).await })
    };
    queues_and_stays_queued(&raw, key, &delete).await;

    writer.commit().await.unwrap();
    delete.await.unwrap().unwrap();
    assert_eq!(rows(&raw).await, 0, "the delete was ordered after the put");
    assert_eq!(waiters(&raw, key).await, 0);
}

#[tokio::test]
async fn a_put_waits_for_the_lock_of_its_key() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store(ring_k1()).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    let (alice, server) = (db.owner("alice"), credential(1));
    let key = advisory_lock_key(&alice, &server);

    let writer = hold_lock(&raw, key).await;
    let put = {
        let (store, alice, server) = (store.clone(), alice.clone(), server.clone());
        tokio::spawn(async move { store.put(&alice, &server, &material("after")).await })
    };
    queues_and_stays_queued(&raw, key, &put).await;
    assert_eq!(rows(&raw).await, 0, "nothing was written behind the lock");

    writer.commit().await.unwrap();
    let version = put.await.unwrap().unwrap();
    let (held, got) = store.get(&alice, &server).await.unwrap().unwrap();
    assert_eq!((held, got.expose()), (version, &b"after"[..]));
}

/// The version is compared after the lock, so a rotate that waited sees what the writer ahead of it
/// committed: here a newer version, so the rotate is a conflict and writes nothing.
#[tokio::test]
async fn a_rotate_waits_for_the_lock_of_its_key_and_compares_versions_after_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = db.store(ring_k1()).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    let (alice, server) = (db.owner("alice"), credential(1));
    let key = advisory_lock_key(&alice, &server);
    let read = store
        .put(&alice, &server, &material("before"))
        .await
        .unwrap();

    // a writer is under way; it is going to commit another version of the key
    let mut writer = hold_lock(&raw, key).await;
    let rotate = {
        let (store, alice, server) = (store.clone(), alice.clone(), server.clone());
        tokio::spawn(async move { store.rotate(&alice, &server, read, &material("mine")).await })
    };
    queues_and_stays_queued(&raw, key, &rotate).await;
    sqlx::query("UPDATE user_secrets SET version = version + 1000")
        .execute(&mut *writer)
        .await
        .unwrap();
    writer.commit().await.unwrap();

    assert!(
        matches!(rotate.await.unwrap(), Err(SecretError::Conflict)),
        "the rotate read the version before the writer's commit"
    );
    let (version,): (i64,) = sqlx::query_as("SELECT version FROM user_secrets")
        .fetch_one(&raw)
        .await
        .unwrap();
    assert_eq!(
        version,
        i64::try_from(read.get()).unwrap() + 1000,
        "the writer's row is as it left it"
    );
}

/// A pool as `PgSecretStore::connect` makes it: four connections, ten seconds to get one.
async fn pool_of_four(db: &TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(db.options().application_name("orch-secrets-pool-of-four"))
        .await
        .unwrap()
}

/// One key's lock held for long (a wedged replica, a paused VM, a repair that was never committed)
/// must cost the writers of that key a timeout each and nobody else anything: without a bound on
/// the wait they keep a pooled connection after their callers have given up, the pool of four is
/// gone, and a `get` of an unrelated key fails for want of a connection.
#[tokio::test]
async fn a_key_whose_lock_is_held_makes_its_writers_unavailable_and_leaves_the_pool_to_the_rest() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let pool = pool_of_four(&db).await;
    let store = PgSecretStore::from_pool(pool.clone(), ring_k1());
    store.migrate().await.unwrap();
    let raw = db.pool("orch-secrets-raw", 4).await;
    let (alice, stuck, free) = (db.owner("alice"), credential(1), credential(2));
    let key = advisory_lock_key(&alice, &stuck);
    store
        .put(&alice, &free, &material("free material"))
        .await
        .unwrap();

    let wedged = hold_lock(&raw, key).await;

    // more writers of the wedged key than the pool has connections
    let started = Instant::now();
    let writers: Vec<_> = (0..8_u32)
        .map(|n| {
            let (store, alice, stuck) = (store.clone(), alice.clone(), stuck.clone());
            tokio::spawn(async move {
                let result = store
                    .put(&alice, &stuck, &material(&format!("writer {n}")))
                    .await;
                (result, started.elapsed())
            })
        })
        .collect();
    // the four connections of the pool are all waiting for the lock
    wait_for_waiters(&raw, key, 4).await;

    // meanwhile another key is served, as soon as a connection is free (at the latest when the
    // first writers give up), and well before a call gives up waiting for the pool
    let during = Instant::now();
    let (version, got) = store.get(&alice, &free).await.unwrap().unwrap();
    assert_eq!(got.expose(), b"free material");
    let another = store
        .put(&alice, &credential(3), &material("another key"))
        .await
        .unwrap();
    assert!(another > version);
    assert!(
        during.elapsed() < Duration::from_secs(9),
        "the other keys waited {:?} for a connection",
        during.elapsed()
    );
    assert_eq!(holders(&raw, key).await, 1, "the lock was held all along");

    // every writer of the wedged key has given up: Unavailable, the class that is retried, in a
    // few seconds, and none of them is still waiting in the database
    for writer in writers {
        let (result, took) = tokio::time::timeout(Duration::from_secs(12), writer)
            .await
            .expect("a writer is still waiting for the lock")
            .unwrap();
        let error = result.expect_err("the lock was held");
        assert!(
            matches!(error, SecretError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
        assert!(error.to_string().contains("locked"), "{error}");
        assert!(
            took >= LOCK_TIMEOUT - Duration::from_millis(200),
            "gave up after {took:?}, before the lock timeout"
        );
    }
    assert_eq!(waiters(&raw, key).await, 0, "nobody waits for the lock");

    // and the pool is whole: four calls at once, each on a connection of its own, in a moment
    let before = Instant::now();
    let calls: Vec<_> = (10..14_u32)
        .map(|n| {
            let (store, alice) = (store.clone(), alice.clone());
            tokio::spawn(async move {
                store
                    .put(&alice, &credential(n), &material("quick"))
                    .await
                    .unwrap()
            })
        })
        .collect();
    for call in calls {
        call.await.unwrap();
    }
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "{:?} for four writes on four connections",
        before.elapsed()
    );

    // when the lock is let go, the key is written again
    wedged.commit().await.unwrap();
    store.put(&alice, &stuck, &material("back")).await.unwrap();
    pool.close().await;
}

/// A wait for a lock that ends is the same for a rewrap: it stops there, `Unavailable`, and the rows
/// before it are done and the ones after are untouched.
#[tokio::test]
async fn a_rewrap_that_meets_a_wedged_key_stops_there_and_has_done_the_rows_before_it() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let old = db.store(ring("k1", &[("k1", 1)])).await;
    let new = db.another_store(ring("k2", &[("k1", 1), ("k2", 2)])).await;
    let raw = db.pool("orch-secrets-raw", 4).await;
    let alice = db.owner("alice");
    for n in 1..=3 {
        old.put(&alice, &credential(n), &material("x"))
            .await
            .unwrap();
    }
    // rows are visited in the order of (owner, server): the second is the wedged one
    let wedged = hold_lock(&raw, advisory_lock_key(&alice, &credential(2))).await;

    let error = tokio::time::timeout(Duration::from_secs(12), new.rewrap())
        .await
        .expect("the run is still waiting for the wedged lock")
        .unwrap_err();
    assert!(
        matches!(error, SecretError::Unavailable { .. }),
        "{error:?}"
    );
    let kids: Vec<(String,)> = sqlx::query_as("SELECT kid FROM user_secrets ORDER BY server")
        .fetch_all(&raw)
        .await
        .unwrap();
    assert_eq!(
        kids.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(),
        ["k2", "k1", "k1"]
    );

    wedged.commit().await.unwrap();
    let report = new.rewrap().await.unwrap();
    assert_eq!(
        (report.rewrapped, report.already_current, report.failed),
        (2, 1, 0)
    );
}

/// The writes do not take a database's `default_transaction_isolation` for granted. Under
/// `REPEATABLE READ` the snapshot is taken by the first query of the transaction, the lock's, which
/// is before the wait: the writer that waited would then fail with a serialization error on the row
/// the writer ahead of it had just written.
#[tokio::test]
async fn writers_of_one_key_do_not_depend_on_the_isolation_the_database_defaults_to() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    // the schema is made in the ordinary way
    db.store(ring_k1()).await;
    let repeatable = [("default_transaction_isolation", "repeatable read")];
    let pool = db
        .pool_with("orch-secrets-repeatable", 8, &repeatable)
        .await;
    let other = db
        .pool_with("orch-secrets-repeatable-2", 8, &repeatable)
        .await;
    // the setting is in force: an ordinary transaction on these sessions is REPEATABLE READ
    for pool in [&pool, &other] {
        let mut tx = pool.begin().await.unwrap();
        let (level,): (String,) = sqlx::query_as("SHOW transaction_isolation")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(level, "repeatable read");
    }
    let stores = [
        PgSecretStore::from_pool(pool.clone(), ring_k1()),
        PgSecretStore::from_pool(other.clone(), ring_k1()),
    ];

    let alice = db.owner("alice");
    for round in 0..10_u32 {
        let server = credential(round);
        let first = stores[0]
            .put(&alice, &server, &material("start"))
            .await
            .unwrap();
        let tasks: Vec<_> = (0..16_u32)
            .map(|n| {
                let (store, alice, server) = (
                    stores[(n % 2) as usize].clone(),
                    alice.clone(),
                    server.clone(),
                );
                tokio::spawn(async move {
                    let mine = material(&format!("writer {round}/{n}"));
                    if n % 4 == 3 {
                        store.rotate(&alice, &server, first, &mine).await.ok()
                    } else {
                        Some(
                            store
                                .put(&alice, &server, &mine)
                                .await
                                .unwrap_or_else(|e| panic!("round {round}, writer {n}: {e:?}")),
                        )
                    }
                })
            })
            .collect();
        let mut given = Vec::new();
        for task in tasks {
            if let Some(version) = task.await.unwrap() {
                given.push(version);
            }
        }
        let (held, _) = stores[0].get(&alice, &server).await.unwrap().unwrap();
        assert_eq!(held, *given.iter().max().unwrap(), "round {round}");
    }
}

/// The tests above trust `waiters` and `holders` to see one key's lock and every bit of it: the
/// server keeps a `bigint` key as two `oid`s, unsigned, and a key with the sign bit set, or with
/// the top bit of its low half set, is where putting them back together goes wrong.
#[tokio::test]
async fn the_helpers_that_watch_the_locks_see_exactly_one_key_whatever_its_bits() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let raw = db.pool("orch-secrets-raw", 4).await;
    // one key of each kind (sign bit, top bit of the low half), from this test's own owners
    let mut keys: Vec<i64> = Vec::new();
    let mut kinds = std::collections::BTreeSet::new();
    for n in 0..400_u32 {
        let key = advisory_lock_key(&db.owner(&format!("p{n}")), &credential(n));
        if kinds.insert((key < 0, (key & 0xffff_ffff) >= 0x8000_0000)) {
            keys.push(key);
        }
    }
    assert_eq!(keys.len(), 4, "{keys:?}");

    for &key in &keys {
        let holder = hold_lock(&raw, key).await;
        let blocked = {
            let raw = raw.clone();
            tokio::spawn(async move { hold_lock(&raw, key).await.commit().await.unwrap() })
        };
        wait_for_a_waiter(&raw, key).await;
        assert_eq!(
            (holders(&raw, key).await, waiters(&raw, key).await),
            (1, 1),
            "{key}"
        );
        for &other in keys.iter().filter(|other| **other != key) {
            assert_eq!(
                (holders(&raw, other).await, waiters(&raw, other).await),
                (0, 0),
                "{other} is not {key}"
            );
        }
        holder.commit().await.unwrap();
        blocked.await.unwrap();
        assert_eq!(
            (holders(&raw, key).await, waiters(&raw, key).await),
            (0, 0),
            "{key}"
        );
    }
}
