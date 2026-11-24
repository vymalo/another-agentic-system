//! `TeamPresetStore` over SQLite: a person's team presets (ADR 0074, migration `0007`).
//!
//! The table is `user_team_presets`, **beside the log**: no event records a preset and deleting a
//! thread erases nothing of it.
//!
//! - **One writer, one transaction.** Postgres needs an advisory lock of the owner so that the limit
//!   of 50 holds under racing inserts; here every write is a `BEGIN IMMEDIATE` transaction on the one
//!   connection, so the id, the name and the count are read and the row is inserted with nobody
//!   between them, and the refusals come in the order the port promises.
//! - **The owner predicate is in every statement**: a preset that is not the caller's is `None` or
//!   `NotFound`, the same answer as for an id that never existed.
//! - **A revision is a number of one counter** (`user_team_preset_counter`), drawn by the statement
//!   that writes it and never given twice, a delete included.
//! - **A row is read back through the rules of `orch_core::team`**: a row that is not a preset is
//!   `Corrupt`, with a message that holds no owner and no word of it. A listing does not parse the
//!   members' array (it reads `json_array_length`), so one corrupt row of another person cannot break
//!   everybody's list.
//! - **Nothing of a preset is logged** and no error holds a word of one.

use orch_core::{
    CheckpointRules, HandoffApproval, MemberName, TeamMemberInput, TeamPreset, TeamPresetId,
    TeamPresetInput,
};
use orch_core::{Timestamp, UserId};
use orch_ports::{
    MAX_TEAM_PRESETS, StoreError, StoredTeamPreset, TEAM_PRESET_PAGE, TeamPresetPage,
    TeamPresetRevision, TeamPresetStore, TeamPresetStoreError, TeamPresetSummary,
};
use rusqlite::{OptionalExtension, params};

use crate::codec::{from_db, to_db, ts};
use crate::error::{store_err, unique_on};
use crate::store::write;

use crate::SqliteStore;

/// A full row, in the order of [`COLUMNS`].
type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<i64>,
    String,
    bool,
    i64,
    bool,
    i64,
    i64,
);

/// A listing row: no members' roles, no charter.
type SummaryRow = (String, String, String, String, i64, i64, i64);

const COLUMNS: &str = "id, name, title, lead, members, charter, max_handoffs, handoff_approval, \
     handoff_after_seconds, checkpoint_after, checkpoint_before_pull_request, push_tokens, summary, \
     revision, updated_at";

/// What one row's columns are, rendered once outside the blocking task.
struct Fields {
    id: String,
    name: String,
    title: String,
    lead: String,
    members: String,
    charter: String,
    max_handoffs: i64,
    mode: &'static str,
    after_seconds: Option<i64>,
    checkpoint_after: String,
    checkpoint_before_pull_request: bool,
    push_tokens: i64,
    summary: bool,
}

impl Fields {
    fn of(preset: &TeamPreset) -> Result<Self, TeamPresetStoreError> {
        let input = preset.to_input();
        let internal = |what: &str| TeamPresetStoreError::internal(what);
        let (mode, after_seconds) = match preset.handoff_approval() {
            HandoffApproval::Ask => ("ask", None),
            HandoffApproval::Auto => ("auto", None),
            HandoffApproval::After { seconds } => ("after", Some(i64::from(seconds))),
        };
        Ok(Self {
            id: preset.id().as_str().to_owned(),
            name: preset.name().to_owned(),
            title: preset.title().to_owned(),
            lead: preset.lead().as_str().to_owned(),
            members: serde_json::to_string(&input.members)
                .map_err(|_| internal("a member of the preset does not serialise"))?,
            charter: preset.charter().to_owned(),
            max_handoffs: i64::from(preset.max_handoffs()),
            mode,
            after_seconds,
            checkpoint_after: serde_json::to_string(
                &preset.checkpoints().after.iter().collect::<Vec<_>>(),
            )
            .map_err(|_| internal("a checkpoint rule does not serialise"))?,
            checkpoint_before_pull_request: preset.checkpoints().before_pull_request,
            push_tokens: i64::from(preset.push_tokens()),
            summary: preset.summary(),
        })
    }
}

