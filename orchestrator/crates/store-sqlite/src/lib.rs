//! SQLite implementation of the orchestrator's [`ThreadStore`] port (ADR 0076 decision 3, sys#382
//! E2).
//!
//! # One file, one process
//!
//! The store is a single SQLite file in WAL mode, and it belongs to one process (ADR 0076
//! decision 6: a thread has one home, and the home's store holds its log, and nothing else writes
//! it). [`open`](SqliteStore::open) takes an exclusive advisory lock on `<path>.lock`, named after
//! the file's real path so that a symlink and its target share it, and refuses a second process
//! with [`StoreError::Refused`]`("already_open")`. The operating system releases the lock when the
//! process exits, including a `SIGKILL`, so a restart opens the file at once.
//!
//! # What this crate is, in this milestone
//!
//! **`ThreadStore`, all of it**: threads, the append-only log, the binding, the outbox, the inbox,
//! the watches and the purge rows, with the leases and the fences the port's contract states.
//!
//! The port's three supertraits — [`UsageLedger`](orch_ports::UsageLedger),
//! [`ThreadSearch`](orch_ports::ThreadSearch) and [`PromptStore`](orch_ports::PromptStore) — are
//! **declared and not built** (a `ThreadStore` that did not implement them would not compile). Each
//! of their methods answers [`StoreError::Internal`] naming this milestone, and
//! [`PromptStoreError::Internal`](orch_ports::PromptStoreError) for the prompt library: never an
//! empty answer, which a caller could not tell from "nothing matches". They follow as their own
//! commits, with their own tables, their own conformance suites and the `forget` and `age` hooks
//! those suites need. `Wakeup`, `UserToolServerStore` and `PendingAuthorizations` are not part of
//! this crate yet either.
//!
//! # Concurrency
//!
//! `rusqlite` is synchronous. Every call runs on a blocking thread
//! ([`tokio::task::spawn_blocking`]) and takes the one connection behind a semaphore of one, so the
//! async runtime never waits on the file and the calls of one process serialise instead of racing
//! for the write lock. Each change is one `BEGIN IMMEDIATE` transaction; the version check, the
//! outbox and inbox fences, the events, the ledger's rows and the fences' completions are all in
//! it, so a crash leaves none of them.
//!
//! # Schema
//!
//! [`MIGRATIONS_TABLE`] records what ran, with the SHA-256 of each file: an applied
//! migration is never edited, and the runner refuses one that was. [`SqliteStore::migrate`] is
//! idempotent and is one transaction.

#![forbid(unsafe_code)]

mod codec;
mod error;
mod schema;
mod store;
mod unbuilt;

pub use schema::MIGRATIONS_TABLE;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use orch_ports::StoreError;
use rusqlite::Connection;
use rustix::fs::FlockOperation;
use tokio::sync::Semaphore;

/// [`ThreadStore`](orch_ports::ThreadStore) on one SQLite file.
///
/// Cheap to clone: clones share the file, the lock and the connection.
#[derive(Clone)]
pub struct SqliteStore {
    inner: Arc<Inner>,
}

struct Inner {
    /// The one connection: one writer, and every reader serialised behind it.
    conn: Mutex<Connection>,
    /// One permit: a call waits for the connection here, on the runtime, and then on a blocking
    /// thread holds it. Taken before the blocking task starts, so no thread pool worker is ever
    /// waiting for another call to finish.
    permits: Semaphore,
    path: PathBuf,
    /// The lock file, held open: the operating system drops the lock when this is closed, which is
    /// when the store is.
    _lock: std::fs::File,
}

