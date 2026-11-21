//! This runner and sqlx 0.9's read and continue each other's history (sys#382 E1): the same
//! checksums for every migration of the workspace, a database migrated by one is a no-op for the
//! other, the refusals match, and the advisory lock is the same lock. The database tests are
//! skipped unless `ORCH_TEST_DATABASE_URL` is set; each makes a database of its own.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use orch_pg_migrate::{MigrateError, Migration, Migrator};
use sqlx::migrate::Migrate as _;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection as _, PgConnection, PgPool};

const TABLE: &str = "_compat_migrations";

/// The migrations of a directory, read as this runner reads them.
fn ours(dir: &Path) -> Vec<Migration> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sql"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| {
            let sql: &'static str = Box::leak(std::fs::read_to_string(p).unwrap().into_boxed_str());
            Migration::from_file(p.file_name().unwrap().to_str().unwrap(), sql).unwrap()
        })
        .collect()
}

/// Every migration directory of the workspace: each file has the version, description and
/// checksum sqlx gives it, so a history sqlx wrote is this runner's.
#[tokio::test]
async fn every_migration_of_the_workspace_reads_as_sqlx_reads_it() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut dirs = 0;
    for entry in std::fs::read_dir(crates).unwrap() {
        let dir = entry.unwrap().path().join("migrations");
        if !dir.is_dir() {
            continue;
        }
        dirs += 1;
        let theirs = sqlx::migrate::Migrator::new(dir.as_path()).await.unwrap();
        let theirs: Vec<(i64, String, Vec<u8>)> = theirs
            .iter()
            .map(|m| (m.version, m.description.to_string(), m.checksum.to_vec()))
            .collect();
        let mine: Vec<(i64, String, Vec<u8>)> = ours(&dir)
            .iter()
            .map(|m| (m.version(), m.description().to_owned(), m.checksum()))
            .collect();
        assert!(!mine.is_empty(), "{}", dir.display());
        assert_eq!(mine, theirs, "{}", dir.display());
    }
    assert!(dirs >= 2, "store-postgres and secrets-pg have migrations");
}

/// A database of its own on the test server, with a sqlx pool and a tokio-postgres client.
struct Db {
    sqlx: PgPool,
    options: PgConnectOptions,
    config: tokio_postgres::Config,
}

impl Db {
    async fn new() -> Option<Self> {
        let url = std::env::var("ORCH_TEST_DATABASE_URL").ok()?;
        static N: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "compat_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        );
        let admin: PgConnectOptions = url.parse().unwrap();
        let mut conn = PgConnection::connect_with(&admin).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut conn)
            .await
            .unwrap();
        let options = admin.database(&name);
        let sqlx = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        let mut config: tokio_postgres::Config = url.parse().unwrap();
        config.dbname(&name);
        Some(Self {
            sqlx,
            options,
            config,
        })
    }

    async fn client(&self) -> tokio_postgres::Client {
        let (client, connection) = self.config.connect(tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(connection);
        client
    }

    async fn rows(&self) -> Vec<(i64, bool)> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT version, success FROM {TABLE} ORDER BY version"
        )))
        .fetch_all(&self.sqlx)
        .await
        .unwrap()
    }
}

/// A directory of migration files `(name, text)`.
fn dir(files: &[(&str, &str)]) -> tempdir::Dir {
    let dir = tempdir::Dir::new();
    for (name, sql) in files {
        std::fs::write(dir.path().join(name), sql).unwrap();
    }
    dir
}

