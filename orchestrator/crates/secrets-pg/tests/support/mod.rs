//! Test isolation, as `orch-store-postgres`' tests do it: every store gets its own schema
//! (`search_path`), so the tests run in parallel against one database. Set `ORCH_TEST_DATABASE_URL`
//! to run them; without it they skip (the same variable, so the one Postgres of CI serves both).
//!
//! Behind a transaction-mode pooler (issue 290) a database of its own replaces the schema, as in
//! `orch-store-postgres`' tests: set `ORCH_TEST_DIRECT_DATABASE_URL` to the database itself and
//! `ORCH_TEST_DATABASE_URL` to the pooler's address of it.
//!
//! The connections are the crate's own: `orch-pg-connect` (the connection string as sqlx read it),
//! `deadpool-postgres` pools and `tokio-postgres` statements, through the small helpers at the end
//! of this file ([`fetch_all`], [`fetch_one`], [`execute`]) that read a row into a tuple.
#![allow(
    dead_code,
    async_fn_in_trait,
    clippy::unwrap_used,
    clippy::expect_used,
    missing_docs
)]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use deadpool_postgres::Object;
pub use deadpool_postgres::Pool;
use orch_core::UserId;
use orch_pg_connect::PgOptions;
use orch_secrets_pg::{KeyBytes, KeyId, Keyring, PgSecretStore};
use tokio_postgres::types::{FromSql, ToSql};
use tokio_postgres::{Client, Row};
use uuid::Uuid;

const ENV: &str = "ORCH_TEST_DATABASE_URL";
const DIRECT_ENV: &str = "ORCH_TEST_DIRECT_DATABASE_URL";
const SCHEMA_PREFIX: &str = "tsp_";
const STALE_AFTER_SECS: u64 = 3600;
/// A database is bigger than a schema, so it is swept sooner (and not when its test ends: a helper
/// may return the store and let the `TestDb` go while the test goes on).
const STALE_DATABASE_AFTER_SECS: u64 = 900;
/// How long a test's pool waits for a connection: sqlx's default acquire timeout, which the pools
/// of these tests had.
const POOL_WAIT: Duration = Duration::from_secs(30);

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

