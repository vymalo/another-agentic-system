//! Test isolation: every store gets its own schema (`search_path`), so tests run in parallel
//! against one database. Set `ORCH_TEST_DATABASE_URL` to run them; without it they skip.
//!
//! Behind a transaction-mode pooler (issue 290) the isolation is a database of its own instead:
//! a pooler cannot carry the `search_path` startup option, and a `SET` would not outlive the
//! transaction. Set `ORCH_TEST_DIRECT_DATABASE_URL` to the database itself, with
//! `ORCH_TEST_DATABASE_URL` the pooler's address of it: every test then gets a database made
//! through the direct URL, queries go through the pooler, and what must be direct
//! (migrations, `LISTEN`) takes `TestDb::direct_url`. The databases are swept by the next run.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use orch_store_postgres::{PgStore, PgWakeup};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
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
            dedicated: None,
        })
    }

    /// A database of its own, whatever the mode: for a test that needs to see every connection that
    /// worked on it, or DDL that is database-wide. Made and dropped over the direct URL.
    pub async fn isolated_database() -> Option<Self> {
        let url = database_url()?;
        let direct = direct_database_url().unwrap_or_else(|| url.clone());
        let name = format!("{SCHEMA_PREFIX}{}_{}", now_secs(), Uuid::now_v7().simple());
        let mut conn = PgConnection::connect(&direct)
            .await
            .expect("connect to test database");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&mut conn)
            .await
            .expect("create database");
        drop_stale_databases(&mut conn).await;
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

    pub fn options(&self) -> PgConnectOptions {
        self.options_of(&self.url)
    }

    /// The options of the direct URL, pinned like `options()`.
    pub fn direct_options(&self) -> PgConnectOptions {
        self.options_of(&self.direct_url)
    }

    fn options_of(&self, url: &str) -> PgConnectOptions {
        let options = PgConnectOptions::from_str(url)
            .expect("valid ORCH_TEST_DATABASE_URL")
            // a pooler refuses sqlx's default startup parameter for it (see `PgStore::connect_with`)
            .extra_float_digits(None);
        if self.has_database() {
            options
        } else {
            options.options([("search_path", self.schema.as_str())])
        }
    }

    /// A pool confined to this schema, its connections named `app` for `pg_stat_activity`.
    pub async fn pool(&self, app: &str, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .connect_with(self.options().application_name(app))
            .await
            .expect("connect pool")
    }

    /// The options of `url` whose sessions have the given settings from the start (what a role or a
    /// database setting does: `ALTER ROLE ... SET`).
    ///
    /// A pooler cannot carry a startup option, so with a database of its own the settings are the
    /// database's (`ALTER DATABASE ... SET`, made over the direct URL) and reach the sessions opened
    /// after this call: call it before the first pooled connection of the test.
    pub async fn options_with(&self, settings: &[(&str, &str)]) -> PgConnectOptions {
        if !self.has_database() {
            return self.options().options(settings.iter().copied());
        }
        let mut conn = PgConnection::connect_with(&self.direct_options())
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
    }

    /// A pool confined to this schema whose sessions have the given settings from the start; see
    /// [`options_with`](Self::options_with).
    pub async fn pool_with(&self, app: &str, max: u32, settings: &[(&str, &str)]) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .connect_with(self.options_with(settings).await.application_name(app))
            .await
            .expect("connect pool")
    }

    /// A wakeup whose `NOTIFY`s go by a pool of `max` connections to `url` and whose listener is
    /// the direct connection when a pooler is in front (a listener cannot be pooled).
    pub fn wakeup(&self, app: &str, max: u32) -> impl std::future::Future<Output = PgWakeup> + '_ {
        let app = app.to_owned();
        async move {
            let pool = self.pool(&app, max).await;
            if self.url == self.direct_url {
                PgWakeup::start(pool)
            } else {
                // the listener's session carries the pool's name, as it does when it is taken from it
                let sep = if self.pinned_direct_url().contains('?') {
                    '&'
                } else {
                    '?'
                };
                let url = format!("{}{sep}application_name={app}", self.pinned_direct_url());
                PgWakeup::start_direct(pool, &url).expect("a valid URL")
            }
        }
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
async fn drop_stale_databases(conn: &mut PgConnection) {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT datname FROM pg_database WHERE datname LIKE 't\\_%'")
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
        sqlx::query_as("SELECT nspname FROM pg_namespace WHERE nspname LIKE 't\\_%'")
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
