//! `TeamPresetStore` over Postgres: a person's team presets (ADR 0074, migration `0032`).
//!
//! The table is `user_team_presets`, **beside the log**: no event records a preset, deleting a thread
//! erases nothing of it, and there is no foreign key to `threads`.
//!
//! - **The owner predicate is in every statement**: a read of another person's preset finds nothing
//!   and both writes match no row, so a preset that is not the caller's is `None` or `NotFound`, the
//!   same answer as for an id that never existed. Nothing of another person's preset is ever read
//!   out of the table, so no handler can leak it.
//! - **The limit of 50 is kept under a lock of the owner**, taken before the count: every
//!   [`insert_team_preset`](TeamPresetStore::insert_team_preset) runs in a transaction that takes
//!   `pg_advisory_xact_lock(TEAM_OWNER_LOCK_CLASS, hashtext(owner))` first, then looks whether the id
//!   is taken, whether the name is, then counts, then inserts, so writers racing for the last place
//!   take turns and the refusals come in the order the port promises. The class is `tprs`, a key
//!   space of its own: not the `prmp` of prompts nor the `usrv` of tool servers.
//! - **A revision is a number of one sequence** (`user_team_preset_revisions`), drawn by the
//!   statement that writes it. A replace is one `UPDATE … WHERE id = $1 AND owner = $2 AND revision
//!   = $expected`: the compare and the swap are one statement.
//! - **Every write is its own `READ COMMITTED` transaction** with [`LOCK_TIMEOUT`] (2 s).
//! - **A row is read back through the rules of `orch_core::team`**: a row that is not a preset is
//!   `Corrupt`, with a message that holds no owner and no word of it. **A listing does not parse the
//!   members' array** (it reads `jsonb_array_length`), so one corrupt row of another person cannot
//!   break everybody's list.
//! - **Nothing of a preset is logged** and no error holds a word of one.

use std::time::Duration;

use crate::codec::{to_db, ts};
use crate::error::{DbFailure, SqlError, is_unique_violation};
use crate::sql::{FromRow, Row as DbRow, Tx, query, query_as, query_scalar};
use crate::store::PgStore;
use orch_core::{
    CheckpointRules, HandoffApproval, TeamMemberInput, TeamPreset, TeamPresetId, TeamPresetInput,
};
use orch_core::{ErrorClass, Timestamp, UserId};
use orch_ports::{
    MAX_TEAM_PRESETS, StoredTeamPreset, TEAM_PRESET_PAGE, TeamPresetPage, TeamPresetRevision,
    TeamPresetStore, TeamPresetStoreError, TeamPresetSummary,
};

/// How long a write waits for a lock, the owner's first and then the row's, before it gives up as
/// [`TeamPresetStoreError::Unavailable`]: two seconds (the reasoning of the prompts' store).
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// The `classid` of the owner's advisory lock: the first of the two `int4` arguments of
/// `pg_advisory_xact_lock(int4, int4)`, the second being `hashtext(owner)`. `0x7470_7273` is `tprs`:
/// a class of its own, so a preset insert never waits for a prompt's or a tool server's.
pub const TEAM_OWNER_LOCK_CLASS: i32 = 0x7470_7273;

/// A row as a read returns it, in the order of [`COLUMNS`].
///
/// A named struct rather than a tuple: this crate's `FromRow` covers tuples of at most **ten**
/// columns (`sql.rs`'s `tuple_from_row!`), and a preset is fifteen. The three lines are the ones the
/// macro writes, with the columns named.
struct PresetRow {
    id: String,
    name: String,
    title: String,
    lead: String,
    members: serde_json::Value,
    charter: String,
    max_handoffs: i32,
    handoff_approval: String,
    handoff_after_seconds: Option<i16>,
    checkpoint_after: Vec<String>,
    checkpoint_before_pull_request: bool,
    push_tokens: i32,
    summary: bool,
    revision: i64,
    updated_at: jiff::Timestamp,
}

