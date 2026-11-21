//! `PromptStore` over Postgres: a person's prompt library (ADR 0070, migration `0026`).
//!
//! The table is `user_prompts`, **beside the log**: no event records a prompt, deleting a thread
//! erases nothing of it, and there is no foreign key to `threads`.
//!
//! - **The limit of 100 is kept under a lock of the owner**, taken before the count: every
//!   [`insert_prompt`](PromptStore::insert_prompt) runs in a transaction that takes
//!   `pg_advisory_xact_lock(PROMPT_OWNER_LOCK_CLASS, hashtext(owner))` first, then looks whether the
//!   id is taken, whether the name is, then counts, then inserts, so writers racing for the last
//!   place take turns and the second counts the first, and the refusals come in the order the port
//!   promises. The class is `prmp`, a key space of its own: not the `usrv` of tool servers.
//! - **A revision is a number of one sequence** (`user_prompt_revisions`), drawn by the statement
//!   that writes it. A replace is one `UPDATE … WHERE id = $1 AND owner = $2 AND revision =
//!   $expected`: the compare and the swap are one statement.
//! - **Every write is its own `READ COMMITTED` transaction** with [`LOCK_TIMEOUT`] (2 s).
//! - **The shared query is one overlap**, `audience && roles_plus_star`, and **it states the partial
//!   index's own predicate beside it** (`cardinality(audience) > 0 AND audience && $2`): the planner
//!   uses `user_prompts_audience` only for a query whose `WHERE` implies the index's, and
//!   `audience && $2` alone does not (an empty audience overlaps nothing, but the planner does not
//!   know it). The `all` scope is `owner = $1 OR (<that>)`, a `BitmapOr` of the two indexes. The roles
//!   are passed **with `"*"` added**, never `OR audience = ARRAY['*']`, and only the ones that can be in
//!   an audience (a role with a NUL, a space or over 64 bytes matches nothing and is not sent).
//!   `tests/prompts.rs` runs `EXPLAIN` on it.
//! - **A row is read back through [`PromptDefinition::from_stored`]**: a row that is not a prompt
//!   is `Corrupt`, with a message that holds no owner and no word of it. **A listing does not parse
//!   `variables`** (it reads `jsonb_array_length`), so one corrupt row of another person cannot
//!   break everybody's list.
//! - **Nothing of a prompt is logged** and no error holds a word of one.

use std::time::Duration;

use crate::error::{DbFailure, SqlError};
use crate::sql::{Tx, query, query_as, query_scalar};
use orch_core::prompt::{PromptAudience, PromptDefinition, PromptId};
use orch_core::{ErrorClass, Timestamp, UserId};
use orch_ports::{
    MAX_PROMPTS, PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError,
    PromptSummary, PromptViewer, StoredPrompt,
};

use crate::codec::{to_db, ts};
use crate::error::is_unique_violation;
use crate::store::PgStore;

/// How long a write waits for a lock, the owner's first and then the row's, before it gives up as
/// [`PromptStoreError::Unavailable`]: two seconds (the reasoning of the tool servers' store).
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// The `classid` of the owner's advisory lock: the first of the two `int4` arguments of
/// `pg_advisory_xact_lock(int4, int4)`, the second being `hashtext(owner)`. `0x7072_6d70` is
/// `prmp`: a class of its own, so a prompt insert never waits for a tool server's, and the
/// one-`bigint` locks of everything else are a different key space altogether.
pub const PROMPT_OWNER_LOCK_CLASS: i32 = 0x7072_6d70;

/// A row as a read returns it, in the order of [`COLUMNS`].
type Row = (
    String,
    String,
    String,
    String,
    String,
    serde_json::Value,
    Vec<String>,
    i64,
    jiff::Timestamp,
);

/// A row with its owner in front (the owner is read only to say whether the viewer owns it, and goes no further).
type OwnedRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    serde_json::Value,
    Vec<String>,
    i64,
    jiff::Timestamp,
);

const COLUMNS: &str =
    "id, name, title, description, body, variables, audience, revision, updated_at";

/// A listing row: no body, no variables.
type SummaryRow = (
    String,
    String,
    String,
    String,
    i32,
    Vec<String>,
    bool,
    i64,
    jiff::Timestamp,
);

/// The constraint names of the migration.
const PRIMARY_KEY: &str = "user_prompts_pk";
const NAME_PER_OWNER: &str = "user_prompts_name_per_owner";