impl TeamPresetStore for SqliteStore {
    async fn insert_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        at: Timestamp,
    ) -> Result<StoredTeamPreset, TeamPresetStoreError> {
        let written = ts(at);
        let (owner, f) = (owner.as_str().to_owned(), Fields::of(preset)?);
        let revision = self
            .call(move |conn| {
                write(conn, |tx| {
                    let exists = |sql: &str, args: &[&dyn rusqlite::ToSql]| {
                        tx.query_row(sql, args, |r| r.get::<_, bool>(0))
                            .map_err(store_err)
                    };
                    if exists(
                        "SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE id = ?1)",
                        &[&f.id],
                    )? {
                        return Ok(Err(TeamPresetStoreError::Exists));
                    }
                    if exists(
                        "SELECT EXISTS (SELECT 1 FROM user_team_presets WHERE owner = ?1 AND name = ?2)",
                        &[&owner, &f.name],
                    )? {
                        return Ok(Err(TeamPresetStoreError::NameTaken));
                    }
                    let held: i64 = tx
                        .query_row(
                            "SELECT count(*) FROM user_team_presets WHERE owner = ?1",
                            [&owner],
                            |r| r.get(0),
                        )
                        .map_err(store_err)?;
                    if !usize::try_from(held).is_ok_and(|held| held < MAX_TEAM_PRESETS) {
                        return Ok(Err(TeamPresetStoreError::TooMany {
                            limit: MAX_TEAM_PRESETS,
                        }));
                    }
                    let revision = next_revision(tx)?;
                    let us = to_db(written)?;
                    let inserted = tx.execute(
                        "INSERT INTO user_team_presets \
                           (id, owner, name, title, lead, members, charter, max_handoffs, \
                            handoff_approval, handoff_after_seconds, checkpoint_after, \
                            checkpoint_before_pull_request, push_tokens, summary, revision, \
                            created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                            ?15, ?16, ?16)",
                        params![
                            f.id,
                            owner,
                            f.name,
                            f.title,
                            f.lead,
                            f.members,
                            f.charter,
                            f.max_handoffs,
                            f.mode,
                            f.after_seconds,
                            f.checkpoint_after,
                            f.checkpoint_before_pull_request,
                            f.push_tokens,
                            f.summary,
                            revision,
                            us
                        ],
                    );
                    match inserted {
                        Ok(_) => Ok(Ok(revision)),
                        Err(e) if unique_on(&e, "user_team_presets.id") => {
                            Ok(Err(TeamPresetStoreError::Exists))
                        }
                        Err(e) if unique_on(&e, "user_team_presets.owner") => {
                            Ok(Err(TeamPresetStoreError::NameTaken))
                        }
                        Err(e) => Err(store_err(e)),
                    }
                })
            })
            .await
            .map_err(port_err)??;
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
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        let found: Option<Row> = self
            .call(move |conn| {
                let sql =
                    format!("SELECT {COLUMNS} FROM user_team_presets WHERE id = ?1 AND owner = ?2");
                conn.query_row(&sql, params![id, owner], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                })
                .optional()
                .map_err(store_err)
            })
            .await
            .map_err(port_err)?;
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
        let owner = owner.as_str().to_owned();
        let after = after.map(|a| a.as_str().to_owned());
        let (rows, held): (Vec<SummaryRow>, i64) = self
            .call(move |conn| {
                let mut statement = conn
                    .prepare(
                        "SELECT id, name, title, lead, json_array_length(members), revision, \
                           updated_at \
                         FROM user_team_presets \
                         WHERE owner = ?1 AND (?2 IS NULL OR id > ?2) \
                         ORDER BY id LIMIT ?3",
                    )
                    .map_err(store_err)?;
                let rows: Vec<SummaryRow> = statement
                    .query_map(params![owner, after, limit + 1], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    })
                    .map_err(store_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(store_err)?;
                let held: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM user_team_presets WHERE owner = ?1",
                        [&owner],
                        |r| r.get(0),
                    )
                    .map_err(store_err)?;
                Ok((rows, held))
            })
            .await
            .map_err(port_err)?;
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
        let written = ts(at);
        let (owner, f) = (owner.as_str().to_owned(), Fields::of(preset)?);
        let expected = stored_revision(expected);
        let revision = self
            .call(move |conn| {
                write(conn, |tx| {
                    let current: Option<i64> = tx
                        .query_row(
                            "SELECT revision FROM user_team_presets WHERE id = ?1 AND owner = ?2",
                            params![f.id, owner],
                            |r| r.get(0),
                        )
                        .optional()
                        .map_err(store_err)?;
                    match current {
                        None => return Ok(Err(TeamPresetStoreError::NotFound)),
                        Some(held) if held != expected => {
                            return Ok(Err(TeamPresetStoreError::Conflict));
                        }
                        Some(_) => {}
                    }
                    let revision = next_revision(tx)?;
                    let updated = tx.execute(
                        "UPDATE user_team_presets SET name = ?3, title = ?4, lead = ?5, \
                           members = ?6, charter = ?7, max_handoffs = ?8, handoff_approval = ?9, \
                           handoff_after_seconds = ?10, checkpoint_after = ?11, \
                           checkpoint_before_pull_request = ?12, push_tokens = ?13, summary = ?14, \
                           revision = ?15, updated_at = ?16 \
                         WHERE id = ?1 AND owner = ?2 AND revision = ?17",
                        params![
                            f.id,
                            owner,
                            f.name,
                            f.title,
                            f.lead,
                            f.members,
                            f.charter,
                            f.max_handoffs,
                            f.mode,
                            f.after_seconds,
                            f.checkpoint_after,
                            f.checkpoint_before_pull_request,
                            f.push_tokens,
                            f.summary,
                            revision,
                            to_db(written)?,
                            expected
                        ],
                    );
                    match updated {
                        Ok(_) => Ok(Ok(revision)),
                        // the rename is taken by another preset of the owner: nothing changed (the
                        // transaction is dropped, and the counter with it)
                        Err(e) if unique_on(&e, "user_team_presets.owner") => {
                            Ok(Err(TeamPresetStoreError::NameTaken))
                        }
                        Err(e) => Err(store_err(e)),
                    }
                })
            })
            .await
            .map_err(port_err)??;
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
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        let expected = expected.map(stored_revision);
        self.call(move |conn| {
            write(conn, |tx| {
                let current: Option<i64> = tx
                    .query_row(
                        "SELECT revision FROM user_team_presets WHERE id = ?1 AND owner = ?2",
                        params![id, owner],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(store_err)?;
                match current {
                    None => Ok(Err(TeamPresetStoreError::NotFound)),
                    Some(held) if expected.is_some_and(|e| e != held) => {
                        Ok(Err(TeamPresetStoreError::Conflict))
                    }
                    Some(_) => {
                        tx.execute(
                            "DELETE FROM user_team_presets WHERE id = ?1 AND owner = ?2",
                            params![id, owner],
                        )
                        .map_err(store_err)?;
                        Ok(Ok(()))
                    }
                }
            })
        })
        .await
        .map_err(port_err)?
    }
}

