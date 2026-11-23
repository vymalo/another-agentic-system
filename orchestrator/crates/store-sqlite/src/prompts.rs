//! `PromptStore` over SQLite: a person's prompt library (ADR 0070, migration `0003`).
//!
//! The table is `user_prompts`, **beside the log**: no event records a prompt and deleting a thread
//! erases nothing of it.
//!
//! - **One writer, one transaction.** Postgres needs an advisory lock of the owner so that the limit
//!   of 100 holds under racing inserts; here every write is a `BEGIN IMMEDIATE` transaction on the
//!   one connection, so the id, the name and the count are read and the row is inserted with nobody
//!   between them, and the refusals come in the order the port promises.
//! - **A revision is a number of one counter** (`user_prompt_counter`), drawn by the statement that
//!   writes it and never given twice, a delete included.
//! - **The audience is a JSON array**, matched with `json_each`; the roles asked for always include
//!   `"*"`, and only the ones that can be in an audience are sent.
//! - **A row is read back through [`PromptDefinition::from_stored`]**: a row that is not a prompt is
//!   `Corrupt`, with a message that holds no owner and no word of it. A listing does not parse
//!   `variables` (it reads `json_array_length`), so one corrupt row of another person cannot break
//!   everybody's list.
//! - **Nothing of a prompt is logged** and no error holds a word of one.

use orch_core::prompt::{PromptAudience, PromptDefinition, PromptId};
use orch_core::{Timestamp, UserId};
use orch_ports::{
    MAX_PROMPTS, PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError,
    PromptSummary, PromptViewer, StoreError, StoredPrompt,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, params, params_from_iter};

use crate::SqliteStore;
use crate::codec::{from_db, to_db, ts};
use crate::error::{store_err, unique_on};
use crate::store::write;

/// A full row, in the order of [`COLUMNS`].
type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
);

/// A listing row: no body, no variables.
type SummaryRow = (String, String, String, String, i64, String, bool, i64, i64);

const COLUMNS: &str =
    "id, name, title, description, body, variables, audience, revision, updated_at";