mod tempdir {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory under the system's temporary directory, removed when dropped.
    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "orch-pg-migrate-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

const ONE: (&str, &str) = ("0001_create_a.sql", "CREATE TABLE a (id INT);\n");
const TWO: (&str, &str) = (
    "0002_create_b.sql",
    "CREATE TABLE b (id INT);\nINSERT INTO b VALUES (1);\n",
);
const THREE: (&str, &str) = ("0003_create_c.sql", "CREATE TABLE c (id INT);\n");

async fn sqlx_runs(db: &Db, files: &Path) {
    let mut migrator = sqlx::migrate::Migrator::new(files).await.unwrap();
    migrator.dangerous_set_table_name(TABLE);
    migrator.run(&db.sqlx).await.unwrap();
}

/// A database sqlx migrated is a no-op for this runner, which then applies only what is new; and
/// sqlx then finds nothing to do in what this runner wrote.
#[tokio::test]
async fn each_continues_the_others_history() {
    let Some(db) = Db::new().await else {
        return;
    };
    let two = dir(&[ONE, TWO]);
    sqlx_runs(&db, two.path()).await;
    let installed: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT installed_on::text FROM {TABLE} ORDER BY version"
    )))
    .fetch_all(&db.sqlx)
    .await
    .unwrap();

    let mut client = db.client().await;
    Migrator::new(TABLE, ours(two.path()))
        .unwrap()
        .run(&mut client)
        .await
        .expect("sqlx's history is this runner's");
    let again: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT installed_on::text FROM {TABLE} ORDER BY version"
    )))
    .fetch_all(&db.sqlx)
    .await
    .unwrap();
    assert_eq!(again, installed, "nothing was applied again");

    let three = dir(&[ONE, TWO, THREE]);
    Migrator::new(TABLE, ours(three.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap();
    assert_eq!(db.rows().await, [(1, true), (2, true), (3, true)]);
    let execution: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT execution_time FROM {TABLE} WHERE version = 3"
    )))
    .fetch_one(&db.sqlx)
    .await
    .unwrap();
    assert!(execution >= 0, "the execution time is recorded");

    sqlx_runs(&db, three.path()).await;
    assert_eq!(db.rows().await.len(), 3, "this runner's history is sqlx's");
}

/// The refusals are sqlx's: a failed row, a version this build lacks, a text that changed. And a
/// migration whose SQL fails leaves no row and none of its work.
#[tokio::test]
async fn the_history_is_checked_before_anything_runs() {
    let Some(db) = Db::new().await else {
        return;
    };
    let two = dir(&[ONE, TWO]);
    let mut client = db.client().await;
    Migrator::new(TABLE, ours(two.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap();

    let one = dir(&[ONE]);
    let missing = Migrator::new(TABLE, ours(one.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap_err();
    assert!(
        matches!(missing, MigrateError::VersionMissing(2)),
        "{missing:?}"
    );

    let changed = dir(&[ONE, ("0002_create_b.sql", "CREATE TABLE b (id BIGINT);\n")]);
    let mismatch = Migrator::new(TABLE, ours(changed.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap_err();
    assert!(
        matches!(mismatch, MigrateError::VersionMismatch(2)),
        "{mismatch:?}"
    );

    let broken = dir(&[
        ONE,
        TWO,
        ("0003_create_c.sql", "CREATE TABLE c (id INT);\nNOT SQL;\n"),
    ]);
    let failed = Migrator::new(TABLE, ours(broken.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap_err();
    assert!(
        matches!(failed, MigrateError::Execute { version: 3, .. }),
        "{failed:?}"
    );
    assert_eq!(db.rows().await.len(), 2, "no row for the failed migration");
    let c: Option<String> = sqlx::query_scalar("SELECT to_regclass('c')::text")
        .fetch_one(&db.sqlx)
        .await
        .unwrap();
    assert_eq!(c, None, "its work was rolled back");

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {TABLE} SET success = false WHERE version = 2"
    )))
    .execute(&db.sqlx)
    .await
    .unwrap();
    let dirty = Migrator::new(TABLE, ours(two.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap_err();
    assert!(matches!(dirty, MigrateError::Dirty(2)), "{dirty:?}");

    // The lock is released after every refusal: sqlx can take it at once.
    let mut conn = PgConnection::connect_with(&db.options).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), conn.lock())
        .await
        .expect("the lock is free")
        .unwrap();
    conn.unlock().await.unwrap();
}

/// While sqlx's migrator holds its lock, this runner waits; it runs once sqlx lets go.
#[tokio::test]
async fn the_lock_is_sqlx_s_lock() {
    let Some(db) = Db::new().await else {
        return;
    };
    let mut held = PgConnection::connect_with(&db.options).await.unwrap();
    held.lock().await.unwrap();

    let files = dir(&[ONE]);
    let migrator = Migrator::new(TABLE, ours(files.path())).unwrap();
    let mut client = db.client().await;
    let run = tokio::spawn(async move { migrator.run(&mut client).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!run.is_finished(), "it waits for sqlx's lock");

    held.unlock().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("it runs once the lock is free")
        .unwrap()
        .unwrap();
    assert_eq!(db.rows().await, [(1, true)]);
}
