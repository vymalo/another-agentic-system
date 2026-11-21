//! This runner and sqlx 0.9 read and continue each other's history (sys#382 E1): the same
//! checksums for every migration of the workspace, a database migrated by one is a no-op for the
//! other, the refusals match, and the advisory lock is the same lock. The database tests are
//! skipped unless `ORCH_TEST_DATABASE_URL` is set; each makes a database of its own and drops it.
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

/// Every migration directory of the workspace: each file has the version, description, checksum
/// and transaction sqlx gives it, so a history sqlx wrote is this runner's.
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
        let theirs: Vec<(i64, String, Vec<u8>, bool)> = theirs
            .iter()
            .map(|m| {
                (
                    m.version,
                    m.description.to_string(),
                    m.checksum.to_vec(),
                    m.no_tx,
                )
            })
            .collect();
        let mine: Vec<(i64, String, Vec<u8>, bool)> = ours(&dir)
            .iter()
            .map(|m| {
                (
                    m.version(),
                    m.description().to_owned(),
                    m.checksum(),
                    m.no_transaction(),
                )
            })
            .collect();
        assert!(!mine.is_empty(), "{}", dir.display());
        assert_eq!(mine, theirs, "{}", dir.display());
    }
    assert!(dirs >= 2, "store-postgres and secrets-pg have migrations");
}

/// A list that does not name every file of its directory, or names one it does not have, is
/// found.
#[test]
fn a_list_is_checked_against_its_directory() {
    let files = dir(&[ONE, TWO]);
    let both = Migrator::new(TABLE, ours(files.path())).unwrap();
    assert_eq!(both.check_directory(files.path()), Ok(()));
    let only_one = Migrator::new(TABLE, vec![Migration::from_file(ONE.0, ONE.1).unwrap()]).unwrap();
    assert_eq!(
        only_one.check_directory(files.path()),
        Err(vec![format!("unlisted {}", TWO.0)])
    );
    let three = Migrator::new(
        TABLE,
        vec![
            Migration::from_file(ONE.0, ONE.1).unwrap(),
            Migration::from_file(TWO.0, TWO.1).unwrap(),
            Migration::from_file(THREE.0, THREE.1).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(
        three.check_directory(files.path()),
        Err(vec![format!("missing {}", THREE.0)])
    );
}

/// A database of its own on the test server, with a sqlx pool and a tokio-postgres client.
struct Db {
    name: String,
    admin: PgConnectOptions,
    sqlx: PgPool,
    options: PgConnectOptions,
    config: tokio_postgres::Config,
}

/// The prefix of this suite's databases; the process's id follows it.
const PREFIX: &str = "pgmigrate_compat_";

impl Db {
    async fn new() -> Option<Self> {
        let url = std::env::var("ORCH_TEST_DATABASE_URL").ok()?;
        static N: AtomicU32 = AtomicU32::new(0);
        let pid = std::process::id();
        let name = format!("{PREFIX}{pid}_{}", N.fetch_add(1, Ordering::SeqCst));
        let admin: PgConnectOptions = url.parse().unwrap();
        let mut conn = PgConnection::connect_with(&admin).await.unwrap();
        // What an earlier run of another process left (it was killed before it dropped them).
        let stale: Vec<String> = sqlx::query_scalar(
            "SELECT datname FROM pg_database WHERE starts_with(datname, $1) \
             AND NOT starts_with(datname, $2)",
        )
        .bind(PREFIX)
        .bind(format!("{PREFIX}{pid}_"))
        .fetch_all(&mut conn)
        .await
        .unwrap();
        for old in stale {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS {old} WITH (FORCE)"
            )))
            .execute(&mut conn)
            .await;
        }
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut conn)
            .await
            .unwrap();
        let options = admin.clone().database(&name);
        let sqlx = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        let mut config: tokio_postgres::Config = url.parse().unwrap();
        config.dbname(&name);
        Some(Self {
            name,
            admin,
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

    /// Drops the database: at the end of each test that made one.
    async fn drop(self) {
        self.sqlx.close().await;
        let mut conn = PgConnection::connect_with(&self.admin).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut conn)
        .await
        .unwrap();
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
/// A statement no transaction may hold: only a file that opts out can run it.
const CONCURRENT: (&str, &str) = (
    "0004_index_a.sql",
    "-- no-transaction\nCREATE INDEX CONCURRENTLY a_id ON a (id);\n",
);

async fn sqlx_runs(db: &Db, files: &Path) {
    let mut migrator = sqlx::migrate::Migrator::new(files).await.unwrap();
    migrator.dangerous_set_table_name(TABLE);
    migrator.run(&db.sqlx).await.unwrap();
}

async fn installed_on(db: &Db) -> Vec<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT installed_on::text FROM {TABLE} ORDER BY version"
    )))
    .fetch_all(&db.sqlx)
    .await
    .unwrap()
}

/// A database sqlx migrated is a no-op for this runner, which then applies only what is new (a
/// file that opts out of the transaction included); and sqlx then finds nothing to do in what this
/// runner wrote.
#[tokio::test]
async fn each_continues_the_others_history() {
    let Some(db) = Db::new().await else {
        return;
    };
    let two = dir(&[ONE, TWO]);
    sqlx_runs(&db, two.path()).await;
    let installed = installed_on(&db).await;

    let mut client = db.client().await;
    Migrator::new(TABLE, ours(two.path()))
        .unwrap()
        .run(&mut client)
        .await
        .expect("sqlx's history is this runner's");
    assert_eq!(
        installed_on(&db).await,
        installed,
        "nothing was applied again"
    );

    let four = dir(&[ONE, TWO, THREE, CONCURRENT]);
    Migrator::new(TABLE, ours(four.path()))
        .unwrap()
        .run(&mut client)
        .await
        .expect("the new ones, one outside a transaction");
    assert_eq!(
        db.rows().await,
        [(1, true), (2, true), (3, true), (4, true)]
    );
    let index: Option<String> = sqlx::query_scalar("SELECT to_regclass('a_id')::text")
        .fetch_one(&db.sqlx)
        .await
        .unwrap();
    assert_eq!(index.as_deref(), Some("a_id"));
    let execution: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT execution_time FROM {TABLE} WHERE version = 3"
    )))
    .fetch_one(&db.sqlx)
    .await
    .unwrap();
    assert!(execution >= 0, "the execution time is recorded");

    sqlx_runs(&db, four.path()).await;
    assert_eq!(db.rows().await.len(), 4, "this runner's history is sqlx's");
    drop(client);
    db.drop().await;
}

/// The refusals are sqlx's: a failed row, a version this build lacks, a text that changed. A
/// migration whose SQL fails leaves no row and none of its work. The lock is free after each.
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
    conn.close().await.unwrap();
    drop(client);
    db.drop().await;
}

/// While sqlx's migrator holds its lock, this runner waits for that very lock (the server shows
/// its request, not granted); it runs once sqlx lets go.
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
    let waiter: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let run = tokio::spawn(async move {
        let outcome = migrator.run(&mut client).await;
        (outcome, client)
    });
    // An advisory lock this runner's session asked for and was not given: sqlx's.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' \
                 AND pid = $1 AND NOT granted)",
            )
            .bind(waiter)
            .fetch_one(&db.sqlx)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the runner waits for sqlx's lock");
    assert!(!run.is_finished());

    held.unlock().await.unwrap();
    held.close().await.unwrap();
    let (outcome, client) = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("it runs once the lock is free")
        .unwrap();
    outcome.unwrap();
    assert_eq!(db.rows().await, [(1, true)]);
    drop(client);
    db.drop().await;
}