impl PromptStore for SqliteStore {
    async fn insert_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        at: Timestamp,
    ) -> Result<StoredPrompt, PromptStoreError> {
        let written = ts(at);
        let (owner, p) = (owner.as_str().to_owned(), Fields::of(prompt)?);
        let revision = self
            .call(move |conn| {
                write(conn, |tx| {
                    let exists = |sql: &str, args: &[&dyn rusqlite::ToSql]| {
                        tx.query_row(sql, args, |r| r.get::<_, bool>(0))
                            .map_err(store_err)
                    };
                    if exists(
                        "SELECT EXISTS (SELECT 1 FROM user_prompts WHERE id = ?1)",
                        &[&p.id],
                    )? {
                        return Ok(Err(PromptStoreError::Exists));
                    }
                    if exists(
                        "SELECT EXISTS (SELECT 1 FROM user_prompts WHERE owner = ?1 AND name = ?2)",
                        &[&owner, &p.name],
                    )? {
                        return Ok(Err(PromptStoreError::NameTaken));
                    }
                    let held: i64 = tx
                        .query_row(
                            "SELECT count(*) FROM user_prompts WHERE owner = ?1",
                            [&owner],
                            |r| r.get(0),
                        )
                        .map_err(store_err)?;
                    if !usize::try_from(held).is_ok_and(|held| held < MAX_PROMPTS) {
                        return Ok(Err(PromptStoreError::TooMany { limit: MAX_PROMPTS }));
                    }
                    let revision = next_revision(tx)?;
                    let us = to_db(written)?;
                    let inserted = tx.execute(
                        "INSERT INTO user_prompts \
                           (id, owner, name, title, description, body, variables, audience, \
                            revision, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                        params![
                            p.id,
                            owner,
                            p.name,
                            p.title,
                            p.description,
                            p.body,
                            p.variables,
                            p.audience,
                            revision,
                            us
                        ],
                    );
                    match inserted {
                        Ok(_) => Ok(Ok(revision)),
                        Err(e) if unique_on(&e, "user_prompts.id") => {
                            Ok(Err(PromptStoreError::Exists))
                        }
                        Err(e) if unique_on(&e, "user_prompts.owner") => {
                            Ok(Err(PromptStoreError::NameTaken))
                        }
                        Err(e) => Err(store_err(e)),
                    }
                })
            })
            .await
            .map_err(port_err)??;
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
        let user = viewer.user.as_str().to_owned();
        let roles = roles_plus_star(viewer);
        let id = id.as_str().to_owned();
        let found: Option<(String, Row)> = self
            .call(move |conn| {
                let mut args: Vec<Value> = vec![id.into(), user.into()];
                let shared = audience_overlap(&mut args, roles);
                let sql = format!(
                    "SELECT owner, {COLUMNS} FROM user_prompts \
                     WHERE id = ?1 AND (owner = ?2 OR ({shared}))"
                );
                conn.query_row(&sql, params_from_iter(args), |r| {
                    Ok((
                        r.get(0)?,
                        (
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                            r.get(8)?,
                            r.get(9)?,
                        ),
                    ))
                })
                .optional()
                .map_err(store_err)
            })
            .await
            .map_err(port_err)?;
        found
            .map(|(owner, row)| stored(row, owner == viewer.user.as_str()))
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
        let user = viewer.user.as_str().to_owned();
        let roles = roles_plus_star(viewer);
        let after = after.map(|a| a.as_str().to_owned());
        let (rows, held): (Vec<SummaryRow>, i64) = self
            .call(move |conn| {
                let mut args: Vec<Value> = vec![
                    user.clone().into(),
                    after.map_or(Value::Null, Value::from),
                    (limit + 1).into(),
                ];
                let shared = audience_overlap(&mut args, roles);
                let predicate = match scope {
                    PromptScope::Mine => "owner = ?1".to_owned(),
                    PromptScope::Shared => format!("owner <> ?1 AND ({shared})"),
                    PromptScope::All => format!("(owner = ?1 OR ({shared}))"),
                };
                // SQLite rejects an argument no statement mentions only when it is numbered past the
                // highest used; the roles are bound by number, so `Mine` drops them
                if matches!(scope, PromptScope::Mine) {
                    args.truncate(3);
                }
                let sql = format!(
                    "SELECT id, name, title, description, json_array_length(variables), audience, \
                       owner = ?1, revision, updated_at \
                     FROM user_prompts \
                     WHERE {predicate} AND (?2 IS NULL OR id > ?2) \
                     ORDER BY id LIMIT ?3"
                );
                let rows = crate::store::rows(conn, &sql, &params_refs(&args), |r| {
                    Ok((
                        r.get(0).map_err(store_err)?,
                        r.get(1).map_err(store_err)?,
                        r.get(2).map_err(store_err)?,
                        r.get(3).map_err(store_err)?,
                        r.get(4).map_err(store_err)?,
                        r.get(5).map_err(store_err)?,
                        r.get(6).map_err(store_err)?,
                        r.get(7).map_err(store_err)?,
                        r.get(8).map_err(store_err)?,
                    ))
                })?;
                let held: i64 = conn
                    .query_row(
                        "SELECT count(*) FROM user_prompts WHERE owner = ?1",
                        [&user],
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
        let written = ts(at);
        let (owner, p) = (owner.as_str().to_owned(), Fields::of(prompt)?);
        let expected = stored_revision(expected);
        let revision = self
            .call(move |conn| {
                write(conn, |tx| {
                    let current: Option<i64> = tx
                        .query_row(
                            "SELECT revision FROM user_prompts WHERE id = ?1 AND owner = ?2",
                            params![p.id, owner],
                            |r| r.get(0),
                        )
                        .optional()
                        .map_err(store_err)?;
                    match current {
                        None => return Ok(Err(PromptStoreError::NotFound)),
                        Some(held) if held != expected => {
                            return Ok(Err(PromptStoreError::Conflict));
                        }
                        Some(_) => {}
                    }
                    let revision = next_revision(tx)?;
                    let updated = tx.execute(
                        "UPDATE user_prompts SET name = ?3, title = ?4, description = ?5, \
                           body = ?6, variables = ?7, audience = ?8, revision = ?9, updated_at = ?10 \
                         WHERE id = ?1 AND owner = ?2 AND revision = ?11",
                        params![
                            p.id,
                            owner,
                            p.name,
                            p.title,
                            p.description,
                            p.body,
                            p.variables,
                            p.audience,
                            revision,
                            to_db(written)?,
                            expected
                        ],
                    );
                    match updated {
                        Ok(_) => Ok(Ok(revision)),
                        // the rename is taken by another prompt of the owner: nothing changed (the
                        // transaction is dropped, and the counter with it)
                        Err(e) if unique_on(&e, "user_prompts.owner") => {
                            Ok(Err(PromptStoreError::NameTaken))
                        }
                        Err(e) => Err(store_err(e)),
                    }
                })
            })
            .await
            .map_err(port_err)??;
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
        let (owner, id) = (owner.as_str().to_owned(), id.as_str().to_owned());
        let expected = expected.map(stored_revision);
        self.call(move |conn| {
            write(conn, |tx| {
                let current: Option<i64> = tx
                    .query_row(
                        "SELECT revision FROM user_prompts WHERE id = ?1 AND owner = ?2",
                        params![id, owner],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(store_err)?;
                match current {
                    None => Ok(Err(PromptStoreError::NotFound)),
                    Some(held) if expected.is_some_and(|e| e != held) => {
                        Ok(Err(PromptStoreError::Conflict))
                    }
                    Some(_) => {
                        tx.execute(
                            "DELETE FROM user_prompts WHERE id = ?1 AND owner = ?2",
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

/// The columns a write sends, rendered once outside the blocking task.
struct Fields {
    id: String,
    name: String,
    title: String,
    description: String,
    body: String,
    variables: String,
    audience: String,
}

impl Fields {
    fn of(p: &PromptDefinition) -> Result<Self, PromptStoreError> {
        Ok(Self {
            id: p.id().as_str().to_owned(),
            name: p.name().as_str().to_owned(),
            title: p.title().to_owned(),
            description: p.description().to_owned(),
            body: p.body().to_owned(),
            variables: serde_json::to_string(&p.variables().to_value())
                .map_err(|_| PromptStoreError::internal("a prompt's variables do not serialise"))?,
            audience: serde_json::to_string(&p.audience().values()).map_err(|_| {
                PromptStoreError::internal("a prompt's audience does not serialise")
            })?,
        })
    }
}

/// Draws the next revision from the counter, in the writing transaction.
fn next_revision(tx: &rusqlite::Transaction<'_>) -> Result<i64, StoreError> {
    tx.query_row(
        "UPDATE user_prompt_counter SET n = n + 1 WHERE one = 1 RETURNING n",
        [],
        |r| r.get(0),
    )
    .map_err(store_err)
}

/// The overlap of the audience with the viewer's roles, as a predicate over numbered parameters
/// appended to `args` (its numbers follow the ones already there).
fn audience_overlap(args: &mut Vec<Value>, roles: Vec<String>) -> String {
    let first = args.len() + 1;
    let marks: Vec<String> = (first..first + roles.len())
        .map(|n| format!("?{n}"))
        .collect();
    args.extend(roles.into_iter().map(Value::from));
    format!(
        "EXISTS (SELECT 1 FROM json_each(user_prompts.audience) WHERE value IN ({}))",
        marks.join(", ")
    )
}

fn params_refs(args: &[Value]) -> Vec<&dyn rusqlite::ToSql> {
    args.iter().map(|v| v as &dyn rusqlite::ToSql).collect()
}

/// The roles the viewer has, **with `"*"` added**, only the ones that can be in an audience.
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

fn stored_revision(revision: PromptRevision) -> i64 {
    // one that does not fit is not any revision the store gave: `-1`, which matches nothing
    i64::try_from(revision.get()).unwrap_or(-1)
}

fn revision_of(revision: i64) -> Result<PromptRevision, PromptStoreError> {
    u64::try_from(revision)
        .map(PromptRevision::new)
        .map_err(|_| {
            PromptStoreError::unavailable("the revision counter gave a number out of range")
        })
}

fn corrupt(what: &str) -> PromptStoreError {
    PromptStoreError::Corrupt(format!("a stored prompt does not read back: {what}"))
}

/// A row, read back through [`PromptDefinition::from_stored`]: whatever is wrong is `Corrupt`, with
/// a sentence that holds no owner and no word of the prompt (its id is what an operator looks for).
fn stored(row: Row, own: bool) -> Result<StoredPrompt, PromptStoreError> {
    let (id, name, title, description, body, variables, audience, revision, updated_at) = row;
    let id = PromptId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let variables: serde_json::Value =
        serde_json::from_str(&variables).map_err(|_| corrupt("the variables are not JSON"))?;
    let audience: Vec<String> =
        serde_json::from_str(&audience).map_err(|_| corrupt("the audience is not JSON"))?;
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
    let at = from_db(updated_at).map_err(|_| corrupt("the time"))?;
    Ok(StoredPrompt::new(prompt, revision, at, own))
}

fn summary(row: SummaryRow) -> Result<PromptSummary, PromptStoreError> {
    let (id, name, title, description, variables, audience, own, revision, updated_at) = row;
    let id = PromptId::parse(&id).map_err(|e| corrupt(&e.to_string()))?;
    let audience = if own {
        let values: Vec<String> =
            serde_json::from_str(&audience).map_err(|_| corrupt("the audience is not JSON"))?;
        Some(PromptAudience::from_values(&values).map_err(|e| corrupt(e.code))?)
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
        updated_at: from_db(updated_at).map_err(|_| corrupt("the time"))?,
    })
}

/// A store failure as the port says it. The driver's error is the source; it holds no parameter of
/// the statement, so no word of a prompt and no owner.
fn port_err(err: StoreError) -> PromptStoreError {
    match err {
        StoreError::Unavailable { source } => {
            PromptStoreError::unavailable("the database is busy; try again").with_source(source)
        }
        StoreError::Corrupt { .. } => {
            PromptStoreError::Corrupt("a row of the store does not decode".to_owned())
        }
        StoreError::Internal { source } => {
            PromptStoreError::internal("the database refused the request").with_source(source)
        }
        _ => PromptStoreError::internal("the database refused the request"),
    }
}