impl PgStore {
    /// Begins a write: `READ COMMITTED`, with the wait for a lock bounded by [`LOCK_TIMEOUT`].
    async fn begin_prompt_write(&self) -> Result<Tx, PromptStoreError> {
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

    /// Begins a write that holds the owner's lock until it ends.
    async fn begin_prompt_write_locked(&self, owner: &UserId) -> Result<Tx, PromptStoreError> {
        let mut tx = self.begin_prompt_write().await?;
        let taken = query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(PROMPT_OWNER_LOCK_CLASS)
            .bind(owner.as_str())
            .execute(&mut *tx)
            .await;
        if let Err(err) = taken {
            let _ = tx.rollback().await;
            return Err(db_err(err));
        }
        Ok(tx)
    }
}

impl PromptStore for PgStore {
    async fn insert_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        at: Timestamp,
    ) -> Result<StoredPrompt, PromptStoreError> {
        let mut tx = self.begin_prompt_write_locked(owner).await?;
        // The id first, then the name, then the count, as the port promises.
        let exists: bool = query_scalar("SELECT EXISTS (SELECT 1 FROM user_prompts WHERE id = $1)")
            .bind(prompt.id().as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        if exists {
            abandon(tx).await;
            return Err(PromptStoreError::Exists);
        }
        let named: bool = query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_prompts WHERE owner = $1 AND name = $2)",
        )
        .bind(owner.as_str())
        .bind(prompt.name().as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if named {
            abandon(tx).await;
            return Err(PromptStoreError::NameTaken);
        }
        // The count is taken with the owner's lock held: no other insert of this owner is between
        // this count and this insert.
        let held: i64 = query_scalar("SELECT count(*) FROM user_prompts WHERE owner = $1")
            .bind(owner.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        if !usize::try_from(held).is_ok_and(|held| held < MAX_PROMPTS) {
            abandon(tx).await;
            return Err(PromptStoreError::TooMany { limit: MAX_PROMPTS });
        }
        let written = ts(at);
        let inserted: Result<i64, SqlError> = query_scalar(
            "INSERT INTO user_prompts \
               (id, owner, name, title, description, body, variables, audience, revision, \
                created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, nextval('user_prompt_revisions'), $9, $9) \
             RETURNING revision",
        )
        .bind(prompt.id().as_str())
        .bind(owner.as_str())
        .bind(prompt.name().as_str())
        .bind(prompt.title())
        .bind(prompt.description())
        .bind(prompt.body())
        .bind(prompt.variables().to_value())
        .bind(prompt.audience().values())
        .bind(to_db(written))
        .fetch_one(&mut *tx)
        .await;
        let revision = match inserted {
            Ok(revision) => revision,
            // a writer that did not take the lock (a repair by hand) got there first
            Err(err) if is_unique_violation(&err, Some(PRIMARY_KEY)) => {
                abandon(tx).await;
                return Err(PromptStoreError::Exists);
            }
            Err(err) if is_unique_violation(&err, Some(NAME_PER_OWNER)) => {
                abandon(tx).await;
                return Err(PromptStoreError::NameTaken);
            }
            Err(err) => return Err(db_err(err)),
        };
        tx.commit().await.map_err(db_err)?;
        Ok(StoredPrompt::new(
            prompt.clone(),
            revision_of(revision)?,
            written,
            true,
        ))
    }

    async fn get_prompt(
        &self,
        viewer: &PromptViewer,
        id: &PromptId,
    ) -> Result<Option<StoredPrompt>, PromptStoreError> {
        let found: Option<OwnedRow> = query_as(format!(
            "SELECT owner, {COLUMNS} FROM user_prompts \
             WHERE id = $1 AND (owner = $2 OR (cardinality(audience) > 0 AND audience && $3))"
        ))
        .bind(id.as_str())
        .bind(viewer.user.as_str())
        .bind(roles_plus_star(viewer))
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        found
            .map(
                |(owner, id, name, title, description, body, variables, audience, revision, at)| {
                    let row = (
                        id,
                        name,
                        title,
                        description,
                        body,
                        variables,
                        audience,
                        revision,
                        at,
                    );
                    stored(row, owner == viewer.user.as_str())
                },
            )
            .transpose()
    }

    async fn list_prompts(
        &self,
        viewer: &PromptViewer,
        scope: PromptScope,
        after: Option<&PromptId>,
        limit: u32,
    ) -> Result<PromptPage, PromptStoreError> {
        let limit = i64::from(limit.min(u32::try_from(MAX_PROMPTS).unwrap_or(100)));
        let rows: Vec<SummaryRow> = query_as(list_sql(scope))
            .bind(viewer.user.as_str())
            .bind(roles_plus_star(viewer))
            .bind(after.map(PromptId::as_str))
            .bind(limit + 1)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
        let held: i64 = query_scalar("SELECT count(*) FROM user_prompts WHERE owner = $1")
            .bind(viewer.user.as_str())
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?;
        let page = usize::try_from(limit).unwrap_or(usize::MAX);
        let more = rows.len() > page;
        let items = rows
            .into_iter()
            .take(page)
            .map(summary)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PromptPage {
            items,
            more,
            held: u32::try_from(held).unwrap_or(u32::MAX),
        })
    }

