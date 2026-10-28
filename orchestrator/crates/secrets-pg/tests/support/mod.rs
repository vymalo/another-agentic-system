//! Test isolation, as `orch-store-postgres`' tests do it: every store gets its own schema
//! (`search_path`), so the tests run in parallel against one database. Set `ORCH_TEST_DATABASE_URL`
//! to run them; without it they skip (the same variable, so the one Postgres of CI serves both).
//!
//! Behind a transaction-mode pooler (issue 290) a database of its own replaces the schema, as in
//! `orch-store-postgres`' tests: set `ORCH_TEST_DIRECT_DATABASE_URL` to the database itself and
//! `ORCH_TEST_DATABASE_URL` to the pooler's address of it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use orch_core::UserId;
use orch_secrets_pg::{KeyBytes, KeyId, Keyring, PgSecretStore};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

const ENV: &str = "ORCH_TEST_DATABASE_URL";
const DIRECT_ENV: &str = "ORCH_TEST_DIRECT_DATABASE_URL";
const SCHEMA_PREFIX: &str = "tsp_";
const STALE_AFTER_SECS: u64 = 3600;
/// A database is bigger than a schema, so it is swept sooner (and not when its test ends: a helper
/// may return the store and let the `TestDb` go while the test goes on).
const STALE_DATABASE_AFTER_SECS: u64 = 900;

pub fn database_url() -> Option<String> {
    std::env::var(ENV).ok().filter(|v| !v.is_empty())
}

/// The database itself, when the main variable is a pooler's address of it.
pub fn direct_database_url() -> Option<String> {
    std::env::var(DIRECT_ENV).ok().filter(|v| !v.is_empty())
}

/// `url` with its database replaced: the path between the authority and the query.
fn with_database(url: &str, database: &str) -> String {
    let authority_start = url.find("://").map_or(0, |i| i + 3);
    let path_start = url[authority_start..]
        .find('/')
        .map_or(url.len(), |i| authority_start + i);
    let query_start = url[path_start..]
        .find('?')
        .map_or(url.len(), |i| path_start + i);
    format!("{}/{}{}", &url[..path_start], database, &url[query_start..])
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A key of 32 copies of `byte`.
pub fn key(byte: u8) -> KeyBytes {
    KeyBytes::from_bytes([byte; 32])
}

pub fn id(text: &str) -> KeyId {
    KeyId::parse(text).unwrap()
}

/// A ring: `current` and `(id, byte)` for every key held.
pub fn ring(current: &str, keys: &[(&str, u8)]) -> Keyring {
    Keyring::new(
        id(current),
        keys.iter().map(|(name, byte)| (id(name), key(*byte))),
    )
    .unwrap()
}

/// The ring of one key, `k1`.
pub fn ring_k1() -> Keyring {
    ring("k1", &[("k1", 1)])
}

/// The connections a test's store may open. A pool opens a connection only when a call needs one,
/// so this bounds what one test can ask of the server; the suites run their tests in parallel,
/// several processes may share one database (`max_connections` is 100 unless it is raised), and a
/// connection that the server refuses shows as `PoolTimedOut`, which a test reads as a store that
/// is down.
const STORE_CONNECTIONS: u32 = 8;

/// An isolated schema (or, behind a pooler, an isolated database): created, and all connections of
/// the returned options use it.
#[derive(Clone)]
pub struct TestDb {
    /// Where queries go: the pooler's address when `ORCH_TEST_DIRECT_DATABASE_URL` is set.
    pub url: String,
    /// The same database over a connection that is not pooled: what a migration takes.
    pub direct_url: String,
    pub schema: String,
    /// Random, one per test: in the owners this test makes (see [`owner`](Self::owner)).
    tag: String,
    /// Whether the isolation is a database of its own, not a schema.
    dedicated: bool,
}

impl TestDb {
    pub async fn new() -> Option<Self> {
        let Some(url) = database_url() else {
            eprintln!("skipped: {ENV} is not set");
            return None;
        };
        if let Some(direct) = direct_database_url() {
            return Some(Self::in_database(&url, direct).await);
        }
        let schema = format!("{SCHEMA_PREFIX}{}_{}", now_secs(), Uuid::now_v7().simple());
        let mut conn = PgConnection::connect(&url)
            .await
            .expect("connect to test database");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA \"{schema}\"")))
            .execute(&mut conn)
            .await
            .expect("create schema");
        drop_stale_schemas(&mut conn).await;
        Some(Self {
            direct_url: url.clone(),
            url,
            schema,
            tag: Uuid::now_v7().simple().to_string(),
            dedicated: false,
        })
    }

    /// A database of its own, made over the direct URL.
    async fn in_database(url: &str, direct: String) -> Self {
        let name = format!("{SCHEMA_PREFIX}{}_{}", now_secs(), Uuid::now_v7().simple());
        let mut conn = PgConnection::connect(&direct)
            .await
            .expect("connect to test database");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&mut conn)
            .await
            .expect("create database");
        drop_stale_databases(&mut conn).await;
        Self {
            url: with_database(url, &name),
            direct_url: with_database(&direct, &name),
            schema: "public".to_owned(),
            tag: Uuid::now_v7().simple().to_string(),
            dedicated: true,
        }
    }

    /// `direct_url` for a process or a call that has no options of ours: with the `search_path` of
    /// the schema when the isolation is a schema.
    pub fn pinned_direct_url(&self) -> String {
        if self.dedicated {
            self.direct_url.clone()
        } else {
            let sep = if self.direct_url.contains('?') {
                '&'
            } else {
                '?'
            };
            format!(
                "{}{sep}options=-c%20search_path%3D{}",
                self.direct_url, self.schema
            )
        }
    }

    /// A person that only this test has. The advisory lock of a key is a hash of its owner and
    /// server, and locks are shared by every schema of the database, every test of the process and
    /// every process: a test that looks at the locks, or waits on one, needs a key that no other
    /// test can take, so its owners carry a tag of their own.
    pub fn owner(&self, name: &str) -> UserId {
        UserId::new(&format!("{name}-{}@example.test", self.tag))
    }

    pub fn options(&self) -> PgConnectOptions {
        let options = PgConnectOptions::from_str(&self.url)
            .expect("valid ORCH_TEST_DATABASE_URL")
            .extra_float_digits(None);
        if self.dedicated {
            options
        } else {
            options.options([("search_path", self.schema.as_str())])
        }
    }

    /// A pool confined to this schema.
    pub async fn pool(&self, app: &str, max: u32) -> PgPool {
        self.pool_with(app, max, &[]).await
    }

    /// A pool confined to this schema whose sessions have the given settings from the start (what
    /// a role or a database setting does: `ALTER ROLE ... SET`).
    ///
    /// A pooler cannot carry a startup option, so with a database of its own the settings are the
    /// database's (`ALTER DATABASE ... SET`, over the direct URL) and reach the sessions opened after
    /// this call.
    pub async fn pool_with(&self, app: &str, max: u32, settings: &[(&str, &str)]) -> PgPool {
        let options = if self.dedicated && !settings.is_empty() {
            let mut conn = PgConnection::connect(&self.direct_url)
                .await
                .expect("connect directly");
            let name: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(&mut conn)
                .await
                .expect("database name");
            for (key, value) in settings {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "ALTER DATABASE \"{name}\" SET {key} = '{value}'"
                )))
                .execute(&mut conn)
                .await
                .expect("alter database");
            }
            self.options()
        } else {
            self.options().options(settings.iter().copied())
        };
        PgPoolOptions::new()
            .max_connections(max)
            .connect_with(options.application_name(app))
            .await
            .expect("connect pool")
    }

    /// A migrated store in this schema, sealing with `keys`.
    pub async fn store(&self, keys: Keyring) -> PgSecretStore {
        let store = PgSecretStore::from_pool(
            self.pool("orch-secrets-test", STORE_CONNECTIONS).await,
            keys,
        );
        PgSecretStore::migrate_direct(&self.pinned_direct_url())
            .await
            .expect("migrate");
        store
    }

    /// Another store over the same schema (another process, as far as the database can tell),
    /// sealing with `keys`. Not migrated: the schema is.
    pub async fn another_store(&self, keys: Keyring) -> PgSecretStore {
        PgSecretStore::from_pool(
            self.pool("orch-secrets-test-2", STORE_CONNECTIONS).await,
            keys,
        )
    }
}

