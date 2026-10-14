//! Test isolation, as `orch-store-postgres`' tests do it: every store gets its own schema
//! (`search_path`), so the tests run in parallel against one database. Set `ORCH_TEST_DATABASE_URL`
//! to run them; without it they skip (the same variable, so the one Postgres of CI serves both).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use orch_secrets_pg::{KeyBytes, KeyId, Keyring, PgSecretStore};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

const ENV: &str = "ORCH_TEST_DATABASE_URL";
const SCHEMA_PREFIX: &str = "tsp_";
const STALE_AFTER_SECS: u64 = 3600;

pub fn database_url() -> Option<String> {
    std::env::var(ENV).ok().filter(|v| !v.is_empty())
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

    /// A pool confined to this schema.
    pub async fn pool(&self, app: &str, max: u32) -> PgPool {
        PgPoolOptions::new()
            .max_connections(max)
            .connect_with(self.options().application_name(app))
            .await
            .expect("connect pool")
    }

    /// A migrated store in this schema, sealing with `keys`.
    pub async fn store(&self, keys: Keyring) -> PgSecretStore {
        let store = PgSecretStore::from_pool(self.pool("orch-secrets-test", 24).await, keys);
        store.migrate().await.expect("migrate");
        store
    }

    /// Another store over the same schema (another process, as far as the database can tell),
    /// sealing with `keys`. Not migrated: the schema is.
    pub async fn another_store(&self, keys: Keyring) -> PgSecretStore {
        PgSecretStore::from_pool(self.pool("orch-secrets-test-2", 24).await, keys)
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
