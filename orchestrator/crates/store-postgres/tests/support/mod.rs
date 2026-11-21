//! Test isolation: every store gets its own schema (`search_path`), so tests run in parallel
//! against one database. Set `ORCH_TEST_DATABASE_URL` to run them; without it they skip.
//!
//! Behind a transaction-mode pooler (issue 290) the isolation is a database of its own instead:
//! a pooler cannot carry the `search_path` startup option, and a `SET` would not outlive the
//! transaction. Set `ORCH_TEST_DIRECT_DATABASE_URL` to the database itself, with
//! `ORCH_TEST_DATABASE_URL` the pooler's address of it: every test then gets a database made
//! through the direct URL, queries go through the pooler, and what must be direct
//! (migrations, `LISTEN`) takes `TestDb::direct_url`. The databases are swept by the next run.
//!
//! The connections are the store's own: `orch-pg-connect`'s options (the connection string as sqlx
//! read it), [`PgPool`] and the statements of `orch_store_postgres::sql`.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::{SystemTime, UNIX_EPOCH};

use orch_store_postgres::sql::{Client, PgOptions, query_scalar, raw_sql};
use orch_store_postgres::{PgPool, PgStore, PgWakeup};
use uuid::Uuid;

const ENV: &str = "ORCH_TEST_DATABASE_URL";
const DIRECT_ENV: &str = "ORCH_TEST_DIRECT_DATABASE_URL";
const SCHEMA_PREFIX: &str = "t_";
const STALE_AFTER_SECS: u64 = 3600;
/// A database is bigger than a schema, so it is swept sooner.
const STALE_DATABASE_AFTER_SECS: u64 = 900;

pub fn database_url() -> Option<String> {
    std::env::var(ENV).ok().filter(|v| !v.is_empty())
}

/// The database itself, when the main variable is a pooler's address of it.
pub fn direct_database_url() -> Option<String> {
    std::env::var(DIRECT_ENV).ok().filter(|v| !v.is_empty())
}

/// `url` up to the end of its authority (`postgres://user:pw@host:port`) and the rest.
fn split_authority(url: &str) -> (&str, &str) {
    let start = url.find("://").map_or(0, |i| i + 3);
    let end = url[start..].find('/').map_or(url.len(), |i| start + i);
    url.split_at(end)
}

/// The same database as `url`, a URL of the pooler `ORCH_TEST_DATABASE_URL` names, over the direct
/// connection: `None` when no pooler is in front or `url` is not the pooler's. For a process that
/// takes its connection strings as it is given (the binary's tests): it gets both.
pub fn direct_counterpart(url: &str) -> Option<String> {
    let direct = direct_database_url()?;
    let pooled = database_url()?;
    let (pooler, _) = split_authority(&pooled);
    let (server, _) = split_authority(&direct);
    let (authority, rest) = split_authority(url);
    (authority == pooler).then(|| format!("{server}{rest}"))
}

/// `url` with its database replaced: the path between the authority and the query.
pub fn with_database(url: &str, database: &str) -> String {
    let authority_start = url.find("://").map_or(0, |i| i + 3);
    let path_start = url[authority_start..]
        .find('/')
        .map_or(url.len(), |i| authority_start + i);
    let query_start = url[path_start..]
        .find('?')
        .map_or(url.len(), |i| path_start + i);
    format!("{}/{}{}", &url[..path_start], database, &url[query_start..])
}

/// `url` with `options=-c search_path=<schema>` added.
fn with_search_path(url: &str, schema: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}options=-c%20search_path%3D{schema}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A startup option `-c key=value`, its value's spaces and backslashes escaped as the server reads
/// them.
fn startup_option(key: &str, value: &str) -> String {
    let value = value.replace('\\', "\\\\").replace(' ', "\\ ");
    format!("-c {key}={value}")
}

/// `options` with `settings` added to its startup options (`-c key=value`), after the ones it has.
fn with_settings(mut options: PgOptions, settings: &[(&str, &str)]) -> PgOptions {
    if settings.is_empty() {
        return options;
    }
    let mut startup: Vec<String> = options
        .config()
        .get_options()
        .map(str::to_owned)
        .into_iter()
        .collect();
    startup.extend(settings.iter().map(|(k, v)| startup_option(k, v)));
    options.config_mut().options(startup.join(" "));
    options
}