/// One connection to `url`, of its own.
pub async fn connect(url: &str) -> Client {
    url.parse::<PgOptions>()
        .expect("valid connection string")
        .connect()
        .await
        .expect("connect to test database")
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
/// connection that the server refuses shows as a pool that is empty, which a test reads as a store
/// that is down.
const STORE_CONNECTIONS: usize = 8;

/// A startup option `-c key=value`, its value's spaces and backslashes escaped as the server reads
/// them.
fn startup_option(key: &str, value: &str) -> String {
    let value = value.replace('\\', "\\\\").replace(' ', "\\ ");
    format!("-c {key}={value}")
}

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
        let conn = connect(&url).await;
        conn.batch_execute(&format!("CREATE SCHEMA \"{schema}\""))
            .await
            .expect("create schema");
        drop_stale_schemas(&conn).await;
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
        let conn = connect(&direct).await;
        conn.batch_execute(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("create database");
        drop_stale_databases(&conn).await;
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
        self.pinned(&self.direct_url)
    }

    /// [`url`](Self::url), pinned as [`pinned_direct_url`](Self::pinned_direct_url) is.
    pub fn pinned_url(&self) -> String {
        self.pinned(&self.url)
    }

    fn pinned(&self, url: &str) -> String {
        if self.dedicated {
            url.to_owned()
        } else {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}options=-c%20search_path%3D{}", self.schema)
        }
    }

    /// A person that only this test has. The advisory lock of a key is a hash of its owner and
    /// server, and locks are shared by every schema of the database, every test of the process and
    /// every process: a test that looks at the locks, or waits on one, needs a key that no other
    /// test can take, so its owners carry a tag of their own.
    pub fn owner(&self, name: &str) -> UserId {
        UserId::new(&format!("{name}-{}@example.test", self.tag))
    }

    /// The options of [`url`](Self::url), confined to this schema.
    pub fn options(&self) -> PgOptions {
        self.options_with(&[])
    }

    /// [`options`](Self::options) whose sessions have `settings` from the start, as startup
    /// options (`-c key=value`) beside the schema's `search_path`.
    fn options_with(&self, settings: &[(&str, &str)]) -> PgOptions {
        let mut options: PgOptions = self.url.parse().expect("valid ORCH_TEST_DATABASE_URL");
        let mut startup: Vec<String> = options
            .config()
            .get_options()
            .map(str::to_owned)
            .into_iter()
            .collect();
        if !self.dedicated {
            startup.push(startup_option("search_path", &self.schema));
        }
        startup.extend(settings.iter().map(|(k, v)| startup_option(k, v)));
        if !startup.is_empty() {
            options.config_mut().options(startup.join(" "));
        }
        options
    }

    /// A pool confined to this schema.
    pub async fn pool(&self, app: &str, max: usize) -> Pool {
        self.pool_with(app, max, &[]).await
    }

    /// A pool confined to this schema whose sessions have the given settings from the start (what
    /// a role or a database setting does: `ALTER ROLE ... SET`).
    ///
    /// A pooler cannot carry a startup option, so with a database of its own the settings are the
    /// database's (`ALTER DATABASE ... SET`, over the direct URL) and reach the sessions opened after
    /// this call.
    pub async fn pool_with(&self, app: &str, max: usize, settings: &[(&str, &str)]) -> Pool {
        let mut options = if self.dedicated && !settings.is_empty() {
            let conn = connect(&self.direct_url).await;
            let name: String = conn
                .query_one("SELECT current_database()", &[])
                .await
                .expect("database name")
                .get(0);
            for (key, value) in settings {
                conn.batch_execute(&format!("ALTER DATABASE \"{name}\" SET {key} = '{value}'"))
                    .await
                    .expect("alter database");
            }
            self.options()
        } else {
            self.options_with(settings)
        };
        options.config_mut().application_name(app);
        options.pool(max, POOL_WAIT).expect("build pool")
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

/// A transaction of a pooled connection that the test drives by hand (`BEGIN`, its statements,
/// [`commit`](Self::commit)). Dropped without a commit, its connection is closed, not given back,
/// so the server rolls it back and lets its locks go.
pub struct Held {
    conn: Option<Object>,
}

impl Held {
    /// `BEGIN` on a connection of `pool`.
    pub async fn begin(pool: &Pool) -> Self {
        let conn = pool.get().await.expect("a connection");
        conn.batch_execute("BEGIN").await.expect("begin");
        Self { conn: Some(conn) }
    }

    fn client(&self) -> &Client {
        self.conn.as_ref().expect("an open transaction")
    }

    /// `COMMIT`, and the connection back to its pool.
    pub async fn commit(mut self) -> Result<(), tokio_postgres::Error> {
        self.client().batch_execute("COMMIT").await?;
        drop(self.conn.take());
        Ok(())
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            drop(Object::take(conn));
        }
    }
}

/// A transaction that holds the advisory lock `key`: what a writer in the middle of its write has.
/// Nothing else of the writer is there; the test does what it wants of the table, then ends the
/// transaction (commit or drop) to let the others go on.
pub async fn hold_lock(pool: &Pool, key: i64) -> Held {
    let tx = Held::begin(pool).await;
    execute(&tx, "SELECT pg_advisory_xact_lock($1)", &[&key]).await;
    tx
}

/// How many sessions are blocked on the advisory lock `key`, from `pg_locks`. Only that lock: the
/// database's other advisory locks are other tests', and the migrator's own.
///
/// A `bigint` key is split by the server into `classid` (the high 32 bits) and `objid` (the low
/// 32); both are `oid`, unsigned, so they are put back together as `bigint`s, and the shift of the
/// high half into the sign bit wraps as the key does.
pub async fn waiters(pool: &Pool, key: i64) -> i64 {
    let (count,): (i64,) = fetch_one(
        pool,
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted \
         AND ((classid::bigint << 32) | objid::bigint) = $1",
        &[&key],
    )
    .await;
    count
}

/// How many sessions hold the advisory lock `key` (one, while a transaction of [`hold_lock`] is open).
pub async fn holders(pool: &Pool, key: i64) -> i64 {
    let (count,): (i64,) = fetch_one(
        pool,
        "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND granted \
         AND ((classid::bigint << 32) | objid::bigint) = $1",
        &[&key],
    )
    .await;
    count
}

