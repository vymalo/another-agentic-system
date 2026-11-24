//! The `TeamPresetStore` testkit against SQLite (ADR 0074, ADR 0076 E2).
//!
//! Every case gets a store of its own, on a file of its own in the process's temporary directory: the
//! suite needs no docker, no Postgres and no `ORCH_TEST_DATABASE_URL`. `make` returns `Some`, never
//! `None`: a skipped suite prints green and proves nothing.
//!
//! A file of its own: the macro writes the names of its cases into the module, and some of them are
//! a prompt suite's too (`tests/conformance.rs`), so both in one module would not compile.
#![allow(missing_docs)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use orch_store_sqlite::SqliteStore;

/// The directory of this process's stores, made once.
fn directory() -> &'static std::path::Path {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = std::env::temp_dir().join(format!(
            "orch-store-sqlite-team-presets-{}",
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

orch_ports::team_preset_store_conformance!(make);