/// `options` whose sessions are named `app` (`application_name`, for `pg_stat_activity`).
pub fn named(mut options: PgOptions, app: &str) -> PgOptions {
    options.config_mut().application_name(app);
    options
}

/// One connection to `url`, of its own (its task ends when the client is dropped).
pub async fn connect(url: &str) -> Client {
    url.parse::<PgOptions>()
        .expect("valid connection string")
        .connect()
        .await
        .expect("connect to test database")
}

/// One connection with `options`, of its own.
pub async fn connect_with(options: &PgOptions) -> Client {
    options.connect().await.expect("connect to test database")
}

/// A pool of `max` connections with `options`, as the tests' pools were (sqlx's 30 s wait).
pub fn pool_of(options: PgOptions, max: u32) -> PgPool {
    PgPool::new(options, max).expect("build pool")
}

/// A database of its own.
struct Dedicated;

/// An isolated schema (or, behind a pooler, an isolated database): created, and all connections of
/// the returned options use it.
pub struct TestDb {
    /// Where queries go: the pooler's address when `ORCH_TEST_DIRECT_DATABASE_URL` is set. Without a
    /// database of its own (`schema` mode) it is the server's URL, and `schema` is the `search_path`.
    pub url: String,
    /// The same database over a connection that is not pooled: what a migration and a listener take.
    /// The same text as `url` unless a pooler is in front.
    pub direct_url: String,
    /// The schema of this test; `public` when the test has a database of its own.
    pub schema: String,
    dedicated: Option<Dedicated>,
}

impl TestDb {
    /// A schema of the shared database, or a database of its own when a pooler is in front.
    pub async fn new() -> Option<Self> {
        if direct_database_url().is_some() {
            Self::isolated_database().await
        } else {
            Self::in_schema().await
        }
    }

    async fn in_schema() -> Option<Self> {
        let url = database_url()?;
        let schema = format!("{SCHEMA_PREFIX}{}_{}", now_secs(), Uuid::now_v7().simple());
        let conn = connect(&url).await;
        raw_sql(&format!("CREATE SCHEMA \"{schema}\""), &conn)
            .await
            .expect("create schema");
        drop_stale_schemas(&conn).await;
        Some(Self {
            direct_url: url.clone(),
            url,
            schema,
            dedicated: None,
        })
    }

    /// A database of its own, whatever the mode: for a test that needs to see every connection that
    /// worked on it, or DDL that is database-wide. Made and dropped over the direct URL.
    pub async fn isolated_database() -> Option<Self> {
        let url = database_url()?;
        let direct = direct_database_url().unwrap_or_else(|| url.clone());
        let name = format!("{SCHEMA_PREFIX}{}_{}", now_secs(), Uuid::now_v7().simple());
        let conn = connect(&direct).await;
        raw_sql(&format!("CREATE DATABASE \"{name}\""), &conn)
            .await
            .expect("create database");
        drop_stale_databases(&conn).await;
        Some(Self {
            url: with_database(&url, &name),
            direct_url: with_database(&direct, &name),
            schema: "public".to_owned(),
            dedicated: Some(Dedicated),
        })
    }

    /// Whether this test has a database of its own (no `search_path` is needed or sent).
    pub fn has_database(&self) -> bool {
        self.dedicated.is_some()
    }

    /// `url` for a process that has no options of ours: with the `search_path` of the schema when
    /// the isolation is a schema.
    pub fn pinned_url(&self) -> String {
        self.pin(&self.url)
    }

    /// `direct_url` pinned the same way.
    pub fn pinned_direct_url(&self) -> String {
        self.pin(&self.direct_url)
    }

    fn pin(&self, url: &str) -> String {
        if self.has_database() {
            url.to_owned()
        } else {
            with_search_path(url, &self.schema)
        }
    }

    pub fn options(&self) -> PgOptions {
        self.options_of(&self.url)
    }

    /// The options of the direct URL, pinned like `options()`.
    pub fn direct_options(&self) -> PgOptions {
        self.options_of(&self.direct_url)
    }

    fn options_of(&self, url: &str) -> PgOptions {
        let options: PgOptions = url.parse().expect("valid ORCH_TEST_DATABASE_URL");
        if self.has_database() {
            options
        } else {
            with_settings(options, &[("search_path", self.schema.as_str())])
        }
    }