/// Draws the next revision from the counter, in the writing transaction.
fn next_revision(tx: &rusqlite::Transaction<'_>) -> Result<i64, StoreError> {
    tx.query_row(
        "UPDATE user_team_preset_counter SET n = n + 1 WHERE one = 1 RETURNING n",
        [],
        |r| r.get(0),
    )
    .map_err(store_err)
}

fn stored_revision(revision: TeamPresetRevision) -> i64 {
    // one that does not fit is not any revision the store gave: `-1`, which matches nothing
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<TeamPresetRevision, TeamPresetStoreError> {
    u64::try_from(revision)
        .map(TeamPresetRevision::new)
        .map_err(|_| {
            TeamPresetStoreError::unavailable("the revision counter gave a number out of range")
        })
}

fn corrupt(what: &str) -> TeamPresetStoreError {
    TeamPresetStoreError::Corrupt(format!("a stored team preset does not read back: {what}"))
}

/// A row, read back through the rules of `orch_core::team`: whatever is wrong is `Corrupt`, with a
/// sentence that holds no owner and no word of the preset (its id is what an operator looks for).
fn stored(row: Row) -> Result<StoredTeamPreset, TeamPresetStoreError> {
    let (
        id,
        name,
        title,
        lead,
        members,
        charter,
        max_handoffs,
        mode,
        after_seconds,
        checkpoint_after,
        checkpoint_before_pull_request,
        push_tokens,
        summary,
        revision,
        updated_at,
    ) = row;
    let id = TeamPresetId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let members: Vec<TeamMemberInput> =
        serde_json::from_str(&members).map_err(|_| corrupt("the members are not members"))?;
    let handoff_approval = match (mode.as_str(), after_seconds) {
        ("ask", None) => HandoffApproval::Ask,
        ("auto", None) => HandoffApproval::Auto,
        ("after", Some(seconds)) => HandoffApproval::After {
            seconds: u16::try_from(seconds).map_err(|_| corrupt("the wait of an approval"))?,
        },
        _ => return Err(corrupt("the approval mode and its seconds disagree")),
    };
    let after: Vec<String> =
        serde_json::from_str(&checkpoint_after).map_err(|_| corrupt("the checkpoint rules"))?;
    let input = TeamPresetInput {
        name,
        title,
        lead,
        members,
        charter,
        max_handoffs: u32::try_from(max_handoffs).map_err(|_| corrupt("the cap of hand-offs"))?,
        handoff_approval,
        checkpoints: CheckpointRules {
            after: after.into_iter().collect(),
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
    let at = from_db(updated_at).map_err(|_| corrupt("the time"))?;
    Ok(StoredTeamPreset::new(preset, revision, at))
}

fn summary(row: SummaryRow) -> Result<TeamPresetSummary, TeamPresetStoreError> {
    let (id, name, title, lead, members, revision, updated_at) = row;
    let id = TeamPresetId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let lead = MemberName::new(&lead).map_err(|e| corrupt(&e.to_string()))?;
    Ok(TeamPresetSummary {
        id,
        name,
        title,
        lead,
        member_count: u8::try_from(members).map_err(|_| corrupt("a count of members"))?,
        revision: u64::try_from(revision)
            .map(TeamPresetRevision::new)
            .map_err(|_| corrupt("the revision is not a positive number"))?,
        updated_at: from_db(updated_at).map_err(|_| corrupt("the time"))?,
    })
}

/// A store failure as the port says it. The driver's error is the source; it holds no parameter of
/// the statement, so no word of a preset and no owner.
fn port_err(err: StoreError) -> TeamPresetStoreError {
    match err {
        StoreError::Unavailable { source } => {
            TeamPresetStoreError::unavailable("the database is busy; try again").with_source(source)
        }
        StoreError::Corrupt { .. } => {
            TeamPresetStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        StoreError::Internal { source } => {
            TeamPresetStoreError::internal("the database refused the request").with_source(source)
        }
        _ => TeamPresetStoreError::internal("the database refused the request"),
    }
}
