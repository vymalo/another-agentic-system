//! `MemoryNoteStore` over Postgres: a person's memory notes and grants (ADR 0091, migration `0039`).
//!
//! The tables are `user_memories` and `user_memory_settings`, **beside the log**: no event records
//! a note, deleting a thread erases nothing of it, and there is no foreign key to `threads`.
//!
//! - **The owner predicate is in every statement** (`WHERE owner = $1 ...`, and the owner is the
//!   first column of both primary keys): a read of another person's notes finds nothing, a delete of
//!   their id matches no row (the same `false` as for an id that never existed), and a save under
//!   their id is the saver's own note. Nothing of another person's note is ever read out of the
//!   table, so no handler can leak it.
//! - **The limit of [`MAX_NOTES`] is kept under a lock of the owner**, taken before the count: every
//!   [`save_note`](MemoryNoteStore::save_note) runs in a transaction that takes
//!   `pg_advisory_xact_lock(MEMORY_OWNER_LOCK_CLASS, hashtext(owner))` first, then looks whether the
//!   id is held, then counts, then inserts, so writers racing for the last place take turns. The
//!   class is `memo`, a key space of its own.
//! - **Newest first is `ord DESC`**, the identity of the row, never `saved_at`: notes saved at the
//!   same instant keep the order they were saved in.
//! - **A read is one statement**: the matches are counted by `count(*) OVER ()` before the limit cuts
//!   them, so `total` and the notes are of one snapshot.
//! - **The filter is `position(lower(query) IN lower(body))`**, not `LIKE`: `%` and `_` are
//!   characters. `lower` follows the database's collation, so it agrees with Rust's `to_lowercase`
//!   for ASCII and may differ for other scripts under a `C` collation.
//! - **Every write is its own `READ COMMITTED` transaction** with [`LOCK_TIMEOUT`] (2 s).
//! - **Nothing of a note is logged** and no error holds a word of one.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::codec::{to_db, ts};
use crate::error::{DbFailure, SqlError, is_unique_violation};
use crate::sql::{Tx, query, query_as, query_scalar};
use crate::store::PgStore;
use orch_core::{ErrorClass, Timestamp, UserId};
use orch_core::{
    ForgetInput, Grant, MAX_NOTES, MAX_READ_NOTES, MemorySettings, Note, ReadInput, ReadResult,
    SaveInput,
};
use orch_ports::{MemoryNoteStore, MemoryNoteStoreError};

/// How long a write waits for a lock, the owner's first and then the row's, before it gives up as
/// [`MemoryNoteStoreError::Unavailable`]: two seconds (the reasoning of the prompts' store).
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// The `classid` of the owner's advisory lock: the first of the two `int4` arguments of
/// `pg_advisory_xact_lock(int4, int4)`, the second being `hashtext(owner)`. `0x6d65_6d6f` is `memo`:
/// a class of its own, so a save never waits for a prompt's, a preset's or a tool server's.
pub const MEMORY_OWNER_LOCK_CLASS: i32 = 0x6d65_6d6f;

/// The constraint name of the migration.
const PRIMARY_KEY: &str = "user_memories_pk";

/// A note as a read returns it, and how many notes matched before the limit cut them.
type NoteRow = (String, String, jiff::Timestamp, String, i64);

impl PgStore {
    /// Begins a write: `READ COMMITTED`, with the wait for a lock bounded by [`LOCK_TIMEOUT`].
    async fn begin_memory_write(&self) -> Result<Tx, MemoryNoteStoreError> {
        let mut tx = self
            .pool()
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await
            .map_err(db_err)?;
        let bounded = query(format!(
            "SET LOCAL lock_timeout = '{}ms'",
            LOCK_TIMEOUT.as_millis()
        ))
        .execute(&mut *tx)
        .await;
        if let Err(err) = bounded {
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        Ok(tx)
    }
}

impl MemoryNoteStore for PgStore {
    async fn save_note(
        &self,
        owner: &UserId,
        text: &SaveInput,
        written_by: &str,
        id: &str,
        at: Timestamp,
    ) -> Result<Note, MemoryNoteStoreError> {
        // Postgres cannot store U+0000 in text: say so before a statement fails with it
        if text.0.contains('\0') || written_by.contains('\0') || id.contains('\0') {
            return Err(MemoryNoteStoreError::internal(
                "a note holds a character the database cannot store",
            ));
        }
        let mut tx = self.begin_memory_write().await?;
        let taken = query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(MEMORY_OWNER_LOCK_CLASS)
            .bind(owner.as_str())
            .execute(&mut *tx)
            .await;
        if let Err(err) = taken {
            abandon(tx).await;
            return Err(db_err(err));
        }
        // The id first, then the count, as the port promises.
        let exists: bool = query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_memories WHERE owner = $1 AND id = $2)",
        )
        .bind(owner.as_str())
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if exists {
            abandon(tx).await;
            return Err(MemoryNoteStoreError::Exists);
        }
        // The count is taken with the owner's lock held: no other save of this owner is between this
        // count and this insert.
        let held: i64 = query_scalar("SELECT count(*) FROM user_memories WHERE owner = $1")
            .bind(owner.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        if !usize::try_from(held).is_ok_and(|held| held < MAX_NOTES) {
            abandon(tx).await;
            return Err(MemoryNoteStoreError::TooMany { limit: MAX_NOTES });
        }
        let written = ts(at);
        let inserted = query(
            "INSERT INTO user_memories (owner, id, body, written_by, saved_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(owner.as_str())
        .bind(id)
        .bind(text.0.as_str())
        .bind(written_by)
        .bind(to_db(written))
        .execute(&mut *tx)
        .await;
        match inserted {
            Ok(_) => {}
            // a writer that did not take the lock (a repair by hand) got there first
            Err(err) if is_unique_violation(&err, Some(PRIMARY_KEY)) => {
                abandon(tx).await;
                return Err(MemoryNoteStoreError::Exists);
            }
            Err(err) => return Err(db_err(err)),
        }
        tx.commit().await.map_err(db_err)?;
        Ok(Note {
            id: id.to_owned(),
            text: text.0.clone(),
            saved_at: written.to_string(),
            written_by: written_by.to_owned(),
        })
    }

    async fn read_notes(
        &self,
        owner: &UserId,
        input: &ReadInput,
    ) -> Result<ReadResult, MemoryNoteStoreError> {
        // a read returns at least one and at most `MAX_READ_NOTES`, whatever the caller asked for
        let limit = input.limit.clamp(1, MAX_READ_NOTES);
        let rows: Vec<NoteRow> = query_as(
            "SELECT id, body, saved_at, written_by, count(*) OVER () \
             FROM user_memories \
             WHERE owner = $1 \
               AND ($2::text IS NULL OR position(lower($2::text) IN lower(body)) > 0) \
             ORDER BY ord DESC LIMIT $3",
        )
        .bind(owner.as_str())
        .bind(input.query.as_deref())
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        let total = rows
            .first()
            .map_or(Ok(0), |row| usize::try_from(row.4))
            .map_err(|_| MemoryNoteStoreError::Corrupt("a count of notes".to_owned()))?;
        let notes: Vec<Note> = rows.into_iter().map(note).collect();
        Ok(ReadResult {
            truncated: total > notes.len(),
            notes,
            total,
        })
    }

    async fn list_notes(&self, owner: &UserId) -> Result<Vec<Note>, MemoryNoteStoreError> {
        let rows: Vec<NoteRow> = query_as(
            "SELECT id, body, saved_at, written_by, 0::bigint \
             FROM user_memories WHERE owner = $1 \
             ORDER BY ord DESC LIMIT $2",
        )
        .bind(owner.as_str())
        .bind(i64::try_from(MAX_NOTES).unwrap_or(i64::MAX))
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(note).collect())
    }

