//! The schema and its runner.
//!
//! `orch-pg-migrate` is not reusable here: it speaks `tokio-postgres` and continues the history
//! `sqlx` kept, and neither exists in a SQLite file (ADR 0076 decision 3). This module is the
//! small runner instead, with the shape this repository expects of an adapter: a `migrations/`
//! directory, a version table, a hash per file, and a `migrate()` that is idempotent.
//!
//! What the version table holds is `schema_migrations` (`version`, `description`, `checksum`,
//! `applied_at`), not `sqlx`'s `_sqlx_migrations`: a SQLite file of this store has no history to
//! continue, and naming the table after `sqlx` would claim one.
//!
//! The checksum is the SHA-256 of the file's bytes as it is compiled in. A file that changed after
//! it was applied is [`StoreError::Corrupt`] — the file and the database disagree about what was
//! run, and guessing which is right is how a schema drifts.
//!
//! The runner is one `BEGIN IMMEDIATE` transaction, so a crash leaves either no migration applied
//! or all of them, and no other process can be half-way through one (the lock file refuses a
//! second process anyway, [`open`](crate::SqliteStore::open)).

use orch_ports::StoreError;
use rusqlite::{Connection, TransactionBehavior};
use sha2::{Digest, Sha256};

use crate::error::store_err;

/// The table that records what ran.
pub const MIGRATIONS_TABLE: &str = "schema_migrations";

/// One migration: its number, its name (the file's, without `.sql`) and its statements.
pub struct Migration {
    /// The version, from 1, in the order they run.
    pub version: i64,
    /// The file's name without its extension.
    pub description: &'static str,
    /// The file's text.
    pub sql: &'static str,
}

/// The migrations, in order. A file added to `migrations/` and not to this list would never run,
/// so the test at the bottom of this module fails when the two disagree.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        description: "usage_and_search",
        sql: include_str!("../migrations/0002_usage_and_search.sql"),
    },
    Migration {
        version: 3,
        description: "user_prompts",
        sql: include_str!("../migrations/0003_user_prompts.sql"),
    },
    Migration {
        version: 4,
        description: "thread_pick",
        sql: include_str!("../migrations/0004_thread_pick.sql"),
    },
    Migration {
        version: 5,
        description: "user_tool_servers",
        sql: include_str!("../migrations/0005_user_tool_servers.sql"),
    },
    Migration {
        version: 6,
        description: "user_tool_oauth_pending",
        sql: include_str!("../migrations/0006_user_tool_oauth_pending.sql"),
    },
];

/// The SHA-256 of a migration's text, lowercase hex: the `checksum` column.
fn checksum(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Applies every migration the file has not seen. Idempotent: a second call does nothing, and a
/// call after a crash finishes what the crash left (there is nothing to finish: the run is one
/// transaction).
///
/// # Errors
/// [`StoreError::Corrupt`] when an applied migration's file changed afterwards;
/// [`StoreError::Internal`] when SQLite rejects a migration's statements.
pub(crate) fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    tx.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
             version     INTEGER PRIMARY KEY,
             description TEXT NOT NULL,
             checksum    TEXT NOT NULL,
             applied_at  INTEGER NOT NULL
         )"
    ))
    .map_err(store_err)?;
    for migration in MIGRATIONS {
        let applied: Option<String> = tx
            .query_row(
                &format!("SELECT checksum FROM {MIGRATIONS_TABLE} WHERE version = ?1"),
                [migration.version],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(store_err(other)),
            })?;
        let checksum = checksum(migration.sql);
        match applied {
            Some(recorded) if recorded == checksum => {}
            Some(_) => {
                return Err(StoreError::corrupt(format!(
                    "migration {} ({}) was edited after it was applied",
                    migration.version, migration.description
                )));
            }
            None => {
                tx.execute_batch(migration.sql).map_err(|e| {
                    StoreError::internal(format!(
                        "migration {} ({}) failed: {e}",
                        migration.version, migration.description
                    ))
                })?;
                tx.execute(
                    &format!(
                        "INSERT INTO {MIGRATIONS_TABLE} (version, description, checksum, applied_at) \
                         VALUES (?1, ?2, ?3, CAST(unixepoch('subsec') * 1000000 AS INTEGER))"
                    ),
                    rusqlite::params![migration.version, migration.description, checksum],
                )
                .map_err(store_err)?;
                tracing::debug!(
                    version = migration.version,
                    description = migration.description,
                    "migration applied"
                );
            }
        }
    }
    tx.commit().map_err(store_err)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;

    /// The files of `migrations/` are the ones this module compiles in, once each, in order: a file
    /// added to the directory and not to [`MIGRATIONS`] would never run, and one listed twice would
    /// run twice.
    #[test]
    fn the_migrations_listed_are_the_files_of_the_directory() {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"));
        let mut on_disk = BTreeMap::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "sql") {
                let name = path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_end_matches(".sql")
                    .to_owned();
                let text = std::fs::read_to_string(&path).unwrap();
                on_disk.insert(name, text);
            }
        }
        // The directory names a file `<version>_<description>.sql`, so the key is that stem: a
        // migration whose description or number drifts from its file fails here.
        let listed: BTreeMap<String, String> = MIGRATIONS
            .iter()
            .map(|m| {
                (
                    format!("{:04}_{}", m.version, m.description),
                    m.sql.to_owned(),
                )
            })
            .collect();
        assert_eq!(
            listed.keys().collect::<Vec<_>>(),
            on_disk.keys().collect::<Vec<_>>(),
            "the list and the directory name the same files"
        );
        assert_eq!(listed, on_disk, "and the same text");
        let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        assert_eq!(versions, (1..=MIGRATIONS.len() as i64).collect::<Vec<_>>());
        assert_eq!(MIGRATIONS[0].description, "init");
    }

    /// The bundled SQLite is at least 3.35. That is what `DELETE … RETURNING` needs
    /// (`PendingAuthorizations::take`) and what this store's own
    /// `UPDATE … RETURNING` and `AS MATERIALIZED` already run on. The version is the one
    /// `libsqlite3-sys` 0.39.1 bundles, which is the one adam-rs's `adam-store-sqlite` bundles at
    /// our pin (`7ac3281`): one build, one `sqlite3` (adam ADR 0053 decision 2). A bump of
    /// `rusqlite` that lowered it fails here, not in a statement.
    #[test]
    fn the_bundled_sqlite_is_at_least_three_thirty_five() {
        let number = rusqlite::version_number();
        assert!(
            number >= 3_035_000,
            "the bundled SQLite is {} ({number}): `DELETE ... RETURNING` needs 3.35.0 or later",
            rusqlite::version()
        );
    }

    #[test]
    fn a_migrated_file_migrates_once_and_a_changed_file_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {MIGRATIONS_TABLE}"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);

        // what an edited file looks like: the recorded checksum no longer matches
        conn.execute(
            &format!("UPDATE {MIGRATIONS_TABLE} SET checksum = 'edited' WHERE version = 1"),
            [],
        )
        .unwrap();
        let refused = migrate(&mut conn);
        assert!(
            matches!(refused, Err(StoreError::Corrupt { .. })),
            "an edited migration is refused, not re-run"
        );
    }
}