impl FromRow for PresetRow {
    fn from_row(row: &DbRow) -> Result<Self, SqlError> {
        Ok(PresetRow {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            title: row.try_get(2)?,
            lead: row.try_get(3)?,
            members: row.try_get(4)?,
            charter: row.try_get(5)?,
            max_handoffs: row.try_get(6)?,
            handoff_approval: row.try_get(7)?,
            handoff_after_seconds: row.try_get(8)?,
            checkpoint_after: row.try_get(9)?,
            checkpoint_before_pull_request: row.try_get(10)?,
            push_tokens: row.try_get(11)?,
            summary: row.try_get(12)?,
            revision: row.try_get(13)?,
            updated_at: row.try_get(14)?,
        })
    }
}

/// A listing row: no members' roles, no charter.
type SummaryRow = (String, String, String, String, i32, i64, jiff::Timestamp);

const COLUMNS: &str = "id, name, title, lead, members, charter, max_handoffs, handoff_approval, \
     handoff_after_seconds, checkpoint_after, checkpoint_before_pull_request, push_tokens, summary, \
     revision, updated_at";

/// The constraint names of the migration.
const PRIMARY_KEY: &str = "user_team_presets_pk";
const NAME_PER_OWNER: &str = "user_team_presets_name_per_owner";

/// What the two columns of the approval mode say, as the row keeps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ask,
    After,
    Auto,
}

impl Mode {
    const fn as_str(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::After => "after",
            Mode::Auto => "auto",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "ask" => Some(Mode::Ask),
            "after" => Some(Mode::After),
            "auto" => Some(Mode::Auto),
            _ => None,
        }
    }
}

