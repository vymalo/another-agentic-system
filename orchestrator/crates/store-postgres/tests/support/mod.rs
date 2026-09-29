//! Test isolation: every store gets its own schema (`search_path`), so tests run in parallel
//! against one database. Set `ORCH_TEST_DATABASE_URL` to run them; without it they skip.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use orch_store_postgres::PgStore;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const ENV: &str = "ORCH_TEST_DATABASE_URL";
const SCHEMA_PREFIX: &str = "t_";
const STALE_AFTER_SECS: u64 = 3600;

pub fn database_url() -> Option<String> {
    std::env::var(ENV).ok().filter(|v| !v.is_empty())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// An isolated schema: created, and all connections of the returned options use it.
pub struct TestDb {
    pub url: String,
    pub schema: String,
}

impl TestDb {
    pub async fn new() -> Option<Self> {
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
        Some(Self { url, schema })
    }

    pub fn options(&self) -> PgConnectOptions {
        PgConnectOptions::from_str(&self.url)
            .expect("valid ORCH_TEST_DATABASE_URL")
            .options([("search_path", self.schema.as_str())])
    }

    /// A pool confined to this schema, its connections named `app` for `pg_stat_activity`.
    pub async fn pool(&self, app: &str, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .connect_with(self.options().application_name(app))
            .await
            .expect("connect pool")
    }

    /// A migrated store in this schema.
    pub async fn store(&self) -> PgStore {
        let store = PgStore::from_pool(self.pool("orch-test", 24).await);
        store.migrate().await.expect("migrate");
        store
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
