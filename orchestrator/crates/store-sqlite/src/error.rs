//! How a `rusqlite` failure becomes a [`StoreError`] (ADR 0009: no driver type in a port).
//!
//! The three cases that matter, and the only three this store distinguishes:
//!
//! * **busy or locked** — another connection of the same file holds the write lock past the busy
//!   timeout, or a reader blocks a checkpointer. Nothing was written; a caller retries, so it is
//!   [`StoreError::Unavailable`]. One process writes here (ADR 0076 decision 6), so this is a
//!   lock that outlived the timeout, not a second writer: a second *process* is refused by the
//!   lock file in [`open`](crate::SqliteStore::open), not here.
//! * **a row that does not read as the port's type, or a file that is not a database** — the data
//!   is not what this store wrote, so [`StoreError::Corrupt`]: never silently a default.
//! * **anything else** — a statement SQLite rejects is a bug or a schema older than the build,
//!   which no retry fixes: [`StoreError::Internal`].
//!
//! A unique-constraint failure is not mapped here: what it means depends on the statement (an
//! existing thread id, a child session another caller opened, a nonce in use), so the statements
//! that can hit one read it themselves ([`unique_on`]).

use orch_ports::StoreError;
use rusqlite::Error as SqliteError;
use rusqlite::ffi::{ErrorCode, SQLITE_CONSTRAINT_PRIMARYKEY, SQLITE_CONSTRAINT_UNIQUE};

/// A driver failure as a port error.
pub(crate) fn store_err(e: SqliteError) -> StoreError {
    match &e {
        SqliteError::SqliteFailure(code, _) => match code.code {
            ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => StoreError::unavailable(e),
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
                StoreError::corrupt_with("the SQLite file is not a database this store can read", e)
            }
            _ => StoreError::internal(e),
        },
        SqliteError::InvalidColumnType(..)
        | SqliteError::InvalidColumnName(..)
        | SqliteError::FromSqlConversionFailure(..)
        | SqliteError::Utf8Error(..) => {
            StoreError::corrupt_with("a row does not read as the port's type", e)
        }
        SqliteError::QueryReturnedNoRows => {
            StoreError::internal("a statement that must return a row returned none")
        }
        _ => StoreError::internal(e),
    }
}

/// Whether `e` is a unique or primary-key violation whose message names `columns` (the column list
/// SQLite prints, for example `threads.child_of, threads.child_session`).
///
/// SQLite names the columns of the index that refused the row and nothing else, so a caller that
/// wants to know *which* uniqueness it hit asks for the columns of that index. A message that names
/// no column of the wanted set (a table with one unique index per statement) can be matched with
/// `""`, which every message contains.
pub(crate) fn unique_on(e: &SqliteError, columns: &str) -> bool {
    let SqliteError::SqliteFailure(code, Some(message)) = e else {
        return false;
    };
    let unique = code.extended_code == SQLITE_CONSTRAINT_UNIQUE
        || code.extended_code == SQLITE_CONSTRAINT_PRIMARYKEY;
    unique && message.contains(columns)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn unique(columns: &str) -> SqliteError {
        SqliteError::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::ConstraintViolation,
                extended_code: SQLITE_CONSTRAINT_UNIQUE,
            },
            Some(format!("UNIQUE constraint failed: {columns}")),
        )
    }

    #[test]
    fn a_unique_violation_is_read_by_the_columns_it_names() {
        let e = unique("threads.child_of, threads.child_session");
        assert!(unique_on(&e, "threads.child_of"));
        assert!(unique_on(&e, ""));
        assert!(!unique_on(&e, "threads.share_nonce"));
        assert!(!unique_on(&SqliteError::QueryReturnedNoRows, ""));
    }

    #[test]
    fn a_busy_database_is_transient_and_a_corrupt_one_is_corrupt() {
        let busy = SqliteError::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseBusy,
                extended_code: 5,
            },
            None,
        );
        assert!(matches!(store_err(busy), StoreError::Unavailable { .. }));

        let corrupt = SqliteError::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::NotADatabase,
                extended_code: 26,
            },
            None,
        );
        assert!(matches!(store_err(corrupt), StoreError::Corrupt { .. }));

        assert!(matches!(
            store_err(SqliteError::QueryReturnedNoRows),
            StoreError::Internal { .. }
        ));
    }
}