impl PgStore {
    /// Begins a write: `READ COMMITTED`, with the wait for a lock bounded by [`LOCK_TIMEOUT`].
    async fn begin_team_write(&self) -> Result<Tx, TeamPresetStoreError> {
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
    async fn begin_team_write_locked(&self, owner: &UserId) -> Result<Tx, TeamPresetStoreError> {
        let mut tx = self.begin_team_write().await?;
        let taken = query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(TEAM_OWNER_LOCK_CLASS)
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

impl TeamPresetStore for PgStore {
    async fn insert_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        at: Timestamp,
    ) -> Result<StoredTeamPreset, TeamPresetStoreError> {
        let mut tx = self.begin_team_write_locked(owner).await?;
        // The id first, then the name, then the count, as the port promises.
        let exists: bool =
            query_scalar("SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE id = $1)")
                .bind(preset.id().as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
        if exists {
            abandon(tx).await;
            return Err(TeamPresetStoreError::Exists);
        }
        let named: bool = query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE owner = $1 AND name = $2)",
        )
        .bind(owner.as_str())
        .bind(preset.name())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        if named {
            abandon(tx).await;
            return Err(TeamPresetStoreError::NameTaken);
        }
        // The count is taken with the owner's lock held: no other insert of this owner is between
        // this count and this insert.
        let held: i64 = query_scalar("SELECT count(*) FROM user_team_presets WHERE owner = $1")
            .bind(owner.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        if !usize::try_from(held).is_ok_and(|held| held < MAX_TEAM_PRESETS) {
            abandon(tx).await;
            return Err(TeamPresetStoreError::TooMany {
                limit: MAX_TEAM_PRESETS,
            });
        }
        let written = ts(at);
        let columns = columns(preset)?;
        let inserted: Result<i64, SqlError> = query_scalar(
            "INSERT INTO user_team_presets \
               (id, owner, name, title, lead, members, charter, max_handoffs, handoff_approval, \
                handoff_after_seconds, checkpoint_after, checkpoint_before_pull_request, \
                push_tokens, summary, revision, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, \
                nextval('user_team_preset_revisions'), $15, $15) \
             RETURNING revision",
        )
        .bind(preset.id().as_str())
        .bind(owner.as_str())
        .bind(&columns.name)
        .bind(&columns.title)
        .bind(&columns.lead)
        .bind(&columns.members)
        .bind(&columns.charter)
        .bind(columns.max_handoffs)
        .bind(columns.mode)
        .bind(columns.after_seconds)
        .bind(&columns.checkpoint_after)
        .bind(columns.checkpoint_before_pull_request)
        .bind(columns.push_tokens)
        .bind(columns.summary)
        .bind(to_db(written))
        .fetch_one(&mut *tx)
        .await;
        let revision = match inserted {
            Ok(revision) => revision,
            // a writer that did not take the lock (a repair by hand) got there first
            Err(err) if is_unique_violation(&err, Some(PRIMARY_KEY)) => {
                abandon(tx).await;
                return Err(TeamPresetStoreError::Exists);
            }
            Err(err) if is_unique_violation(&err, Some(NAME_PER_OWNER)) => {
                abandon(tx).await;
                return Err(TeamPresetStoreError::NameTaken);
            }
            Err(err) => return Err(db_err(err)),
        };
        tx.commit().await.map_err(db_err)?;
        Ok(StoredTeamPreset::new(
            preset.clone(),
            revision_of(revision)?,
            written,
        ))
    }

    async fn get_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
    ) -> Result<Option<StoredTeamPreset>, TeamPresetStoreError> {
        let found: Option<PresetRow> = query_as(format!(
            "SELECT {COLUMNS} FROM user_team_presets WHERE id = $1 AND owner = $2"
        ))
        .bind(id.as_str())
        .bind(owner.as_str())
        .fetch_optional(self.pool())
        .await
        .map_err(db_err)?;
        found.map(stored).transpose()
    }

    async fn list_team_presets(
        &self,
        owner: &UserId,
        after: Option<&TeamPresetId>,
        limit: u32,
    ) -> Result<TeamPresetPage, TeamPresetStoreError> {
        // a page is at least one and at most `PAGE`, whatever the caller asked for
        let limit = i64::from(limit.clamp(1, u32::try_from(TEAM_PRESET_PAGE).unwrap_or(50)));
        let rows: Vec<SummaryRow> = query_as(
            "SELECT id, name, title, lead, jsonb_array_length(members), revision, updated_at \
             FROM user_team_presets \
             WHERE owner = $1 AND ($2::text IS NULL OR id > $2) \
             ORDER BY id LIMIT $3",
        )
        .bind(owner.as_str())
        .bind(after.map(TeamPresetId::as_str))
        .bind(limit + 1)
        .fetch_all(self.pool())
        .await
        .map_err(db_err)?;
        let held: i64 = query_scalar("SELECT count(*) FROM user_team_presets WHERE owner = $1")
            .bind(owner.as_str())
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
        Ok(TeamPresetPage {
            items,
            more,
            held: u32::try_from(held).unwrap_or(u32::MAX),
        })
    }

    async fn replace_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        expected: TeamPresetRevision,
        at: Timestamp,
    ) -> Result<StoredTeamPreset, TeamPresetStoreError> {
        let mut tx = self.begin_team_write().await?;
        let written = ts(at);
        let columns = columns(preset)?;
        let updated: Result<Option<i64>, SqlError> = query_scalar(
            "UPDATE user_team_presets SET name = $3, title = $4, lead = $5, members = $6, \
               charter = $7, max_handoffs = $8, handoff_approval = $9, handoff_after_seconds = $10, \
               checkpoint_after = $11, checkpoint_before_pull_request = $12, push_tokens = $13, \
               summary = $14, revision = nextval('user_team_preset_revisions'), updated_at = $15 \
             WHERE id = $1 AND owner = $2 AND revision = $16 \
             RETURNING revision",
        )
        .bind(preset.id().as_str())
        .bind(owner.as_str())
        .bind(&columns.name)
        .bind(&columns.title)
        .bind(&columns.lead)
        .bind(&columns.members)
        .bind(&columns.charter)
        .bind(columns.max_handoffs)
        .bind(columns.mode)
        .bind(columns.after_seconds)
        .bind(&columns.checkpoint_after)
        .bind(columns.checkpoint_before_pull_request)
        .bind(columns.push_tokens)
        .bind(columns.summary)
        .bind(to_db(written))
        .bind(stored_revision(expected))
        .fetch_optional(&mut *tx)
        .await;
        let revision = match updated {
            Ok(Some(revision)) => revision,
            Ok(None) => {
                // gone or not the owner's: NotFound; there, at another revision: Conflict
                let present: bool = query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE id = $1 AND owner = $2)",
                )
                .bind(preset.id().as_str())
                .bind(owner.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
                abandon(tx).await;
                return Err(if present {
                    TeamPresetStoreError::Conflict
                } else {
                    TeamPresetStoreError::NotFound
                });
            }
            Err(err) if is_unique_violation(&err, Some(NAME_PER_OWNER)) => {
                abandon(tx).await;
                return Err(TeamPresetStoreError::NameTaken);
            }
            Err(err) => return Err(db_err(err)),
        };
        tx.commit().await.map_err(db_err)?;
        Ok(StoredTeamPreset::new(
            preset.clone(),
            revision_of(revision)?,
            written,
        ))
    }

    async fn delete_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
        expected: Option<TeamPresetRevision>,
    ) -> Result<(), TeamPresetStoreError> {
        let mut tx = self.begin_team_write().await?;
        let gone: Option<i64> = query_scalar(
            "DELETE FROM user_team_presets \
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
        let present: bool = query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE id = $1 AND owner = $2)",
        )
        .bind(id.as_str())
        .bind(owner.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        abandon(tx).await;
        Err(if present {
            TeamPresetStoreError::Conflict
        } else {
            TeamPresetStoreError::NotFound
        })
    }
}