/// Waits until at least `at_least` sessions are blocked on the lock `key`: the proof that a call is
/// waiting where the test wants it, with no sleep standing in for it (the sleeps only pace the
/// polling).
pub async fn wait_for_waiters(pool: &Pool, key: i64, at_least: i64) {
    for _ in 0..400 {
        if waiters(pool, key).await >= at_least {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("fewer than {at_least} sessions ever waited for the lock {key}");
}

/// [`wait_for_waiters`] for one.
pub async fn wait_for_a_waiter(pool: &Pool, key: i64) {
    wait_for_waiters(pool, key, 1).await;
}

/// Best effort: databases of runs that crashed before nobody cleaned up.
async fn drop_stale_databases(conn: &Client) {
    let rows = conn
        .query(
            "SELECT datname::text FROM pg_database WHERE datname LIKE 'tsp\\_%'",
            &[],
        )
        .await
        .unwrap_or_default();
    for row in rows {
        let name: String = row.get(0);
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_DATABASE_AFTER_SECS) {
            let _ = conn
                .batch_execute(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

/// Best effort: schemas of runs that crashed before nobody cleaned up.
async fn drop_stale_schemas(conn: &Client) {
    let rows = conn
        .query(
            "SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 'tsp\\_%'",
            &[],
        )
        .await
        .unwrap_or_default();
    for row in rows {
        let name: String = row.get(0);
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_AFTER_SECS) {
            let _ = conn
                .batch_execute(&format!("DROP SCHEMA IF EXISTS \"{name}\" CASCADE"))
                .await;
        }
    }
}

/// A statement's parameter.
pub type Param<'a> = &'a (dyn ToSql + Sync);

/// Where a test's statement runs: a pool (a connection of it, for the one statement) or a
/// transaction it holds.
pub trait Db {
    async fn rows(&self, sql: &str, params: &[Param<'_>]) -> Vec<Row>;
    async fn exec(&self, sql: &str, params: &[Param<'_>]) -> u64;
}

impl Db for Pool {
    async fn rows(&self, sql: &str, params: &[Param<'_>]) -> Vec<Row> {
        let conn = self.get().await.expect("a connection");
        conn.query(sql, params)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
    }

    async fn exec(&self, sql: &str, params: &[Param<'_>]) -> u64 {
        let conn = self.get().await.expect("a connection");
        conn.execute(sql, params)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
    }
}

impl Db for Held {
    async fn rows(&self, sql: &str, params: &[Param<'_>]) -> Vec<Row> {
        self.client()
            .query(sql, params)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
    }

    async fn exec(&self, sql: &str, params: &[Param<'_>]) -> u64 {
        self.client()
            .execute(sql, params)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e:?}"))
    }
}

/// A row read into a tuple, column by column.
pub trait FromRow {
    fn from_row(row: &Row) -> Self;
}

macro_rules! tuple_from_row {
    ($($t:ident $i:tt),+) => {
        impl<$($t: for<'a> FromSql<'a>),+> FromRow for ($($t,)+) {
            fn from_row(row: &Row) -> Self {
                ($(row.get::<_, $t>($i),)+)
            }
        }
    };
}

tuple_from_row!(A 0);
tuple_from_row!(A 0, B 1);
tuple_from_row!(A 0, B 1, C 2);
tuple_from_row!(A 0, B 1, C 2, D 3);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5);
tuple_from_row!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);

/// Every row of `sql`.
pub async fn fetch_all<T: FromRow>(db: &impl Db, sql: &str, params: &[Param<'_>]) -> Vec<T> {
    db.rows(sql, params).await.iter().map(T::from_row).collect()
}

/// The first row of `sql`, which must have one.
pub async fn fetch_one<T: FromRow>(db: &impl Db, sql: &str, params: &[Param<'_>]) -> T {
    let rows = db.rows(sql, params).await;
    T::from_row(rows.first().unwrap_or_else(|| panic!("no row: {sql}")))
}

/// Runs `sql`; the number of rows it touched.
pub async fn execute(db: &impl Db, sql: &str, params: &[Param<'_>]) -> u64 {
    db.exec(sql, params).await
}
