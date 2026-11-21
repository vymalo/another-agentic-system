//! This runner continues the history sqlx 0.9 kept (sys#382 E1): every migration of the workspace
//! that sqlx ever applied reads to the version, description, checksum and transaction sqlx gave it
//! (`tests/sqlx-0.9-history.txt`, recorded by sqlx itself before it left the workspace), a history
//! written by sqlx's own statements is a no-op for it, its refusals are sqlx's, and the lock it
//! takes is sqlx's (the key sqlx took for four database names is recorded too, `SQLX_LOCK_KEYS`).
//! sqlx's statements are quoted from `sqlx-postgres` 0.9.0, `src/migrate.rs`. The database tests
//! are skipped unless `ORCH_TEST_DATABASE_URL` is set; each makes a database of its own and drops
//! it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use orch_pg_migrate::{MigrateError, Migration, Migrator, lock_id};

const TABLE: &str = "_compat_migrations";

/// What sqlx 0.9 read from every migration file of the workspace.
const SQLX_HISTORY: &str = include_str!("sqlx-0.9-history.txt");

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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every migration sqlx read has, read by this runner, the version, description, checksum and
/// transaction sqlx gave it, so a history sqlx wrote is this runner's. A migration added since is
/// this runner's alone and needs no line.
#[test]
fn every_migration_sqlx_read_reads_as_sqlx_read_it() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut checked = 0;
    for line in SQLX_HISTORY.lines().filter(|l| !l.starts_with('#')) {
        let mut fields = line.splitn(5, ' ');
        let mut next = || fields.next().unwrap();
        let (krate, version, tx, checksum, description) = (next(), next(), next(), next(), next());
        let dir = crates.join(krate).join("migrations");
        let version: i64 = version.parse().unwrap();
        let mine = ours(&dir)
            .into_iter()
            .find(|m| m.version() == version)
            .unwrap_or_else(|| panic!("{krate} has no migration {version} any more"));
        assert_eq!(
            (
                mine.description(),
                hex(&mine.checksum()).as_str(),
                if mine.no_transaction() { "no-tx" } else { "tx" },
            ),
            (description, checksum, tx),
            "{krate} {version}"
        );
        checked += 1;
    }
    assert!(checked >= 29, "the recording holds every migration of E1");
}

/// The advisory lock key sqlx 0.9's migrator took (`Migrate::lock` on a `PgConnection`), read from
/// `pg_locks` (`classid << 32 | objid`) on a Postgres 16 server for each database name: recorded
/// 2026-11-21 by running sqlx itself, so the waits of `the_lock_is_sqlx_s_lock` are on sqlx's key
/// and not only on this runner's own `lock_id`. Never edit a pair.
const SQLX_LOCK_KEYS: &[(&str, i64)] = &[
    ("orch_test", 2_586_541_127_846_781_434),
    ("orch", 427_233_155_538_191_790),
    ("pgmigrate_compat_1_0", 3_334_769_513_974_460_830),
    ("postgres", 368_907_709_894_077_238),
];

#[test]
fn the_lock_key_is_the_one_sqlx_took() {
    for (database, key) in SQLX_LOCK_KEYS {
        assert_eq!(lock_id(database), *key, "{database}");
    }
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

/// One connection to `config`, its task spawned.
async fn connect(config: &tokio_postgres::Config) -> tokio_postgres::Client {
    let (client, connection) = config.connect(tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(connection);
    client
}

/// A database of its own on the test server.
struct Db {
    name: String,
    admin: tokio_postgres::Config,
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
        let admin: tokio_postgres::Config = url.parse().unwrap();
        let conn = connect(&admin).await;
        // What an earlier run of another process left (it was killed before it dropped them).
        let stale: Vec<String> = conn
            .query(
                "SELECT datname::text FROM pg_database WHERE starts_with(datname, $1) \
                 AND NOT starts_with(datname, $2)",
                &[&PREFIX, &format!("{PREFIX}{pid}_")],
            )
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
        for old in stale {
            let _ = conn
                .batch_execute(&format!("DROP DATABASE IF EXISTS {old} WITH (FORCE)"))
                .await;
        }
        conn.batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let mut config = admin.clone();
        config.dbname(&name);
        Some(Self {
            name,
            admin,
            config,
        })
    }

    async fn client(&self) -> tokio_postgres::Client {
        connect(&self.config).await
    }

    async fn rows(&self) -> Vec<(i64, bool)> {
        self.client()
            .await
            .query(
                &format!("SELECT version, success FROM {TABLE} ORDER BY version"),
                &[],
            )
            .await
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1)))
            .collect()
    }

    /// Drops the database: at the end of each test that made one.
    async fn drop(self) {
        connect(&self.admin)
            .await
            .batch_execute(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.name
            ))
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