/// A preset as the columns hold it, built once for a write.
struct Columns {
    name: String,
    title: String,
    lead: String,
    members: serde_json::Value,
    charter: String,
    max_handoffs: i32,
    mode: &'static str,
    after_seconds: Option<i16>,
    checkpoint_after: Vec<String>,
    checkpoint_before_pull_request: bool,
    push_tokens: i32,
    summary: bool,
}

fn columns(preset: &TeamPreset) -> Result<Columns, TeamPresetStoreError> {
    let input = preset.to_input();
    let members = serde_json::to_value(&input.members).map_err(|e| {
        TeamPresetStoreError::internal("a member of the preset is not one the store can write")
            .with_source(e)
    })?;
    let max_handoffs = i32::try_from(preset.max_handoffs()).map_err(|_| {
        TeamPresetStoreError::internal("the cap of hand-offs is not one the store can write")
    })?;
    let push_tokens = i32::try_from(preset.push_tokens()).map_err(|_| {
        TeamPresetStoreError::internal("the budget of a push is not one the store can write")
    })?;
    let (mode, after_seconds) = match preset.handoff_approval() {
        HandoffApproval::Ask => (Mode::Ask, None),
        HandoffApproval::After { seconds } => (
            Mode::After,
            Some(i16::try_from(seconds).map_err(|_| {
                TeamPresetStoreError::internal("the wait of an approval is not one the store keeps")
            })?),
        ),
        HandoffApproval::Auto => (Mode::Auto, None),
    };
    Ok(Columns {
        name: preset.name().to_owned(),
        title: preset.title().to_owned(),
        lead: preset.lead().as_str().to_owned(),
        members,
        charter: preset.charter().to_owned(),
        max_handoffs,
        mode: mode.as_str(),
        after_seconds,
        checkpoint_after: preset.checkpoints().after.iter().cloned().collect(),
        checkpoint_before_pull_request: preset.checkpoints().before_pull_request,
        push_tokens,
        summary: preset.summary(),
    })
}

/// Ends a transaction that wrote nothing, which releases the locks it holds at once.
async fn abandon(tx: Tx) {
    // a failed rollback is a connection that is gone, and the server has released the locks then
    let _ = tx.rollback().await;
}