impl std::fmt::Debug for SqliteStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStore")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl SqliteStore {
    /// Opens the store at `path`, creating the file and its lock file if they are not there.
    ///
    /// Takes the exclusive lock of `<path>.lock` and refuses a second process on the same file
    /// with [`StoreError::Refused`]`("already_open")`; sets WAL, `foreign_keys=ON`, a five-second
    /// busy timeout and `synchronous=NORMAL` (WAL's durable-against-a-crash setting: a power loss
    /// may lose the last transactions, an application crash loses none).
    ///
    /// It does not migrate: call [`migrate`](Self::migrate) at boot.
    ///
    /// # Errors
    /// [`StoreError::Refused`]`("already_open")` when another process holds the file;
    /// [`StoreError::Internal`] when the file or its lock cannot be opened, or SQLite refuses a
    /// pragma; [`StoreError::Corrupt`] when the file is not a SQLite database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = real_path(path.as_ref())?;
        let lock = lock_file(&path)?;
        let conn = Connection::open(&path).map_err(error::store_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error::store_err)?;
        // WAL is a property of the file, written once and kept; setting it again is a no-op. It
        // answers the mode it took, so it is read as a row and not run as an update.
        let _mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .map_err(error::store_err)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(error::store_err)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(error::store_err)?;
        Ok(Self {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
                permits: Semaphore::new(1),
                path,
                _lock: lock,
            }),
        })
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Applies the embedded migrations (see [`schema`]). Idempotent; one transaction.
    ///
    /// # Errors
    /// [`StoreError::Corrupt`] when an applied migration's file changed afterwards;
    /// [`StoreError::Internal`] when SQLite rejects a migration's statements.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        self.call(schema::migrate).await
    }

    /// Runs `f` on a blocking thread with the one connection.
    ///
    /// The permit is taken on the runtime first, so a queued call waits without occupying a
    /// blocking thread. A panic inside `f` (or in a statement it runs) is a poisoned connection:
    /// the lock is taken over rather than propagated, because the next statement sees whatever the
    /// transaction left, and a `BEGIN IMMEDIATE` that never committed is rolled back by SQLite.
    pub(crate) async fn call<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let permit = self
            .inner
            .permits
            .acquire()
            .await
            .map_err(|_| StoreError::internal("the store's connection is closed"))?;
        let inner = Arc::clone(&self.inner);
        let joined = tokio::task::spawn_blocking(move || {
            let mut conn = inner.conn.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut conn)
        })
        .await;
        // the permit is released here, after the blocking task is done
        drop(permit);
        joined.map_err(|e| StoreError::internal(format!("the blocking task failed: {e}")))?
    }
}

/// The path a store and its lock are named after: the file's real path when it exists (so a
/// symlink and its target are one file), else the real parent plus the name.
fn real_path(path: &Path) -> Result<PathBuf, StoreError> {
    if let Ok(real) = std::fs::canonicalize(path) {
        return Ok(real);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| StoreError::internal("the store's path has no file name"))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| StoreError::internal(format!("the store's directory cannot be read: {e}")))?;
    Ok(parent.join(name))
}

/// The lock file of `path`, held with an exclusive `flock` for as long as the store lives.
fn lock_file(path: &Path) -> Result<std::fs::File, StoreError> {
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| {
            StoreError::internal(format!(
                "the lock file {} cannot be opened: {e}",
                lock_path.display()
            ))
        })?;
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(file),
        // A second process holds it: the operating system gives it up when that process exits.
        // (`WOULDBLOCK` is `AGAIN` on Linux, so one arm covers both.)
        Err(rustix::io::Errno::WOULDBLOCK) => Err(StoreError::Refused("already_open")),
        Err(e) => Err(StoreError::internal(format!(
            "the lock file {} cannot be locked: {e}",
            lock_path.display()
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A directory of its own for one test, removed when it is dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "orch-store-sqlite-{name}-{}-{}",
                std::process::id(),
                jiff::Timestamp::now().as_nanosecond()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("orch.sqlite")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn a_second_open_of_the_same_file_is_refused() {
        let dir = TempDir::new("lock");
        let store = SqliteStore::open(dir.file()).unwrap();
        store.migrate().await.unwrap();
        let second = SqliteStore::open(dir.file());
        assert!(
            matches!(second, Err(StoreError::Refused("already_open"))),
            "a second process is refused, and says so: {second:?}"
        );
        // a symlink to the same file is the same file
        #[cfg(unix)]
        {
            let link = dir.0.join("link.sqlite");
            std::os::unix::fs::symlink(dir.file(), &link).unwrap();
            let third = SqliteStore::open(&link);
            assert!(matches!(third, Err(StoreError::Refused("already_open"))));
        }
        drop(store);
        // the lock goes with the store
        let again = SqliteStore::open(dir.file()).unwrap();
        again.migrate().await.unwrap();
    }

    #[tokio::test]
    async fn the_file_is_in_wal_and_the_migrations_are_recorded_once() {
        let dir = TempDir::new("wal");
        let store = SqliteStore::open(dir.file()).unwrap();
        store.migrate().await.unwrap();
        store.migrate().await.unwrap();
        let mode: String = store
            .call(|conn| {
                conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .map_err(error::store_err)
            })
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let count: i64 = store
            .call(|conn| {
                let table = schema::MIGRATIONS_TABLE;
                conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .map_err(error::store_err)
            })
            .await
            .unwrap();
        assert_eq!(count, schema::MIGRATIONS.len() as i64);
    }
}