/// A transaction that holds the advisory lock `key`: what a writer in the middle of its write has.
/// Nothing else of the writer is there; the test does what it wants of the table, then ends the
/// transaction (commit or drop) to let the others go on.
pub async fn hold_lock(pool: &PgPool, key: i64) -> Transaction<'static, Postgres> {
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *tx)
        .await
        .expect("take the lock");
    tx
}

/// How many sessions are blocked on the advisory lock `key`, from `pg_locks`. Only that lock: the
/// database's other advisory locks are other tests', and sqlx's own (the migrations').
///
/// A `bigint` key is split by the server into `classid` (the high 32 bits) and `objid` (the low
/// 32); both are `oid`, unsigned, so they are put back together as `bigint`s, and the shift of the
/// high half into the sign bit wraps as the key does.
pub async fn waiters(pool: &PgPool, key: i64) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
         AND ((classid::bigint << 32) | objid::bigint) = $1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .expect("read pg_locks");
    count
}

/// How many sessions hold the advisory lock `key` (one, while a transaction of [`hold_lock`] is open).
pub async fn holders(pool: &PgPool, key: i64) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND granted \
         AND ((classid::bigint << 32) | objid::bigint) = $1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .expect("read pg_locks");
    count
}

/// Waits until at least `at_least` sessions are blocked on the lock `key`: the proof that a call is
/// waiting where the test wants it, with no sleep standing in for it (the sleeps only pace the
/// polling).
pub async fn wait_for_waiters(pool: &PgPool, key: i64, at_least: i64) {
    for _ in 0..400 {
        if waiters(pool, key).await >= at_least {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("fewer than {at_least} sessions ever waited for the lock {key}");
}

/// [`wait_for_waiters`] for one.
pub async fn wait_for_a_waiter(pool: &PgPool, key: i64) {
    wait_for_waiters(pool, key, 1).await;
}

/// Best effort: databases of runs that crashed before nobody cleaned up.
async fn drop_stale_databases(conn: &mut PgConnection) {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT datname FROM pg_database WHERE datname LIKE 'tsp\\_%'")
            .fetch_all(&mut *conn)
            .await
            .unwrap_or_default();
    for (name,) in rows {
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_DATABASE_AFTER_SECS) {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
            )))
            .execute(&mut *conn)
            .await;
        }
    }
}

/// Best effort: schemas of runs that crashed before nobody cleaned up.
async fn drop_stale_schemas(conn: &mut PgConnection) {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT nspname FROM pg_namespace WHERE nspname LIKE 'tsp\\_%'")
            .fetch_all(&mut *conn)
            .await
            .unwrap_or_default();
    for (name,) in rows {
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_AFTER_SECS) {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP SCHEMA IF EXISTS \"{name}\" CASCADE"
            )))
            .execute(&mut *conn)
            .await;
        }
    }
}