fn stored_revision(revision: TeamPresetRevision) -> i64 {
    // one that does not fit is not any revision the store gave: `-1`, which the column's check
    // forbids, so the write matches nothing and is a `Conflict`
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<TeamPresetRevision, TeamPresetStoreError> {
    u64::try_from(revision)
        .map(TeamPresetRevision::new)
        .map_err(|_| {
            TeamPresetStoreError::unavailable("the revision sequence gave a number out of range")
        })
}

/// A row, read back through the rules of `orch_core::team`: whatever is wrong is `Corrupt`, with a
/// sentence that holds no owner and no word of the preset (its id is what an operator looks for).
fn stored(row: PresetRow) -> Result<StoredTeamPreset, TeamPresetStoreError> {
    let PresetRow {
        id,
        name,
        title,
        lead,
        members,
        charter,
        max_handoffs,
        handoff_approval: mode,
        handoff_after_seconds: after_seconds,
        checkpoint_after,
        checkpoint_before_pull_request,
        push_tokens,
        summary,
        revision,
        updated_at,
    } = row;
    let corrupt = |what: &str| {
        TeamPresetStoreError::Corrupt(format!("a stored team preset does not read back: {what}"))
    };
    let id = TeamPresetId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let members: Vec<TeamMemberInput> = serde_json::from_value(members)
        .map_err(|_| corrupt("the members are not members of a team"))?;
    let mode = Mode::parse(&mode).ok_or_else(|| corrupt("the approval mode is not one"))?;
    let handoff_approval = match (mode, after_seconds) {
        (Mode::Ask, None) => HandoffApproval::Ask,
        (Mode::Auto, None) => HandoffApproval::Auto,
        (Mode::After, Some(seconds)) => HandoffApproval::After {
            seconds: u16::try_from(seconds).map_err(|_| corrupt("the wait of an approval"))?,
        },
        _ => return Err(corrupt("the approval mode and its seconds disagree")),
    };
    let input = TeamPresetInput {
        name,
        title,
        lead,
        members,
        charter,
        max_handoffs: u32::try_from(max_handoffs).map_err(|_| corrupt("the cap of hand-offs"))?,
        handoff_approval,
        checkpoints: CheckpointRules {
            after: checkpoint_after.into_iter().collect(),
            before_pull_request: checkpoint_before_pull_request,
        },
        push_tokens: u32::try_from(push_tokens).map_err(|_| corrupt("the budget of a push"))?,
        summary,
    };
    let preset = TeamPreset::new(id.clone(), input).map_err(|e| {
        tracing::warn!(%id, code = e.code, "a person's team preset does not read back");
        corrupt(e.code)
    })?;
    let revision = u64::try_from(revision)
        .map(TeamPresetRevision::new)
        .map_err(|_| corrupt("the revision is not a positive number"))?;
    Ok(StoredTeamPreset::new(preset, revision, updated_at))
}

fn summary(row: SummaryRow) -> Result<TeamPresetSummary, TeamPresetStoreError> {
    let (id, name, title, lead, members, revision, updated_at) = row;
    let corrupt = |what: &str| {
        TeamPresetStoreError::Corrupt(format!("a stored team preset does not read back: {what}"))
    };
    let id = TeamPresetId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let lead = orch_core::MemberName::new(&lead).map_err(|e| corrupt(&e.to_string()))?;
    Ok(TeamPresetSummary {
        id,
        name,
        title,
        lead,
        member_count: u8::try_from(members).map_err(|_| corrupt("a count of members"))?,
        revision: u64::try_from(revision)
            .map(TeamPresetRevision::new)
            .map_err(|_| corrupt("the revision is not a positive number"))?,
        updated_at,
    })
}

/// SQLSTATE `55P03`, `lock_not_available`: what a statement fails with when `lock_timeout` ends its
/// wait for a lock.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// A driver error as the port says it: a row that does not decode is `Corrupt`; a lock that was
/// waited for too long, the network, the pool and the SQLSTATEs that say "try again later" are
/// `Unavailable`; **everything else is `Internal`**, because a retry repeats it.
fn db_err(err: SqlError) -> TeamPresetStoreError {
    failure_err(DbFailure::of(&err))
}

/// [`db_err`] of the failure as this crate keeps it: its SQLSTATE and class decide, and it is the
/// source (it holds no parameter of the statement, and never a server's `DETAIL`).
fn failure_err(failure: DbFailure) -> TeamPresetStoreError {
    if failure.code() == Some(LOCK_NOT_AVAILABLE) {
        return TeamPresetStoreError::unavailable(
            "the team presets are locked by another writer for longer than a write waits; try again",
        )
        .with_source(failure);
    }
    match failure.class() {
        ErrorClass::Corrupt => {
            TeamPresetStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        ErrorClass::Transient => {
            TeamPresetStoreError::unavailable("the database cannot be reached").with_source(failure)
        }
        _ => {
            TeamPresetStoreError::internal("the database refused the request").with_source(failure)
        }
    }
}
