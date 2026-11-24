//! The port conformance testkit against SQLite (ADR 0009, ADR 0076 E2).
//!
//! Every case gets a store of its own, on a file of its own in the process's temporary directory:
//! the suite needs no docker, no Postgres and no `ORCH_TEST_DATABASE_URL`. `cargo nextest` runs each
//! case in a process of its own, so a case that runs alone leaves nothing behind but a directory the
//! operating system clears.
//!
//! `make` returns `Some`, never `None`: a skipped suite prints green and proves nothing, so this
//! file never gives it the chance (`orch_ports::testkit`).
#![allow(missing_docs)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use orch_store_sqlite::SqliteStore;

/// The directory of this process's stores, made once.
fn directory() -> &'static std::path::Path {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = std::env::temp_dir().join(format!(
            "orch-store-sqlite-conformance-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|e| panic!("a temporary directory: {e}"));
        path
    })
}

/// A fresh, migrated store on a file of its own.
async fn make() -> Option<SqliteStore> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = directory().join(format!("store-{n}.sqlite"));
    let store = SqliteStore::open(&path).unwrap_or_else(|e| panic!("the store opens: {e}"));
    store
        .migrate()
        .await
        .unwrap_or_else(|e| panic!("the schema migrates: {e}"));
    Some(store)
}

orch_ports::thread_store_conformance!(make);

/// The log as a build without the ledger left it (ADR 0065): the ledger is emptied and no thread
/// has been gone through.
async fn forget_usage_ledger(store: &SqliteStore) {
    store
        .forget_usage_ledger()
        .await
        .unwrap_or_else(|e| panic!("empty the ledger: {e}"));
}

/// The log as a build without the search index left it (ADR 0068): the rows are gone and no thread
/// has been gone through.
async fn forget_search_index(store: &SqliteStore) {
    store
        .forget_search_index()
        .await
        .unwrap_or_else(|e| panic!("empty the index: {e}"));
}

/// An index made under an older fold version (ADR 0068): every thread's stored version is lower, so
/// it is unindexed until a commit or the backfill indexes it again.
async fn age_search_index(store: &SqliteStore) {
    store
        .age_search_index()
        .await
        .unwrap_or_else(|e| panic!("lower the fold version: {e}"));
}

orch_ports::usage_ledger_conformance!(make, forget = forget_usage_ledger);
orch_ports::thread_search_conformance!(make, forget = forget_search_index, age = age_search_index);

/// A fresh in-process wakeup: nothing here needs a file.
async fn make_wakeup() -> Option<orch_store_sqlite::SqliteWakeup> {
    Some(orch_store_sqlite::SqliteWakeup::new())
}

orch_ports::wakeup_conformance!(make_wakeup);

orch_ports::prompt_store_conformance!(make);