/// What sqlx 0.9's migrator did for `migrations` on a fresh database, with its own statements: its
/// table, then each migration and its row in one transaction, the row written with an execution
/// time of -1 that is set after the commit.
async fn sqlx_ran(db: &Db, migrations: &[Migration]) {
    let mut client = db.client().await;
    client
        .batch_execute(&format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
);"
        ))
        .await
        .unwrap();
    for migration in migrations {
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(migration_sql(migration)).await.unwrap();
        tx.execute(
            &format!(
                "INSERT INTO {TABLE} ( version, description, success, checksum, execution_time ) \
                 VALUES ( $1, $2, TRUE, $3, -1 )"
            ),
            &[
                &migration.version(),
                &migration.description(),
                &migration.checksum(),
            ],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        client
            .execute(
                &format!("UPDATE {TABLE} SET execution_time = $1 WHERE version = $2"),
                &[&1_000_i64, &migration.version()],
            )
            .await
            .unwrap();
    }
}

/// The SQL of the files this suite writes, by the migration's version.
fn migration_sql(migration: &Migration) -> &'static str {
    [ONE, TWO, THREE, CONCURRENT]
        .iter()
        .find(|(name, _)| *name == migration.file_name())
        .map(|(_, sql)| *sql)
        .unwrap()
}

async fn installed_on(db: &Db) -> Vec<String> {
    db.client()
        .await
        .query(
            &format!("SELECT installed_on::text FROM {TABLE} ORDER BY version"),
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect()
}

/// A database sqlx migrated is a no-op for this runner, which then applies only what is new (a
/// file that opts out of the transaction included); and sqlx's checks of an existing history pass
/// on what this runner wrote.
#[tokio::test]
async fn a_history_sqlx_wrote_is_continued() {
    let Some(db) = Db::new().await else {
        return;
    };
    let two = dir(&[ONE, TWO]);
    sqlx_ran(&db, &ours(two.path())).await;
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
    let index: Option<String> = client
        .query_one("SELECT to_regclass('a_id')::text", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(index.as_deref(), Some("a_id"));
    let execution: i64 = client
        .query_one(
            &format!("SELECT execution_time FROM {TABLE} WHERE version = 3"),
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(execution >= 0, "the execution time is recorded");

    // And sqlx would continue this runner's history: what it reads before it runs anything (its
    // two statements, quoted from `sqlx-postgres` 0.9.0) finds no dirty row, and every row it
    // compares has the version and checksum sqlx gives the file.
    let dirty = client
        .query(
            &format!("SELECT version FROM {TABLE} WHERE success = false ORDER BY version LIMIT 1"),
            &[],
        )
        .await
        .unwrap();
    assert!(dirty.is_empty(), "no dirty row for sqlx");
    let applied: Vec<(i64, Vec<u8>)> = client
        .query(
            &format!("SELECT version, checksum FROM {TABLE} ORDER BY version"),
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    let expected: Vec<(i64, Vec<u8>)> = ours(four.path())
        .iter()
        .map(|m| (m.version(), m.checksum()))
        .collect();
    assert_eq!(
        applied, expected,
        "sqlx's checks pass on this runner's rows"
    );
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
    let observer = db.client().await;
    let c: Option<String> = observer
        .query_one("SELECT to_regclass('c')::text", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(c, None, "its work was rolled back");

    observer
        .batch_execute(&format!(
            "UPDATE {TABLE} SET success = false WHERE version = 2"
        ))
        .await
        .unwrap();
    let dirty = Migrator::new(TABLE, ours(two.path()))
        .unwrap()
        .run(&mut client)
        .await
        .unwrap_err();
    assert!(matches!(dirty, MigrateError::Dirty(2)), "{dirty:?}");

    // The lock is released after every refusal: another session takes it at once.
    let free: bool = observer
        .query_one("SELECT pg_try_advisory_lock($1)", &[&lock_id(&db.name)])
        .await
        .unwrap()
        .get(0);
    assert!(free, "the lock is free");
    drop(observer);
    drop(client);
    db.drop().await;
}

/// While a session holds sqlx's lock (sqlx's `lock`: `SELECT pg_advisory_lock($1)` with
/// `generate_lock_id(current_database())`), this runner waits for that very lock (the server shows
/// its request, not granted); it runs once the lock is let go.
#[tokio::test]
async fn the_lock_is_sqlx_s_lock() {
    let Some(db) = Db::new().await else {
        return;
    };
    let held = db.client().await;
    held.execute("SELECT pg_advisory_lock($1)", &[&lock_id(&db.name)])
        .await
        .unwrap();

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
    let observer = db.client().await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let waiting: bool = observer
                .query_one(
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' \
                     AND pid = $1 AND NOT granted)",
                    &[&waiter],
                )
                .await
                .unwrap()
                .get(0);
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the runner waits for sqlx's lock");
    assert!(!run.is_finished());

    held.execute("SELECT pg_advisory_unlock($1)", &[&lock_id(&db.name)])
        .await
        .unwrap();
    drop(held);
    let (outcome, client) = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("it runs once the lock is free")
        .unwrap();
    outcome.unwrap();
    assert_eq!(db.rows().await, [(1, true)]);
    drop(observer);
    drop(client);
    db.drop().await;
}