    async fn forget_note(
        &self,
        owner: &UserId,
        id: &ForgetInput,
    ) -> Result<bool, MemoryNoteStoreError> {
        // the owner is in the statement: another person's id matches no row, as one never saved does
        let gone = query("DELETE FROM user_memories WHERE owner = $1 AND id = $2")
            .bind(owner.as_str())
            .bind(id.0.as_str())
            .execute(self.pool())
            .await
            .map_err(db_err)?;
        Ok(gone.rows_affected() > 0)
    }

    async fn memory_settings(
        &self,
        owner: &UserId,
    ) -> Result<MemorySettings, MemoryNoteStoreError> {
        let rows: Vec<(String, bool, bool)> = query_as(
            "SELECT agent, can_read, can_write FROM user_memory_settings WHERE owner = $1",
        )
        .bind(owner.as_str())
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        let agents: BTreeMap<String, Grant> = rows
            .into_iter()
            .map(|(agent, read, write)| (agent, Grant { read, write }))
            .collect();
        Ok(MemorySettings {
            default: Grant::default(),
            agents,
        })
    }

    async fn set_memory_grant(
        &self,
        owner: &UserId,
        agent: &str,
        grant: Grant,
    ) -> Result<(), MemoryNoteStoreError> {
        if agent.contains('\0') {
            return Err(MemoryNoteStoreError::internal(
                "an agent id holds a character the database cannot store",
            ));
        }
        query(
            "INSERT INTO user_memory_settings (owner, agent, can_read, can_write) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (owner, agent) DO UPDATE \
               SET can_read = EXCLUDED.can_read, can_write = EXCLUDED.can_write",
        )
        .bind(owner.as_str())
        .bind(agent)
        .bind(grant.read)
        .bind(grant.write)
        .execute(self.pool())
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

fn note(row: NoteRow) -> Note {
    let (id, text, saved_at, written_by, _) = row;
    Note {
        id,
        text,
        saved_at: saved_at.to_string(),
        written_by,
    }
}

/// Ends a transaction that wrote nothing, which releases the locks it holds at once.
async fn abandon(tx: Tx) {
    // a failed rollback is a connection that is gone, and the server has released the locks then
    let _ = tx.rollback().await;
}

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends its
/// wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A driver error as the port says it: a row that does not decode is `Corrupt`; a lock that was
/// waited for too long, the network, the pool and the SQLSTATEs that say "try again later" are
/// `Unavailable`; **everything else is `Internal`**, because a retry repeats it.
fn db_err(err: SqlError) -> MemoryNoteStoreError {
    failure_err(DbFailure::of(&err))
}

/// [`db_err`] of the failure as this crate keeps it: its SQLSTATE and class decide, and it is the
/// source (it holds no parameter of the statement, and never a server's `DETAIL`).
fn failure_err(failure: DbFailure) -> MemoryNoteStoreError {
    if failure.code() == Some(LOCK_NOT_AVAILABLE) {
        return MemoryNoteStoreError::unavailable(
            "a person's memory is locked by another writer for longer than a write waits; try again",
        )
        .with_source(failure);
    }
    match failure.class() {
        ErrorClass::Corrupt => {
            MemoryNoteStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        ErrorClass::Transient => {
            MemoryNoteStoreError::unavailable("the database cannot be reached").with_source(failure)
        }
        _ => {
            MemoryNoteStoreError::internal("the database refused the request").with_source(failure)
        }
    }
}
