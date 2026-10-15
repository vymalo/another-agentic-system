//! `PgPendingAuthorizations` against a real Postgres (ADR 0046, slice 4c): what the conformance suite
//! (`tests/conformance.rs`) cannot say about this adapter, which is how it is built. A row holds the
//! hash of a state and never the state; a server is one row however often it authorizes; the rows of
//! flows nobody came back to are swept by the next put; a take removes the row, an ended one too; the
//! table refuses what is not an owner or a server; a wait for a row that somebody holds is bounded; and
//! an error the server answers carries no row. Skipped unless `ORCH_TEST_DATABASE_URL` is set.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::{OAuthState, PendingAuthorizationError, PendingAuthorizations, UserToolServerId};
use orch_store_postgres::{PENDING_LOCK_TIMEOUT, PgPendingAuthorizations};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use support::TestDb;
use uuid::Uuid;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_790_000_000)
        .unwrap()
        .checked_add(SignedDuration::from_secs(secs))
        .unwrap()
}

fn state() -> OAuthState {
    OAuthState::new(&Uuid::now_v7().simple().to_string()).unwrap()
}

fn owner(name: &str) -> UserId {
    UserId::new(&format!("{name}-{}@example.test", Uuid::now_v7().simple()))
}

fn server(suffix: &str) -> UserToolServerId {
    UserToolServerId::parse(&format!("u-{suffix}")).unwrap()
}

/// A migrated schema and the store over a pool of `max` connections that waits at most three seconds
/// for one.
async fn store_with(db: &TestDb, max: u32) -> PgPendingAuthorizations {
    let migrated = db.store().await;
    drop(migrated);
    let pool = PgPoolOptions::new()
        .max_connections(max)
        .acquire_timeout(Duration::from_secs(3))
        .connect_with(db.options().application_name("orch-pending-test"))
        .await
        .expect("connect pool");
    PgPendingAuthorizations::from_pool(pool)
}

async fn rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM user_tool_oauth_pending")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A row keeps the SHA-256 of the state and nothing of the state: not as a column, not in the text
/// of the whole row, not in the other columns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_holds_the_hash_of_the_state_and_never_the_state() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 4).await;
    let planted = OAuthState::new("SENTINEL-state-0123456789abcdef").unwrap();
    let (alice, x) = (owner("alice"), server("aaaaaaaaaa"));
    store
        .put(&planted, &alice, &x, at(600), at(0))
        .await
        .unwrap();

    let whole_row: String = sqlx::query_scalar("SELECT t::text FROM user_tool_oauth_pending t")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(!whole_row.contains("SENTINEL"), "{whole_row}");
    assert!(!whole_row.contains("0123456789abcdef"), "{whole_row}");
    let is_the_sha256: bool = sqlx::query_scalar(
        "SELECT state_hash = sha256(convert_to($1, 'UTF8')) FROM user_tool_oauth_pending",
    )
    .bind("SENTINEL-state-0123456789abcdef")
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(is_the_sha256, "the column is the SHA-256 of the state");
}

/// A server that authorizes again and again is one row; the rows of other flows that ended are swept
/// by the put that comes after their end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_table_holds_one_row_per_server_and_sweeps_what_has_ended() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 4).await;
    let alice = owner("alice");
    for n in 0..30 {
        store
            .put(&state(), &alice, &server("aaaaaaaaaa"), at(600 + n), at(n))
            .await
            .unwrap();
    }
    assert_eq!(
        rows(store.pool()).await,
        1,
        "the later put replaces the earlier"
    );

    for n in 0..8 {
        store
            .put(
                &state(),
                &alice,
                &server(&format!("{n:010}")),
                at(600),
                at(0),
            )
            .await
            .unwrap();
    }
    assert_eq!(rows(store.pool()).await, 9);
    // ten minutes later someone else begins one
    store
        .put(
            &state(),
            &owner("bob"),
            &server("bbbbbbbbbb"),
            at(1_300),
            at(700),
        )
        .await
        .unwrap();
    assert_eq!(
        rows(store.pool()).await,
        1,
        "the nine flows that ended (the server's latest at 629 included) are gone, bob's is the one"
    );
}