    /// A pool confined to this schema, its connections named `app` for `pg_stat_activity`.
    pub async fn pool(&self, app: &str, max: u32) -> PgPool {
        pool_of(named(self.options(), app), max)
    }

    /// The options of `url` whose sessions have the given settings from the start (what a role or a
    /// database setting does: `ALTER ROLE ... SET`).
    ///
    /// A pooler cannot carry a startup option, so with a database of its own the settings are the
    /// database's (`ALTER DATABASE ... SET`, made over the direct URL) and reach the sessions opened
    /// after this call: call it before the first pooled connection of the test. They then apply to
    /// every session of the test's database, its migrations included.
    pub async fn options_with(&self, settings: &[(&str, &str)]) -> PgOptions {
        if !self.has_database() {
            return with_settings(self.options(), settings);
        }
        let conn = connect_with(&self.direct_options()).await;
        let name: String = query_scalar("SELECT current_database()::text")
            .fetch_one(&conn)
            .await
            .expect("database name");
        for (key, value) in settings {
            raw_sql(
                &format!("ALTER DATABASE \"{name}\" SET {key} = '{value}'"),
                &conn,
            )
            .await
            .expect("alter database");
        }
        self.options()
    }

    /// A pool confined to this schema whose sessions have the given settings from the start; see
    /// [`options_with`](Self::options_with).
    pub async fn pool_with(&self, app: &str, max: u32, settings: &[(&str, &str)]) -> PgPool {
        pool_of(named(self.options_with(settings).await, app), max)
    }

    /// A wakeup whose `NOTIFY`s go by a pool of `max` connections to `url` and whose listener is
    /// the direct connection when a pooler is in front (a listener cannot be pooled).
    pub async fn wakeup(&self, app: &str, max: u32) -> PgWakeup {
        self.wakeup_over(self.pool(app, max).await, app)
    }

    /// A wakeup over `pool`: its listener is made with the pool's options, or, when a pooler is in
    /// front, is the direct connection (named `app`, as the pool's sessions are).
    pub fn wakeup_over(&self, pool: PgPool, app: &str) -> PgWakeup {
        if self.url == self.direct_url {
            return PgWakeup::start(pool);
        }
        let direct = self.pinned_direct_url();
        let sep = if direct.contains('?') { '&' } else { '?' };
        PgWakeup::start_direct(pool, &format!("{direct}{sep}application_name={app}"))
            .expect("a valid URL")
    }

    /// A migrated store in this schema: the migrations on the direct connection, the queries on `url`.
    pub async fn store(&self) -> PgStore {
        let store = PgStore::from_pool(self.pool("orch-test", 24).await);
        PgStore::migrate_direct(&self.pinned_direct_url())
            .await
            .expect("migrate");
        store
    }
}

/// Best effort: databases of earlier runs. A test's database is not dropped when its `TestDb` is (a
/// helper may return the store and let the `TestDb` go while the test goes on), so they pile up until
/// the next run finds them old enough.
async fn drop_stale_databases(conn: &Client) {
    let rows: Vec<String> =
        query_scalar("SELECT datname::text FROM pg_database WHERE datname LIKE 't\\_%'")
            .fetch_all(conn)
            .await
            .unwrap_or_default();
    for name in rows {
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_DATABASE_AFTER_SECS) {
            let _ = raw_sql(
                &format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"),
                conn,
            )
            .await;
        }
    }
}

/// Best effort: schemas of runs that crashed before nobody cleaned up.
async fn drop_stale_schemas(conn: &Client) {
    let rows: Vec<String> =
        query_scalar("SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 't\\_%'")
            .fetch_all(conn)
            .await
            .unwrap_or_default();
    for name in rows {
        let created = name
            .strip_prefix(SCHEMA_PREFIX)
            .and_then(|rest| rest.split('_').next())
            .and_then(|secs| secs.parse::<u64>().ok());
        if created.is_some_and(|c| now_secs().saturating_sub(c) > STALE_AFTER_SECS) {
            let _ = raw_sql(&format!("DROP SCHEMA IF EXISTS \"{name}\" CASCADE"), conn).await;
        }
    }
}