    async fn replace_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        expected: PromptRevision,
        at: Timestamp,
    ) -> Result<StoredPrompt, PromptStoreError> {
        let mut tx = self.begin_prompt_write().await?;
        let written = ts(at);
        let updated: Result<Option<i64>, SqlError> = query_scalar(
            "UPDATE user_prompts SET name = $3, title = $4, description = $5, body = $6, \
               variables = $7, audience = $8, revision = nextval('user_prompt_revisions'), \
               updated_at = $9 \
             WHERE id = $1 AND owner = $2 AND revision = $10 \
             RETURNING revision",
        )
        .bind(prompt.id().as_str())
        .bind(owner.as_str())
        .bind(prompt.name().as_str())
        .bind(prompt.title())
        .bind(prompt.description())
        .bind(prompt.body())
        .bind(prompt.variables().to_value())
        .bind(prompt.audience().values())
        .bind(to_db(written))
        .bind(stored_revision(expected))
        .fetch_optional(&mut *tx)
        .await;
        let revision = match updated {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                // gone or not the owner's: NotFound; there, at another revision: Conflict
                let present: bool = query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM user_prompts WHERE id = $1 AND owner = $2)",
                )
                .bind(prompt.id().as_str())
                .bind(owner.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
                abandon(tx).await;
                return Err(if present {
                    PromptStoreError::Conflict
                } else {
                    PromptStoreError::NotFound
                });
            }
            Err(err) if is_unique_violation(&err, Some(NAME_PER_OWNER)) => {
                abandon(tx).await;
                return Err(PromptStoreError::NameTaken);
            }
            Err(err) => return Err(db_err(err)),
        };
        tx.commit().await.map_err(db_err)?;
        Ok(StoredPrompt::new(
            prompt.clone(),
            revision_of(revision)?,
            written,
            true,
        ))
    }

    async fn delete_prompt(
        &self,
        owner: &UserId,
        id: &PromptId,
        expected: Option<PromptRevision>,
    ) -> Result<(), PromptStoreError> {
        let mut tx = self.begin_prompt_write().await?;
        let gone: Option<i64> = query_scalar(
            "DELETE FROM user_prompts \
             WHERE id = $1 AND owner = $2 AND ($3::bigint IS NULL OR revision = $3) \
             RETURNING revision",
        )
        .bind(id.as_str())
        .bind(owner.as_str())
        .bind(expected.map(stored_revision))
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        if gone.is_some() {
            return tx.commit().await.map_err(db_err);
        }
        let present: bool =
            query_scalar("SELECT EXISTS (SELECT 1 FROM user_prompts WHERE id = $1 AND owner = $2)")
                .bind(id.as_str())
                .bind(owner.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
        abandon(tx).await;
        Err(if present {
            PromptStoreError::Conflict
        } else {
            PromptStoreError::NotFound
        })
    }
}

/// The statement of a listing, with its parameters: `$1` the viewer, `$2` their roles with `*`
/// added, `$3` the cursor (or null) and `$4` the limit plus one. It is a function of its own so
/// that `tests/prompts.rs` can ask Postgres for the plan of exactly this text.
pub fn list_sql(scope: PromptScope) -> String {
    let predicate = match scope {
        // `$2` is named in every scope: Postgres cannot type a parameter no statement mentions
        PromptScope::Mine => "owner = $1 AND cardinality($2::text[]) > 0",
        // `cardinality(audience) > 0` is the predicate of the partial index: without it the planner
        // cannot use `user_prompts_audience` (and the overlap alone would be a scan of the table)
        PromptScope::Shared => "owner <> $1 AND cardinality(audience) > 0 AND audience && $2",
        PromptScope::All => "(owner = $1 OR (cardinality(audience) > 0 AND audience && $2))",
    };
    format!(
        "SELECT id, name, title, description, jsonb_array_length(variables), audience, \
           owner = $1, revision, updated_at \
         FROM user_prompts \
         WHERE {predicate} AND ($3::text IS NULL OR id > $3) \
         ORDER BY id LIMIT $4"
    )
}

/// The roles the viewer has, **with `"*"` added**: the one overlap `audience && $roles` that the
/// partial GIN index answers, for a prompt shared with a role of theirs and for one shared with
/// everybody.
fn roles_plus_star(viewer: &PromptViewer) -> Vec<String> {
    let mut roles: Vec<String> = viewer
        .roles
        .iter()
        .filter(|r| PromptAudience::is_role_value(r))
        .cloned()
        .collect();
    roles.push("*".to_owned());
    roles
}

/// Ends a transaction that wrote nothing, which releases the locks it holds at once.
async fn abandon(tx: Tx) {
    // a failed rollback is a connection that is gone, and the server has released the locks then
    let _ = tx.rollback().await;
}