/// A take removes the row, an ended one's too (and gives back nothing).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_take_removes_the_row_and_so_does_the_take_of_an_ended_state() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 4).await;
    let (alice, x, y) = (owner("alice"), server("xxxxxxxxxx"), server("yyyyyyyyyy"));
    let (live, ended) = (state(), state());
    store.put(&live, &alice, &x, at(600), at(0)).await.unwrap();
    store.put(&ended, &alice, &y, at(600), at(0)).await.unwrap();
    assert_eq!(rows(store.pool()).await, 2);

    assert!(store.take(&live, at(1)).await.unwrap().is_some());
    assert_eq!(rows(store.pool()).await, 1);
    assert!(store.take(&ended, at(9_000)).await.unwrap().is_none());
    assert_eq!(
        rows(store.pool()).await,
        0,
        "an ended state is consumed by the take that finds it"
    );
}

/// The table is what the port needs and no more: an owner that is empty and a server that is not an
/// id are refused by the table itself, a state hash that is not 32 bytes too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_table_refuses_what_is_not_an_owner_a_server_or_a_digest() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 4).await;
    let insert = |hash: Vec<u8>, owner: &'static str, server: &'static str| {
        let pool = store.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO user_tool_oauth_pending (state_hash, owner, server, expires_at) \
                 VALUES ($1, $2, $3, now())",
            )
            .bind(hash)
            .bind(owner)
            .bind(server)
            .execute(&pool)
            .await
        }
    };
    assert!(
        insert(vec![0; 32], "a@example.test", "u-0123456789")
            .await
            .is_ok()
    );
    assert!(
        insert(vec![1; 32], "", "u-0123456789").await.is_err(),
        "no owner"
    );
    for bad in [
        "U-0123456789",
        "u-012345678",
        "u-0123456789a",
        "0123456789ab",
        "u-012345678!",
    ] {
        assert!(
            insert(vec![2; 32], "a@example.test", bad).await.is_err(),
            "{bad}"
        );
    }
    for short in [vec![3; 31], vec![3; 33], Vec::new()] {
        assert!(
            insert(short, "b@example.test", "u-0123456789")
                .await
                .is_err()
        );
    }
}

/// A statement waits for a row somebody holds, and gives up after [`PENDING_LOCK_TIMEOUT`] as
/// `Unavailable`, handing its connection back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wait_for_a_row_that_somebody_holds_ends_as_unavailable_and_gives_the_connection_back() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    // one connection: if the failed statement kept it, the next call could not get one
    let store = store_with(&db, 1).await;
    let holder = db.pool("holder", 2).await;
    let (alice, x, pending) = (owner("alice"), server("hhhhhhhhhh"), state());
    store
        .put(&pending, &alice, &x, at(600), at(0))
        .await
        .unwrap();

    let mut tx = holder.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM user_tool_oauth_pending FOR UPDATE")
        .execute(&mut *tx)
        .await
        .unwrap();

    let started = Instant::now();
    let error = store.take(&pending, at(1)).await.unwrap_err();
    let waited = started.elapsed();
    assert!(
        waited >= PENDING_LOCK_TIMEOUT.mul_f32(0.8) && waited < PENDING_LOCK_TIMEOUT * 15,
        "it waited for the lock and gave up: {waited:?}"
    );
    assert!(
        matches!(error, PendingAuthorizationError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Transient);

    tx.rollback().await.unwrap();
    // the state is still pending, and the one connection is back
    assert!(store.take(&pending, at(2)).await.unwrap().is_some());
}

/// An error the server answers does not carry the row it was about: the `DETAIL` of a constraint
/// violation holds it and a driver's `Debug` prints it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_the_server_answers_names_no_row() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 4).await;
    // an owner the table refuses (the port cannot say an owner is empty)
    let error = store
        .put(
            &state(),
            &UserId::new(""),
            &server("eeeeeeeeee"),
            at(600),
            at(0),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, PendingAuthorizationError::Unavailable { .. }),
        "{error:?}"
    );
    let mut said = String::new();
    let mut at_error: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(e) = at_error {
        said.push_str(&format!("{e} | {e:?} | "));
        at_error = e.source();
    }
    assert!(
        said.contains("SQLSTATE 23514"),
        "the check that refused it is named: {said}"
    );
    for leaked in ["Failing row", "u-eeeeeeeeee", "DETAIL"] {
        assert!(!said.contains(leaked), "{leaked} is in: {said}");
    }
}

/// The table is created by the migrations, on a database that has them applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_migrations_make_the_table() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = store_with(&db, 2).await;
    assert_eq!(rows(store.pool()).await, 0);
}
