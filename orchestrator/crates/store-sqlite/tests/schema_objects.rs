//! The fresh-database test of ADR 0107 decision 6 for SQLite: the store's migrations, applied to an
//! empty file, create exactly the triggers and views [`OBJECTS`] lists (none before slice S1). A
//! slice that adds, replaces or drops one moves this list in the same pull request, so one a
//! migration leaves behind, or one it forgot, fails here. Needs no server.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use orch_store_sqlite::SqliteStore;

/// Every trigger and view of the store's file, as `trigger name` or `view name`.
const OBJECTS: &[&str] = &[];

/// A file of its own under the process's temporary directory, removed with its directory when the
/// test ends.
struct File(PathBuf);

impl File {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "orch-store-sqlite-objects-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.join("store.sqlite")
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A migrated store on `path`, closed again (its lock file let go).
async fn migrate(path: &Path) {
    let store = SqliteStore::open(path).unwrap();
    store.migrate().await.unwrap();
    drop(store);
}

/// The triggers and views of the file, sorted.
fn objects(conn: &rusqlite::Connection) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT type || ' ' || name FROM sqlite_schema \
             WHERE type IN ('trigger', 'view') ORDER BY 1",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn every_trigger_and_view_is_listed() {
    let file = File::new();
    migrate(&file.path()).await;
    let conn = rusqlite::Connection::open(file.path()).unwrap();
    let mut listed: Vec<&str> = OBJECTS.to_vec();
    listed.sort_unstable();
    assert_eq!(
        objects(&conn),
        listed,
        "the migrations' triggers and views differ from OBJECTS: a slice that adds, changes or \
         drops one lists it here (ADR 0107 decision 6)"
    );
}

/// The listing sees what a migration would leave: a stray trigger and view are found, so the
/// test above cannot pass on a query that sees nothing.
#[tokio::test]
async fn a_stray_trigger_or_view_is_found() {
    let file = File::new();
    migrate(&file.path()).await;
    let conn = rusqlite::Connection::open(file.path()).unwrap();
    conn.execute_batch(
        "CREATE TABLE stray (x INTEGER); \
         CREATE TRIGGER orch_stray BEFORE INSERT ON stray BEGIN SELECT RAISE(ABORT, 'orch:stray'); END; \
         CREATE VIEW orch_stray_view AS SELECT 1 AS one;",
    )
    .unwrap();
    assert_eq!(
        objects(&conn),
        ["trigger orch_stray", "view orch_stray_view"]
    );
}