fn stored_revision(revision: PromptRevision) -> i64 {
    // one that does not fit is not any revision the store gave: `-1`, which the column's check
    // forbids, so the write matches nothing and is a `Conflict`
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<PromptRevision, PromptStoreError> {
    u64::try_from(revision)
        .map(PromptRevision::new)
        .map_err(|_| {
            PromptStoreError::unavailable("the revision sequence gave a number out of range")
        })
}

/// A row, read back through [`PromptDefinition::from_stored`]: whatever is wrong is `Corrupt`, with
/// a sentence that holds no owner and no word of the prompt (its id is what an operator looks for).
fn stored(row: Row, own: bool) -> Result<StoredPrompt, PromptStoreError> {
    let (id, name, title, description, body, variables, audience, revision, updated_at) = row;
    let corrupt = |what: &str| {
        PromptStoreError::Corrupt(format!("a stored prompt does not read back: {what}"))
    };
    let id = PromptId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let prompt = PromptDefinition::from_stored(
        id.clone(),
        &name,
        &title,
        &description,
        &body,
        variables,
        &audience,
    )
    .map_err(|e| {
        tracing::warn!(%id, code = e.code, "a person's prompt does not read back");
        corrupt(e.code)
    })?;
    let revision = u64::try_from(revision)
        .map(PromptRevision::new)
        .map_err(|_| corrupt("the revision is not a positive number"))?;
    Ok(StoredPrompt::new(prompt, revision, updated_at, own))
}

fn summary(row: SummaryRow) -> Result<PromptSummary, PromptStoreError> {
    let (id, name, title, description, variables, audience, own, revision, updated_at) = row;
    let corrupt = |what: &str| {
        PromptStoreError::Corrupt(format!("a stored prompt does not read back: {what}"))
    };
    let id = PromptId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let audience = if own {
        Some(PromptAudience::from_values(&audience).map_err(|e| corrupt(e.code))?)
    } else {
        None
    };
    Ok(PromptSummary {
        id,
        name,
        title,
        description,
        variable_count: u8::try_from(variables).map_err(|_| corrupt("a count of variables"))?,
        audience,
        own,
        revision: u64::try_from(revision)
            .map(PromptRevision::new)
            .map_err(|_| corrupt("the revision is not a positive number"))?,
        updated_at,
    })
}

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends its
/// wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A driver error as the port says it: a row that does not decode is `Corrupt`; a lock that was
/// waited for too long, the network, the pool and the SQLSTATEs that say "try again later" are
/// `Unavailable` (a 503 a client may retry); **everything else is `Internal`**, because a retry
/// repeats it: a value the database cannot hold (22P05, a NUL in a `jsonb` string), a violated
/// constraint nobody maps, a rejected statement. The driver's error is the source; it holds no
/// parameter of the statement, so no word of a prompt and no owner.
fn db_err(err: SqlError) -> PromptStoreError {
    failure_err(DbFailure::of(&err))
}

/// [`db_err`] of the failure as this crate keeps it: its SQLSTATE and class decide, and it is the
/// source (it holds no parameter of the statement, and never a server's `DETAIL`).
fn failure_err(failure: DbFailure) -> PromptStoreError {
    if failure.code() == Some(LOCK_NOT_AVAILABLE) {
        return PromptStoreError::unavailable(
            "the prompts are locked by another writer for longer than a write waits; try again",
        )
        .with_source(failure);
    }
    match failure.class() {
        ErrorClass::Corrupt => {
            PromptStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        ErrorClass::Transient => {
            PromptStoreError::unavailable("the database cannot be reached").with_source(failure)
        }
        _ => PromptStoreError::internal("the database refused the request").with_source(failure),
    }
}

#[cfg(test)]
mod tests {
    use orch_core::ErrorClass;
    use orch_ports::PromptStoreError;

    use super::failure_err;
    use crate::error::DbFailure;

    fn class_of(code: &'static str) -> ErrorClass {
        use orch_core::Classify;
        failure_err(DbFailure::sqlstate(code)).class()
    }

    #[test]
    fn what_a_retry_repeats_is_not_unavailable() {
        // 22P05 untranslatable character (a NUL in a jsonb string), 22021 invalid byte sequence,
        // 22001 value too long, 23502 not null, 23514 check violation, 42601 syntax error
        for code in [
            "22P05", "22021", "22001", "22003", "23502", "23514", "42601", "42P01",
        ] {
            assert_eq!(class_of(code), ErrorClass::Internal, "{code}");
        }
        // the lock wait, a serialization failure, a deadlock, a connection that is gone, a shutdown
        for code in ["55P03", "40001", "40P01", "08006", "53300", "57P01"] {
            assert_eq!(class_of(code), ErrorClass::Transient, "{code}");
        }
        assert!(matches!(
            failure_err(DbFailure::sqlstate("22P05")),
            PromptStoreError::Internal { .. }
        ));
    }
}
