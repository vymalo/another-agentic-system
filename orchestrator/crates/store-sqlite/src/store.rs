//! `ThreadStore` over SQLite.
//!
//! The statements are the Postgres store's (`orch-store-postgres/src/store.rs`) with SQLite's
//! types and without the clauses SQLite does not have. What those clauses carried is carried by the
//! transaction here: every write is one `BEGIN IMMEDIATE`, so no second writer is in the file while
//! one is, and `FOR UPDATE`, `FOR SHARE` and `SKIP LOCKED` have no work to do (ADR 0076 decision 6:
//! a thread has one home, and one process writes it).
//!
//! `RETURNING` is SQLite's own (3.35 and later; the bundled build is 3.54.1), which is what the
//! claims, the version check and the commit are written on. A CTE whose result a statement must not
//! re-read after it starts changing rows is `AS MATERIALIZED` (3.35 and later).
//!
//! The commit and the creation of a thread also write the usage ledger ([`crate::usage`], ADR 0065)
//! and the search index ([`crate::search`], ADR 0068) **in the same transaction**, so the log and
//! the two derived stores cannot disagree; [`delete`] clears the usage rows' thread and lets the
//! index's rows cascade.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    EditLink, Event, EventKind, ForkKind, ForkNode, Job, NONCE_LEN, RankError, ThreadId,
    ThreadRecord, UserId, WatchKey, between, spread, usage_counted_through,
};
use orch_ports::{
    AgentBinding, ArchivedFilter, Arrangement, BindingUpdate, ChildThread, ClaimOrder, Commit,
    CommitOutcome, ForkOrigin, InboxFinal, InboxId, InboxItem, InboxLease, Lease, ListOrder,
    MAX_CHILD_SESSION_CHARS, NewEvent, NewInbox, NewOutbox, NewThreadRecord, NewTimer, OutboxFinal,
    OutboxId, OutboxItem, OutboxPayload, OutboxStats, Parking, Place, Received, SharingChange,
    StoreError, TIMER_SOURCE, ThreadListing, ThreadStore,
};
use rusqlite::types::Value;
use rusqlite::{
    Connection, Row, ToSql, Transaction, TransactionBehavior, params, params_from_iter,
};

use crate::SqliteStore;
use crate::codec::{
    INBOX_COLS, OUTBOX_COLS, THREAD_COLS, binding_from_row, enum_str, event_from_row, from_db,
    inbox_from_row, job_json, outbox_from_row, parse_enum, thread_from_row, to_db,
};
use crate::error::{store_err, unique_on};
use crate::{search, usage};

// ------------------------------------------------------------------------------ the statements

/// One write, as one `BEGIN IMMEDIATE` transaction: `f` either writes all of it or, by returning an
/// error, none of it (the transaction is rolled back when it is dropped).
pub(crate) fn write<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_err)?;
    let outcome = f(&tx)?;
    tx.commit().map_err(store_err)?;
    Ok(outcome)
}

/// Every row `sql` gives, decoded by `decode`.
pub(crate) fn rows<T>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
    decode: impl Fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut stmt = conn.prepare(sql).map_err(store_err)?;
    let mut rows = stmt.query(params).map_err(store_err)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(store_err)? {
        out.push(decode(row)?);
    }
    Ok(out)
}

/// The first row `sql` gives, if any.
pub(crate) fn row_opt<T>(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
    decode: impl Fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    Ok(rows(conn, sql, params, decode)?.into_iter().next())
}

/// A count, or any single integer column.
pub(crate) fn scalar(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<i64, StoreError> {
    let mut stmt = conn.prepare(sql).map_err(store_err)?;
    let mut rows = stmt.query(params).map_err(store_err)?;
    match rows.next().map_err(store_err)? {
        Some(row) => row.get(0).map_err(store_err),
        None => Err(StoreError::internal(
            "a statement that must return a row returned none",
        )),
    }
}

pub(crate) fn exec(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<usize, StoreError> {
    conn.execute(sql, params).map_err(store_err)
}

/// A statement whose `IN` list is built from values.
pub(crate) fn dynamic_rows<T>(
    conn: &Connection,
    sql: &str,
    values: Vec<Value>,
    decode: impl Fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut stmt = conn.prepare(sql).map_err(store_err)?;
    let mut rows = stmt.query(params_from_iter(values)).map_err(store_err)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(store_err)? {
        out.push(decode(row)?);
    }
    Ok(out)
}

fn dynamic_row_opt<T>(
    conn: &Connection,
    sql: &str,
    values: Vec<Value>,
    decode: impl Fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    Ok(dynamic_rows(conn, sql, values, decode)?.into_iter().next())
}

pub(crate) fn dynamic_exec(
    conn: &Connection,
    sql: &str,
    values: Vec<Value>,
) -> Result<usize, StoreError> {
    conn.execute(sql, params_from_iter(values))
        .map_err(store_err)
}

/// `?from, ?from+1, ...` for `n` numbered placeholders, to build an `IN` list beside other
/// parameters. SQLite gives a bare `?` the index after the largest one used so far, so a statement
/// that names any parameter by number must number every one of them.
pub(crate) fn numbered(from: usize, n: usize) -> String {
    (from..from + n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `?, ?, ?` for `n` placeholders, to build an `IN` list in a statement that names none by number.
pub(crate) fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(", ")
}

/// `n` text values, for an `IN` list of ids.
pub(crate) fn texts(items: &[String]) -> Vec<Value> {
    items.iter().map(|t| Value::Text(t.clone())).collect()
}

/// The next `ord` of the outbox: one counter and one writer, so no gaps and no ties.
fn next_ord(tx: &Transaction<'_>) -> Result<i64, StoreError> {
    scalar(tx, "SELECT COALESCE(MAX(ord), 0) + 1 FROM outbox", &[])
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(t)
}

/// `outbox.attempts` and `inbox.attempts` are `INTEGER`; a lease past `i64::MAX` matches no row.
fn attempt(lease_attempt: u32) -> i64 {
    i64::from(lease_attempt)
}

pub(crate) fn parse_id(text: &str) -> Result<uuid::Uuid, StoreError> {
    uuid::Uuid::parse_str(text)
        .map_err(|e| StoreError::corrupt_with("a stored id is not a uuid", e))
}

pub(crate) fn archived_name(filter: ArchivedFilter) -> &'static str {
    match filter {
        ArchivedFilter::Exclude => "exclude",
        ArchivedFilter::Only => "only",
        ArchivedFilter::Include => "include",
    }
}

// ---------------------------------------------------------------------------------- the rail

/// The threads a listing may name: `?1` is the owner, `?2` whether the threads made by an edit are
/// listed.
const LISTED: &str = "t.owner = ?1 AND (?2 OR t.fork_kind IS NOT 'edit')";

/// The three sections in order (pinned, the rest, archived); the `sec` of a row.
const SECTION: &str = "CASE WHEN t.archived_at IS NOT NULL THEN 2 \
     WHEN t.pinned_at IS NOT NULL THEN 0 ELSE 1 END";

/// What follows the cursor in the order of the units, with `?4` the cursor's section, `?5` its rank,
/// `?6` its archived time and `?7` its id.
///
/// It is only ever used **with** a cursor. Bound to NULLs it admits nothing — every comparison with
/// NULL is NULL, never true — which is a page that starts at the top coming back empty. A listing
/// with no cursor takes [`NO_CURSOR`] instead, as `store-postgres` picks between its `after_cursor!`
/// and `no_cursor!`.
const AFTER_CURSOR: &str = "AND (u.sec > ?4 \
     OR (u.sec = ?4 AND ?4 < 2 AND (u.rail_rank > ?5 OR (u.rail_rank = ?5 AND u.id < ?7))) \
     OR (u.sec = ?4 AND ?4 = 2 AND (u.archived_at < ?6 OR (u.archived_at = ?6 AND u.id < ?7))))";

/// The clause of a page that starts at the top: the four cursor parameters are all NULL, and this
/// admits every unit (a statement naming `?4` to `?7` must be given them all).
const NO_CURSOR: &str = "AND (?4 IS NULL AND ?5 IS NULL AND ?6 IS NULL AND ?7 IS NULL)";

/// The top-level units a filter lists (ADR 0042).
const UNIT_EXCLUDE: &str = "t.rail_parent IS NULL AND t.archived_at IS NULL";
const UNIT_INCLUDE: &str = "t.rail_parent IS NULL";
const UNIT_ONLY: &str = "t.archived_at IS NOT NULL AND (t.rail_parent IS NULL OR NOT EXISTS (\
     SELECT 1 FROM threads p WHERE p.id = t.rail_parent AND p.archived_at IS NOT NULL))";

fn unit_filter(archived: ArchivedFilter) -> &'static str {
    match archived {
        ArchivedFilter::Exclude => UNIT_EXCLUDE,
        ArchivedFilter::Include => UNIT_INCLUDE,
        ArchivedFilter::Only => UNIT_ONLY,
    }
}

/// The top-level units of a [`ListOrder::Rail`] listing under a filter, after the cursor or from the
/// start, in the owner's order.
fn units_sql(archived: ArchivedFilter, after_cursor: bool) -> String {
    let cursor = if after_cursor {
        AFTER_CURSOR
    } else {
        NO_CURSOR
    };
    format!(
        "SELECT {THREAD_COLS} FROM (SELECT t.*, {SECTION} AS sec FROM threads t \
         WHERE {LISTED} AND {}) u WHERE u.owner = ?1 {cursor} \
         ORDER BY u.sec, CASE WHEN u.sec < 2 THEN u.rail_rank END, \
         u.archived_at DESC NULLS LAST, u.id DESC LIMIT ?3",
        unit_filter(archived)
    )
}

/// The cursor's own position, for a filter: it is none when the filter does not list the cursor as a
/// unit.
fn cursor_position_sql(archived: ArchivedFilter) -> String {
    format!(
        "SELECT {SECTION} AS sec, t.rail_rank, t.archived_at, t.id FROM threads t \
         WHERE {LISTED} AND {} AND t.id = ?3",
        unit_filter(archived)
    )
}

/// [`ThreadStore::list_threads`] with [`ListOrder::Recent`]: `id` descending.
fn list_recent(
    conn: &Connection,
    owner: &str,
    listing: ThreadListing,
) -> Result<Vec<ThreadRecord>, StoreError> {
    if let Some(cursor) = listing.before {
        let known = row_opt(
            conn,
            "SELECT 1 FROM threads WHERE id = ?1 AND owner = ?2",
            &[&cursor.0.to_string(), &owner],
            |_| Ok(()),
        )?;
        if known.is_none() {
            return Ok(Vec::new());
        }
    }
    rows(
        conn,
        &format!(
            "SELECT {THREAD_COLS} FROM threads \
             WHERE owner = ?1 AND (?2 IS NULL OR id < ?2) AND (?4 OR fork_kind IS NOT 'edit') \
               AND ((?5 = 'include') OR ((?5 = 'only') = (archived_at IS NOT NULL))) \
             ORDER BY id DESC LIMIT ?3"
        ),
        &[
            &owner,
            &listing.before.map(|b| b.0.to_string()).as_deref(),
            &i64::from(listing.limit),
            &listing.include_edits,
            &archived_name(listing.archived),
        ],
        thread_from_row,
    )
}

/// [`ThreadStore::list_threads`] with [`ListOrder::Rail`]: the page of top-level rows, then the
/// children of those.
fn list_rail(
    conn: &Connection,
    owner: &str,
    listing: ThreadListing,
) -> Result<Vec<ThreadRecord>, StoreError> {
    // Where the cursor stands in the order: its section, rank, archived time and id. A cursor the
    // filter does not list as a unit (unknown, foreign, nested) has no page after it.
    let cursor = match listing.before {
        None => None,
        Some(id) => {
            let found = row_opt(
                conn,
                &cursor_position_sql(listing.archived),
                &[&owner, &listing.include_edits, &id.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0).map_err(store_err)?,
                        row.get::<_, String>(1).map_err(store_err)?,
                        row.get::<_, Option<i64>>(2).map_err(store_err)?,
                        row.get::<_, String>(3).map_err(store_err)?,
                    ))
                },
            )?;
            let Some((sec, rank, at, id)) = found else {
                return Ok(Vec::new());
            };
            Some((sec, rank, at, id))
        }
    };
    let mut values = vec![
        Value::Text(owner.to_owned()),
        Value::Integer(i64::from(listing.include_edits)),
        Value::Integer(i64::from(listing.limit)),
    ];
    match &cursor {
        Some((sec, rank, at, id)) => {
            values.push(Value::Integer(*sec));
            values.push(Value::Text(rank.clone()));
            values.push(at.map_or(Value::Null, Value::Integer));
            values.push(Value::Text(id.clone()));
        }
        None => values.extend([Value::Null, Value::Null, Value::Null, Value::Null]),
    }
    let units: Vec<ThreadRecord> = dynamic_rows(
        conn,
        &units_sql(listing.archived, cursor.is_some()),
        values,
        thread_from_row,
    )?;
    if units.is_empty() {
        return Ok(units);
    }
    let ids: Vec<String> = units.iter().map(|u| u.id.0.to_string()).collect();
    let list = numbered(4, ids.len());
    let mut values = vec![
        Value::Text(owner.to_owned()),
        Value::Integer(i64::from(listing.include_edits)),
        Value::Integer(i64::from(listing.archived != ArchivedFilter::Exclude)),
    ];
    values.extend(texts(&ids));
    let children: Vec<ThreadRecord> = dynamic_rows(
        conn,
        &format!(
            "SELECT {THREAD_COLS} FROM threads \
             WHERE owner = ?1 AND rail_parent IN ({list}) \
               AND (?2 OR fork_kind IS NOT 'edit') AND (?3 OR archived_at IS NULL) \
             ORDER BY id DESC"
        ),
        values,
        thread_from_row,
    )?;
    let mut out = Vec::with_capacity(units.len() + children.len());
    for unit in units {
        let id = unit.id;
        out.push(unit);
        out.extend(
            children
                .iter()
                .filter(|c| c.rail_parent == Some(id))
                .cloned(),
        );
    }
    Ok(out)
}

/// The lowest rank of the owner's top-level rows, leaving `except` out.
fn lowest_rank(
    tx: &Transaction<'_>,
    owner: &str,
    except: Option<ThreadId>,
) -> Result<Option<String>, StoreError> {
    row_opt(
        tx,
        "SELECT min(rail_rank) FROM threads \
         WHERE owner = ?1 AND rail_parent IS NULL AND (?2 IS NULL OR id <> ?2)",
        &[&owner, &except.map(|e| e.0.to_string()).as_deref()],
        |row| row.get::<_, Option<String>>(0).map_err(store_err),
    )
    .map(Option::flatten)
}

/// The row right after (or before) the position `(rank, id)` in a section of the owner's list: the
/// top-level, listed, unarchived rows, pinned or not, by rank with ties newest first. `skip` is the
/// row being moved, nobody's neighbour.
fn neighbour(
    tx: &Transaction<'_>,
    owner: &str,
    pinned: bool,
    at: (&str, uuid::Uuid),
    skip: Option<ThreadId>,
    next: bool,
) -> Result<Option<(uuid::Uuid, String)>, StoreError> {
    let statement = if next {
        "SELECT id, rail_rank FROM threads \
         WHERE owner = ?1 AND rail_parent IS NULL AND archived_at IS NULL \
           AND fork_kind IS NOT 'edit' AND (pinned_at IS NOT NULL) = ?2 \
           AND (?5 IS NULL OR id <> ?5) \
           AND (rail_rank > ?3 OR (rail_rank = ?3 AND id < ?4)) \
         ORDER BY rail_rank ASC, id DESC LIMIT 1"
    } else {
        "SELECT id, rail_rank FROM threads \
         WHERE owner = ?1 AND rail_parent IS NULL AND archived_at IS NULL \
           AND fork_kind IS NOT 'edit' AND (pinned_at IS NOT NULL) = ?2 \
           AND (?5 IS NULL OR id <> ?5) \
           AND (rail_rank < ?3 OR (rail_rank = ?3 AND id > ?4)) \
         ORDER BY rail_rank DESC, id ASC LIMIT 1"
    };
    let id = at.1.to_string();
    let skip = skip.map(|s| s.0.to_string());
    let found = row_opt(
        tx,
        statement,
        &[&owner, &pinned, &at.0, &id, &skip.as_deref()],
        |row| {
            Ok((
                row.get::<_, String>(0).map_err(store_err)?,
                row.get::<_, String>(1).map_err(store_err)?,
            ))
        },
    )?;
    found
        .map(|(id, rank)| parse_id(&id).map(|id| (id, rank)))
        .transpose()
}

/// Writes the owner's top-level ranks again, evenly spread, in the order they have now.
fn respread(tx: &Transaction<'_>, owner: &str) -> Result<(), StoreError> {
    let ids: Vec<String> = rows(
        tx,
        "SELECT id FROM threads WHERE owner = ?1 AND rail_parent IS NULL \
         ORDER BY rail_rank, id DESC",
        &[&owner],
        |row| row.get(0).map_err(store_err),
    )?;
    for (id, rank) in ids.iter().zip(spread(ids.len())) {
        exec(
            tx,
            "UPDATE threads SET rail_rank = ?2 WHERE id = ?1",
            &[id, &rank],
        )?;
    }
    Ok(())
}

/// The key of the slot `place` names for `moving` among the owner's rows. Where no key fits (two
/// neighbours of one rank, or the cap), the owner's ranks are re-spread first, in the same
/// transaction.
fn new_rank(
    tx: &Transaction<'_>,
    owner: &str,
    moving: ThreadId,
    place: Place,
) -> Result<String, StoreError> {
    for round in 0..2 {
        let (lower, upper) = match place {
            Place::Top => (None, lowest_rank(tx, owner, Some(moving))?),
            Place::Before(anchor) | Place::After(anchor) => {
                let anchored = row_opt(
                    tx,
                    "SELECT rail_rank, pinned_at IS NOT NULL AS pinned FROM threads \
                     WHERE id = ?1 AND owner = ?2 AND rail_parent IS NULL \
                       AND archived_at IS NULL AND fork_kind IS NOT 'edit'",
                    &[&anchor.0.to_string(), &owner],
                    |row| {
                        Ok((
                            row.get::<_, String>(0).map_err(store_err)?,
                            row.get::<_, bool>(1).map_err(store_err)?,
                        ))
                    },
                )?
                .ok_or(StoreError::Refused("bad_anchor"))?;
                let (rank, pinned) = anchored;
                let at = (rank.as_str(), anchor.0);
                if matches!(place, Place::Before(_)) {
                    let before = neighbour(tx, owner, pinned, at, Some(moving), false)?;
                    (before.map(|(_, r)| r), Some(rank))
                } else {
                    let after = neighbour(tx, owner, pinned, at, Some(moving), true)?;
                    (Some(rank), after.map(|(_, r)| r))
                }
            }
        };
        match between(lower.as_deref(), upper.as_deref()) {
            Ok(key) => return Ok(key),
            Err(RankError::Order | RankError::TooLong) if round == 0 => respread(tx, owner)?,
            Err(e) => return Err(StoreError::corrupt(e.to_string())),
        }
    }
    Err(StoreError::corrupt("no rank fits after a re-spread"))
}

/// Whether `rec` is already where `place` puts it, in its own section.
fn already_there(
    tx: &Transaction<'_>,
    owner: &str,
    rec: &ThreadRecord,
    place: Place,
) -> Result<bool, StoreError> {
    let listed = rec.rail_parent.is_none()
        && rec.archived_at.is_none()
        && !rec.forked_from.is_some_and(|f| f.kind == ForkKind::Edit);
    if !listed {
        return Ok(false);
    }
    let pinned = rec.pinned_at.is_some();
    let at = (rec.rail_rank.as_str(), rec.id.0);
    Ok(match place {
        Place::Top => neighbour(tx, owner, pinned, at, None, false)?.is_none(),
        Place::Before(a) => {
            neighbour(tx, owner, pinned, at, None, true)?.is_some_and(|(id, _)| id == a.0)
        }
        Place::After(a) => {
            neighbour(tx, owner, pinned, at, None, false)?.is_some_and(|(id, _)| id == a.0)
        }
    })
}

/// Where an ejected row goes: right after the block it left, unless that block is pinned or
/// archived, which the row is not: then on top of the unpinned ones.
fn after_block(tx: &Transaction<'_>, owner: &str, rec: &ThreadRecord) -> Result<Place, StoreError> {
    let Some(parent) = rec.rail_parent else {
        return Ok(Place::Top);
    };
    let kept = row_opt(
        tx,
        "SELECT 1 FROM threads WHERE id = ?1 AND owner = ?2 AND pinned_at IS NULL \
         AND archived_at IS NULL AND fork_kind IS NOT 'edit'",
        &[&parent.0.to_string(), &owner],
        |_| Ok(()),
    )?;
    Ok(if kept.is_some() {
        Place::After(parent)
    } else {
        Place::Top
    })
}

/// [`ThreadStore::arrange_thread`] in a transaction.
fn arrange(
    tx: &Transaction<'_>,
    owner: &str,
    id: ThreadId,
    change: Arrangement,
    now: Timestamp,
) -> Result<ThreadRecord, StoreError> {
    let rec = row_opt(
        tx,
        &format!("SELECT {THREAD_COLS} FROM threads WHERE id = ?1 AND owner = ?2"),
        &[&id.0.to_string(), &owner],
        thread_from_row,
    )?
    .ok_or(StoreError::NotFound)?;
    let nested = rec.rail_parent.is_some();
    let unnest = change.unnest && nested;
    if nested && !unnest && (change.pinned == Some(true) || change.place.is_some()) {
        return Err(StoreError::Refused("nested_row"));
    }
    if let Some(Place::Before(a) | Place::After(a)) = change.place {
        let usable = a != id
            && row_opt(
                tx,
                "SELECT 1 FROM threads WHERE id = ?1 AND owner = ?2 AND rail_parent IS NULL \
                 AND archived_at IS NULL AND fork_kind IS NOT 'edit'",
                &[&a.0.to_string(), &owner],
                |_| Ok(()),
            )?
            .is_some();
        if !usable {
            return Err(StoreError::Refused("bad_anchor"));
        }
    }
    let want_pinned = change.pinned.unwrap_or(rec.pinned_at.is_some());
    let want_archived = change.archived.unwrap_or(rec.archived_at.is_some());
    let pin_changed = want_pinned != rec.pinned_at.is_some();
    let archive_changed = want_archived != rec.archived_at.is_some();

    let in_place = !pin_changed && !unnest;
    let slot = match change.place {
        Some(place) if in_place && already_there(tx, owner, &rec, place)? => None,
        Some(place) => Some(place),
        None if pin_changed => Some(Place::Top),
        None if unnest => Some(after_block(tx, owner, &rec)?),
        None => None,
    };
    if slot.is_none() && !archive_changed && !pin_changed && !unnest {
        return Ok(rec);
    }
    let rail_rank = match slot {
        Some(place) => new_rank(tx, owner, id, place)?,
        None => rec.rail_rank.clone(),
    };
    let pinned_at = if pin_changed {
        want_pinned.then_some(now)
    } else {
        rec.pinned_at
    };
    let archived_at = if archive_changed {
        want_archived.then_some(now)
    } else {
        rec.archived_at
    };
    let rail_parent = if unnest { None } else { rec.rail_parent };
    row_opt(
        tx,
        &format!(
            "UPDATE threads SET pinned_at = ?2, archived_at = ?3, rail_parent = ?4, \
             rail_rank = ?5 WHERE id = ?1 RETURNING {THREAD_COLS}"
        ),
        &[
            &id.0.to_string(),
            &pinned_at.map(to_db).transpose()?,
            &archived_at.map(to_db).transpose()?,
            &rail_parent.map(|p| p.0.to_string()),
            &rail_rank,
        ],
        thread_from_row,
    )?
    .ok_or(StoreError::NotFound)
}

// ------------------------------------------------------------------------------- delete a thread

/// A thread nested under one being deleted: its id, the rank it takes, the pin and archive of the
/// thread it was nested under, and its own archive.
type NestedChild = (String, String, Option<i64>, Option<i64>, Option<i64>);

/// [`ThreadStore::delete_threads`] in a transaction.
fn delete(
    tx: &Transaction<'_>,
    owner: &UserId,
    threads: &[(ThreadId, i64)],
    now: Timestamp,
) -> Result<(), StoreError> {
    let mut ids: Vec<uuid::Uuid> = threads.iter().map(|(id, _)| id.0).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let id_text: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let list = placeholders(id_text.len());
    // Locked in id order, whatever order the caller named them (here: no second writer, but the
    // order keeps the statement's reading of the versions deterministic).
    let mut values = texts(&id_text);
    values.push(Value::Text(owner.as_str().to_owned()));
    let locked: Vec<(String, i64)> = dynamic_rows(
        tx,
        &format!("SELECT id, version FROM threads WHERE id IN ({list}) AND owner = ? ORDER BY id"),
        values,
        |row| {
            Ok((
                row.get::<_, String>(0).map_err(store_err)?,
                row.get::<_, i64>(1).map_err(store_err)?,
            ))
        },
    )?;
    if locked.len() != ids.len() {
        return Err(StoreError::NotFound);
    }
    let moved = threads.iter().any(|(id, version)| {
        locked
            .iter()
            .find(|(locked_id, _)| *locked_id == id.0.to_string())
            .is_none_or(|(_, v)| v != version)
    });
    if moved {
        return Err(StoreError::VersionConflict);
    }
    // An edit of one that goes that the caller did not name: it would be left with the log it was
    // cut from gone and no way for the person to see it.
    let mut twice = texts(&id_text);
    twice.extend(texts(&id_text));
    if dynamic_row_opt(
        tx,
        &format!(
            "SELECT 1 FROM threads WHERE fork_kind = 'edit' AND forked_from IN ({list}) \
             AND id NOT IN ({list}) LIMIT 1"
        ),
        twice,
        |_| Ok(()),
    )?
    .is_some()
    {
        return Err(StoreError::VersionConflict);
    }
    // A child of one that goes that the caller did not name: a session opened since it looked.
    let mut twice = texts(&id_text);
    twice.extend(texts(&id_text));
    if dynamic_row_opt(
        tx,
        &format!("SELECT 1 FROM threads WHERE child_of IN ({list}) AND id NOT IN ({list}) LIMIT 1"),
        twice,
        |_| Ok(()),
    )?
    .is_some()
    {
        return Err(StoreError::VersionConflict);
    }
    // The threads nested under one that goes take its place in the owner's list: top-level, with its
    // rank (children are told apart by `id`, newest first, as the list reads them), its pin and its
    // archive. Before the delete, which would only null `rail_parent`.
    let mut twice = texts(&id_text);
    twice.extend(texts(&id_text));
    let children: Vec<NestedChild> = dynamic_rows(
        tx,
        &format!(
            "SELECT c.id, p.rail_rank, p.pinned_at, p.archived_at, c.archived_at \
             FROM threads c JOIN threads p ON c.rail_parent = p.id \
             WHERE p.id IN ({list}) AND c.id NOT IN ({list})"
        ),
        twice,
        |row| {
            Ok((
                row.get::<_, String>(0).map_err(store_err)?,
                row.get::<_, String>(1).map_err(store_err)?,
                row.get::<_, Option<i64>>(2).map_err(store_err)?,
                row.get::<_, Option<i64>>(3).map_err(store_err)?,
                row.get::<_, Option<i64>>(4).map_err(store_err)?,
            ))
        },
    )?;
    for (child, rank, pinned, parent_archived, own_archived) in children {
        exec(
            tx,
            "UPDATE threads SET rail_parent = NULL, rail_rank = ?2, pinned_at = ?3, \
             archived_at = COALESCE(?4, ?5) WHERE id = ?1",
            &[&child, &rank, &pinned, &own_archived, &parent_archived],
        )?;
    }
    // The timers of the inbox name the thread in their payload and have no foreign key.
    dynamic_exec(
        tx,
        &format!(
            "DELETE FROM inbox WHERE source = ? AND json_extract(payload, '$.thread') IN ({list})"
        ),
        {
            let mut values = vec![Value::Text(TIMER_SOURCE.to_owned())];
            values.extend(texts(&id_text));
            values
        },
    )?;
    // The promise that the files will go, in the transaction that deletes the log.
    for id in &id_text {
        exec(
            tx,
            "INSERT INTO thread_purges (thread_id, deleted_at) VALUES (?1, ?2) \
             ON CONFLICT (thread_id) DO NOTHING",
            &[id, &to_db(now)?],
        )?;
    }
    // The usage of the threads stays, naming no thread any more (ADR 0065): in the transaction that
    // deletes the log, so the ledger never points at a thread that is gone. The index needs no
    // statement: `message_terms` cascades from `events` and `thread_terms` from `threads`.
    usage::forget_threads(tx, &id_text)?;
    // Cascades the events, outbox, binding and watches; nulls `forked_from` and `rail_parent` of the
    // forks; the child reference is deferred, so a parent and its named children go in one
    // statement.
    dynamic_exec(
        tx,
        &format!("DELETE FROM threads WHERE id IN ({list})"),
        texts(&id_text),
    )?;
    Ok(())
}

// ------------------------------------------------------------------------------- the writer

/// [`ThreadStore::create_thread`] and [`ThreadStore::fork_thread`].
fn create(
    conn: &mut Connection,
    new: NewThreadRecord,
    fork: Option<ForkOrigin>,
    first: Commit,
) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
    // only `open_child_thread` makes a child
    if new.child_of.is_some() {
        return Err(StoreError::Refused("child_of"));
    }
    match insert_thread(conn, new, fork, first, false)? {
        ChildThread::Created { thread, events } => Ok((thread, events)),
        ChildThread::Existing(_) => Err(StoreError::corrupt("a thread that is no child exists")),
    }
}

/// The one transaction behind `create`, `fork_thread` and `open_child_thread`.
fn insert_thread(
    conn: &mut Connection,
    new: NewThreadRecord,
    fork: Option<ForkOrigin>,
    first: Commit,
    child: bool,
) -> Result<ChildThread, StoreError> {
    write(conn, |tx| {
        let cut = fork.map_or(0, |o| o.cut);
        let owner = new.owner.as_str().to_owned();
        let thread = new.id.0.to_string();
        if child && let Some(of) = &new.child_of {
            let parent = row_opt(
                tx,
                "SELECT child_of IS NOT NULL AS is_child FROM threads WHERE id = ?1 AND owner = ?2",
                &[&of.thread.0.to_string(), &owner],
                |row| row.get::<_, bool>(0).map_err(store_err),
            )?;
            let Some(is_child) = parent else {
                return Err(StoreError::NotFound);
            };
            if is_child {
                return Err(StoreError::Refused("nested_child"));
            }
            let found = row_opt(
                tx,
                &format!(
                    "SELECT {THREAD_COLS} FROM threads WHERE child_of = ?1 AND child_session = ?2"
                ),
                &[&of.thread.0.to_string(), &of.session],
                thread_from_row,
            )?;
            if let Some(record) = found {
                return Ok(ChildThread::Existing(record));
            }
        }
        if let Some(origin) = fork {
            let parent = row_opt(
                tx,
                "SELECT last_seq FROM threads WHERE id = ?1 AND owner = ?2",
                &[&origin.parent.0.to_string(), &owner],
                |row| row.get::<_, i64>(0).map_err(store_err),
            )?;
            let Some(last_seq) = parent else {
                return Err(StoreError::NotFound);
            };
            if origin.cut < 0 || last_seq < origin.cut {
                return Err(StoreError::corrupt("the cut is beyond the parent's log"));
            }
        }
        if let Some(parent) = new.rail_parent {
            let top_level = row_opt(
                tx,
                "SELECT 1 FROM threads WHERE id = ?1 AND owner = ?2 AND rail_parent IS NULL",
                &[&parent.0.to_string(), &owner],
                |_| Ok(()),
            )?;
            if top_level.is_none() {
                return Err(StoreError::corrupt(
                    "a thread is nested under a top-level thread of its owner's",
                ));
            }
        }
        let hidden = fork.is_some_and(|o| o.kind == ForkKind::Edit);
        let rail_rank = if new.rail_parent.is_some() || hidden {
            // a row the list does not rank: it takes the rank of the first, and burns no key
            lowest_rank(tx, &owner, None)?.unwrap_or_else(|| "i".to_owned())
        } else {
            new_rank(tx, &owner, new.id, Place::Top)?
        };
        let description = new.description.as_deref().filter(|d| !d.is_empty());
        let job = job_json(first.job.as_ref().unwrap_or(&Job::default()))?;
        let state = enum_str(&first.new_state)?;
        let now = to_db(new.now)?;
        let inserted = tx.execute(
            "INSERT INTO threads (id, owner, title, description, agent_id, release, state, job, \
             version, last_seq, created_at, updated_at, forked_from, forked_at, fork_kind, \
             rail_parent, rail_rank, child_of, child_session, search_fold) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?10, ?11, ?12, ?13, ?14, ?15, \
             ?16, ?17, ?18)",
            params![
                thread,
                owner,
                new.title,
                description,
                new.target.agent_id.as_str(),
                new.target.release.as_deref(),
                state,
                job,
                cut,
                now,
                fork.map(|o| o.parent.0.to_string()),
                fork.map(|o| o.cut),
                fork.map(|o| o.kind.as_str()),
                new.rail_parent.map(|p| p.0.to_string()),
                rail_rank,
                new.child_of.as_ref().map(|of| of.thread.0.to_string()),
                new.child_of.as_ref().map(|of| of.session.as_str()),
                orch_core::search::FOLD_VERSION,
            ],
        );
        if let Err(e) = inserted {
            // another caller opened this session between our look and our insert: it won, and the
            // thread it made is the answer
            if unique_on(&e, "threads.child_of")
                && let Some(of) = &new.child_of
            {
                return row_opt(
                    tx,
                    &format!(
                        "SELECT {THREAD_COLS} FROM threads \
                         WHERE child_of = ?1 AND child_session = ?2"
                    ),
                    &[&of.thread.0.to_string(), &of.session],
                    thread_from_row,
                )?
                .map(ChildThread::Existing)
                // gone again already (the winner and its parent were deleted): as if the parent
                // were missing
                .ok_or(StoreError::NotFound);
            }
            return Err(if unique_on(&e, "") {
                StoreError::corrupt("thread id already exists")
            } else {
                store_err(e)
            });
        }
        exec(
            tx,
            "INSERT INTO a2a_bindings (thread_id, agent_id, context_id, updated_at) \
             VALUES (?1, ?2, ?3, ?4)",
            &[
                &thread,
                &new.target.agent_id.as_str(),
                &new.context_id.as_deref(),
                &now,
            ],
        )?;
        if let Some(update) = &first.binding {
            update_binding(tx, new.id, update, first.now)?;
        }
        let has_outbox = !first.outbox.is_empty();
        if let Some(origin) = fork {
            // The parent's events as they are: the same seq, time, actor and data. Events up to the
            // cut never change.
            let copied = exec(
                tx,
                "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
                 SELECT ?1, seq, at, kind, actor, data FROM events \
                 WHERE thread_id = ?2 AND seq <= ?3",
                &[&thread, &origin.parent.0.to_string(), &origin.cut],
            )?;
            if i64::try_from(copied).ok() != Some(origin.cut) {
                return Err(StoreError::corrupt(
                    "the parent's log has a gap before the cut",
                ));
            }
        }
        let events = insert_events(tx, new.id, cut + 1, first.events)?;
        // The usage ledger, in the transaction of the events (ADR 0065); what a fork copied was
        // counted in its parent, so only the events of its own are folded in.
        usage::fold_events(tx, &new.owner, &events)?;
        // The search index, in the same transaction (ADR 0068): what a fork copied is indexed from
        // the fork's own log (the parent's rows may no longer hold a message it replaced), then the
        // events of its own.
        let search_reached = if cut > 0 {
            search::catch_up_before_commit(tx, new.id, &new.owner, 0, cut)?
        } else {
            0
        };
        search::index_events(tx, new.id, &new.owner, &events)?;
        search::write_thread_keys(tx, new.id, "title", &search::keys_of(Some(&new.title)))?;
        search::write_thread_keys(tx, new.id, "description", &search::keys_of(description))?;
        insert_outbox(tx, new.id, &owner, first.outbox, first.now)?;
        let rearmed =
            insert_watches_and_timers(tx, new.id, &first.watches, first.timers, first.now)?;
        let last_seq = cut + i64::try_from(events.len()).unwrap_or(i64::MAX - cut);
        // The ledger is through the whole log: what a fork copied was counted in its parent, and
        // `usage_counted_through` says the same with the cut.
        let search_through = if search_reached >= cut {
            last_seq
        } else {
            search_reached
        };
        let record = row_opt(
            tx,
            &format!(
                "UPDATE threads SET last_seq = ?2, usage_through = ?2, search_through = ?3, \
                 updated_at = ?4 WHERE id = ?1 RETURNING {THREAD_COLS}"
            ),
            &[&thread, &last_seq, &search_through, &to_db(first.now)?],
            thread_from_row,
        )?
        .ok_or(StoreError::NotFound)?;
        if has_outbox || rearmed {
            // The wakeup of this store is in process (ADR 0076 decision 3) and is a later
            // milestone's port. Nothing is lost meanwhile: the dispatcher and the inbox worker read
            // what is due when they look.
            tracing::trace!(thread = %new.id, "outbox or inbox rows changed");
        }
        Ok(ChildThread::Created {
            thread: record,
            events,
        })
    })
}

/// Inserts `events` as `first_seq..`, returning them as stored.
fn insert_events(
    tx: &Transaction<'_>,
    thread: ThreadId,
    first_seq: i64,
    events: Vec<NewEvent>,
) -> Result<Vec<Event>, StoreError> {
    let mut stored = Vec::with_capacity(events.len());
    for (offset, new) in (0_i64..).zip(events) {
        let seq = first_seq + offset;
        let actor = serde_json::to_string(&new.actor)
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        let data = serde_json::to_string(&new.body.data_value())
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        tx.execute(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data, idempotency_key) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                thread.0.to_string(),
                seq,
                to_db(new.at)?,
                new.body.kind().as_str(),
                actor,
                data,
                new.idempotency_key.as_deref(),
            ],
        )
        .map_err(|e| {
            if unique_on(&e, "events.idempotency_key") {
                StoreError::corrupt("the same idempotency key twice in one commit")
            } else {
                store_err(e)
            }
        })?;
        stored.push(Event {
            seq,
            thread_id: thread,
            at: new.at,
            actor: new.actor,
            body: new.body,
        });
    }
    Ok(stored)
}

fn insert_outbox(
    tx: &Transaction<'_>,
    thread: ThreadId,
    owner: &str,
    rows: Vec<NewOutbox>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let now = to_db(now)?;
    for row in rows {
        let payload = serde_json::to_string(&row.payload)
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        let ord = next_ord(tx)?;
        exec(
            tx,
            "INSERT INTO outbox (id, ord, thread_id, owner, kind, payload, status, \
             next_attempt_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7, ?7)",
            &[
                &row.id.0.to_string(),
                &ord,
                &thread.0.to_string(),
                &owner,
                &enum_str(&row.payload.kind())?,
                &payload,
                &now,
            ],
        )?;
    }
    Ok(())
}

fn update_binding(
    tx: &Transaction<'_>,
    thread: ThreadId,
    update: &BindingUpdate,
    now: Timestamp,
) -> Result<(), StoreError> {
    let task_state = update.task_state.as_ref().map(enum_str).transpose()?;
    exec(
        tx,
        // The context is adopted once: the first one the agent assigned stands (ADR 0055).
        "UPDATE a2a_bindings SET task_id = COALESCE(?2, task_id), \
         task_state = COALESCE(?3, task_state), revision = COALESCE(?4, revision), \
         context_id = COALESCE(context_id, ?6), updated_at = ?5 WHERE thread_id = ?1",
        &[
            &thread.0.to_string(),
            &update.task_id.as_deref(),
            &task_state.as_deref(),
            &update.revision.as_deref(),
            &to_db(now)?,
            &update.context_id.as_deref().filter(|c| !c.is_empty()),
        ],
    )
    .map(|_| ())
}

/// Marks the inbox row `applied` (the caller holds it: its claim was checked above).
fn finish_inbox_row(
    tx: &Transaction<'_>,
    lease: &InboxLease,
    now: Timestamp,
) -> Result<(), StoreError> {
    exec(
        tx,
        "UPDATE inbox SET status = 'applied', lease_owner = NULL, lease_until = NULL, \
         updated_at = ?2 WHERE id = ?1",
        &[&lease.id.0.to_string(), &to_db(now)?],
    )
    .map(|_| ())
}

fn outbox_outcome(outcome: OutboxFinal) -> (&'static str, Option<String>) {
    match outcome {
        OutboxFinal::Delivered => ("delivered", None),
        OutboxFinal::Dead { error } => ("dead", Some(error)),
        OutboxFinal::Skipped => ("skipped", None),
    }
}

/// The statement that ends a claimed outbox row.
const OUTBOX_FINISH: &str = "UPDATE outbox SET status = ?4, last_error = COALESCE(?5, last_error), \
     lease_owner = NULL, lease_until = NULL, updated_at = ?6 \
     WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'";

fn finish_outbox_row(
    tx: &Transaction<'_>,
    lease: &Lease,
    outcome: OutboxFinal,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (status, error) = outbox_outcome(outcome);
    exec(
        tx,
        OUTBOX_FINISH,
        &[
            &lease.id.0.to_string(),
            &lease.owner,
            &attempt(lease.attempt),
            &status,
            &error.as_deref(),
            &to_db(now)?,
        ],
    )
    .map(|_| ())
}

/// Finishes the thread's unsent messages (`delegate` and `steer` rows) as `skipped`: those
/// `pending`, and those `inflight` whose lease expired before `now`. A round to follow and a
/// person's start or close of a session are no messages and stay.
fn skip_unsent_rows(
    tx: &Transaction<'_>,
    thread: ThreadId,
    now: Timestamp,
) -> Result<u32, StoreError> {
    let changed = exec(
        tx,
        "UPDATE outbox SET status = 'skipped', lease_owner = NULL, lease_until = NULL, \
         updated_at = ?2 \
         WHERE thread_id = ?1 AND kind IN ('delegate', 'steer') AND sent_at IS NULL \
           AND json_extract(payload, '$.observe') IS NULL \
           AND json_extract(payload, '$.session_start') IS NULL \
           AND json_extract(payload, '$.session_close') IS NULL \
           AND (status = 'pending' OR (status = 'inflight' AND lease_until < ?2))",
        &[&thread.0.to_string(), &to_db(now)?],
    )?;
    Ok(u32::try_from(changed).unwrap_or(u32::MAX))
}

/// Inserts the commit's watches and, in the same transaction, re-arms the parked inbox rows they
/// match; then inserts its timers. Returns whether any inbox row became claimable now.
///
/// Postgres serialised "add a watch" against "park for it" with an advisory lock per key, because
/// two transactions could each read the other's snapshot and leave a row parked behind a watch.
/// Here both are statements of one writer: nothing else is in the file between them.
fn insert_watches_and_timers(
    tx: &Transaction<'_>,
    thread: ThreadId,
    watches: &[WatchKey],
    timers: Vec<NewTimer>,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let mut rearmed = false;
    if !watches.is_empty() {
        let mut keys: Vec<&str> = watches.iter().map(WatchKey::as_str).collect();
        keys.sort_unstable();
        keys.dedup();
        for key in &keys {
            let inserted = exec(
                tx,
                "INSERT INTO watches (key, thread_id, created_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (key) DO NOTHING",
                &[key, &thread.0.to_string(), &to_db(now)?],
            )?;
            if inserted == 0 {
                warn_if_watched_by_another(tx, key, thread)?;
            }
        }
        let list = placeholders(keys.len());
        // the statement's placeholders, in order: `available_at`, `updated_at`, then the keys
        let now = to_db(now)?;
        let mut values: Vec<Value> = vec![Value::Integer(now), Value::Integer(now)];
        values.extend(keys.iter().map(|k| Value::Text((*k).to_owned())));
        let changed = dynamic_exec(
            tx,
            &format!(
                "UPDATE inbox SET status = 'pending', available_at = ?, parked_at = NULL, \
                 refunded = attempts, updated_at = ? \
                 WHERE status = 'parked' AND correlation IN ({list})"
            ),
            values,
        )?;
        rearmed = changed > 0;
    }
    for timer in timers {
        let payload = serde_json::to_string(&timer.payload(thread))
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        exec(
            tx,
            "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, \
             available_at, created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'timer', ?4, 'pending', ?5, ?6, ?6) \
             ON CONFLICT (source, idempotency_key) DO NOTHING",
            &[
                &timer.id.0.to_string(),
                &TIMER_SOURCE,
                &timer.idempotency_key(thread),
                &payload,
                &to_db(timer.due(now))?,
                &to_db(now)?,
            ],
        )?;
    }
    Ok(rearmed)
}

/// A key that is watched already stays with its first thread. When the thread that asked for it now
/// is another one, the reports for the key will never reach it: say so.
fn warn_if_watched_by_another(
    tx: &Transaction<'_>,
    key: &str,
    thread: ThreadId,
) -> Result<(), StoreError> {
    let owner = row_opt(
        tx,
        "SELECT thread_id FROM watches WHERE key = ?1",
        &[&key],
        |row| row.get::<_, String>(0).map_err(store_err),
    )?;
    if let Some(owner) = owner.filter(|owner| owner != &thread.0.to_string()) {
        tracing::warn!(
            watch = key,
            watched_by = owner,
            asked_by = %thread,
            "a watch key is owned by another thread; this thread will not hear its reports"
        );
    }
    Ok(())
}

// ------------------------------------------------------------------------------- the commit

/// [`ThreadStore::commit`] in a transaction.
fn commit_in(
    tx: &Transaction<'_>,
    thread: ThreadId,
    expected_version: i64,
    commit: Commit,
) -> Result<CommitOutcome, StoreError> {
    let thread_text = thread.0.to_string();
    // The one writer holds the file: the seq counter below cannot be raced, so seq has no gaps and
    // no duplicates.
    let locked = row_opt(
        tx,
        "SELECT version, last_seq, state, owner, usage_through, forked_at, search_through, \
         search_fold, title, description FROM threads WHERE id = ?1",
        &[&thread_text],
        |row| {
            Ok((
                row.get::<_, i64>(0).map_err(store_err)?,
                row.get::<_, i64>(1).map_err(store_err)?,
                row.get::<_, String>(2).map_err(store_err)?,
                row.get::<_, String>(3).map_err(store_err)?,
                row.get::<_, i64>(4).map_err(store_err)?,
                row.get::<_, Option<i64>>(5).map_err(store_err)?,
                row.get::<_, i64>(6).map_err(store_err)?,
                row.get::<_, i32>(7).map_err(store_err)?,
                row.get::<_, String>(8).map_err(store_err)?,
                row.get::<_, Option<String>>(9).map_err(store_err)?,
            ))
        },
    )?;
    let Some((
        version,
        last_seq,
        state,
        owner,
        usage_through,
        forked_at,
        search_through,
        search_fold,
        title,
        description,
    )) = locked
    else {
        return Err(StoreError::NotFound);
    };
    if let Some(lease) = &commit.lease {
        let held = row_opt(
            tx,
            "SELECT 1 FROM outbox WHERE id = ?1 AND thread_id = ?2 AND lease_owner = ?3 \
             AND attempts = ?4 AND status = 'inflight'",
            &[
                &lease.id.0.to_string(),
                &thread_text,
                &lease.owner,
                &attempt(lease.attempt),
            ],
            |_| Ok(()),
        )?;
        if held.is_none() {
            return Ok(CommitOutcome::Fenced);
        }
    }
    if let Some(lease) = &commit.inbox {
        let held = row_opt(
            tx,
            "SELECT 1 FROM inbox WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 \
             AND status = 'inflight'",
            &[
                &lease.id.0.to_string(),
                &lease.owner,
                &attempt(lease.attempt),
            ],
            |_| Ok(()),
        )?;
        if held.is_none() {
            return Ok(CommitOutcome::Fenced);
        }
    }
    if version != expected_version {
        return Err(StoreError::VersionConflict);
    }
    if commit.only_finishes_inbox() && enum_str(&commit.new_state)? == state {
        // Nothing to write to the thread. The claim and the version were checked above (a "nothing
        // to do" is only as good as the thread it was decided on); the row is finished and the
        // thread is left alone: no version bump, no wakeup.
        if let Some(lease) = &commit.inbox {
            finish_inbox_row(tx, lease, commit.now)?;
        }
        let record = row_opt(
            tx,
            &format!("SELECT {THREAD_COLS} FROM threads WHERE id = ?1"),
            &[&thread_text],
            thread_from_row,
        )?
        .ok_or(StoreError::NotFound)?;
        return Ok(CommitOutcome::Applied {
            thread: record,
            events: Vec::new(),
        });
    }
    let keys: Vec<String> = commit
        .events
        .iter()
        .filter_map(|e| e.idempotency_key.clone())
        .collect();
    if !keys.is_empty() {
        let list = placeholders(keys.len());
        let mut values = vec![Value::Text(thread_text.clone())];
        values.extend(texts(&keys));
        let seen = dynamic_row_opt(
            tx,
            &format!(
                "SELECT 1 FROM events WHERE thread_id = ? AND idempotency_key IN ({list}) LIMIT 1"
            ),
            values,
            |_| Ok(()),
        )?;
        if seen.is_some() {
            return Ok(CommitOutcome::Duplicate);
        }
    }
    let has_outbox = !commit.outbox.is_empty() || commit.finishes_outbox.is_some();
    // The usage ledger is written in this transaction (ADR 0065). First what it has not been
    // through: events a build without the ledger appended, or the log a thread had before migration
    // `0002`; none in steady state, when `usage_through` is the log.
    let owner_id = UserId::new(&owner);
    let behind = usage_counted_through(usage_through, forked_at);
    if behind < last_seq {
        usage::catch_up(tx, thread, &owner_id, behind, last_seq)?;
    }
    // The search index (ADR 0068), the same way, but **bounded**: some of what it has not been
    // through first (at most `COMMIT_CATCH_UP_EVENTS` events, the watermark advancing only as far as
    // it got), with the title and the description of the row, then this commit's events, always. A
    // thread stored under another fold version starts again from nothing.
    let mut search_through = search_through;
    if search_fold != orch_core::search::FOLD_VERSION {
        search::delete_index(tx, thread)?;
        search_through = 0;
    }
    let search_behind = search_through < last_seq || search_fold != orch_core::search::FOLD_VERSION;
    let mut title_keys = None;
    let mut description_keys = None;
    let mut search_reached = last_seq;
    if search_behind {
        search_reached =
            search::catch_up_before_commit(tx, thread, &owner_id, search_through, last_seq)?;
        title_keys = Some(search::keys_of(Some(&title)));
        description_keys = Some(search::keys_of(description.as_deref()));
    }
    let events = insert_events(tx, thread, last_seq + 1, commit.events)?;
    usage::fold_events(tx, &owner_id, &events)?;
    search::index_events(tx, thread, &owner_id, &events)?;
    // a title or description this commit writes is the one that is searched
    if let Some(title) = &commit.title {
        title_keys = Some(search::keys_of(Some(title)));
    }
    if let Some(description) = &commit.description {
        description_keys = Some(search::keys_of(Some(description)));
    }
    let new_last_seq = last_seq + i64::try_from(events.len()).unwrap_or(0);
    // The index is through the whole log only if the old part was finished; else exactly where it
    // stopped, so a later commit or the backfill continues from there.
    let search_through_after = if search_reached >= last_seq {
        new_last_seq
    } else {
        search_reached
    };
    // The job is written with the state, under the same version check; a commit without one leaves
    // the stored job alone.
    let job = commit.job.as_ref().map(job_json).transpose()?;
    let (sharing_op, sharing_level, sharing_nonce) = match commit.sharing {
        Some(SharingChange::Set { level, nonce }) => (
            Some("set"),
            Some(enum_str(&level)?),
            Some(nonce.as_bytes().to_vec()),
        ),
        Some(SharingChange::Clear) => (Some("clear"), None, None),
        None => (None, None, None),
    };
    let now = to_db(commit.now)?;
    let new_state = enum_str(&commit.new_state)?;
    let sql = format!(
        "UPDATE threads SET state = ?2, job = COALESCE(?3, job), \
         title = COALESCE(?4, title), \
         description = CASE WHEN ?5 IS NULL THEN description ELSE NULLIF(?5, '') END, \
         visibility = CASE ?6 WHEN 'set' THEN ?7 WHEN 'clear' THEN 'private' \
             ELSE visibility END, \
         share_nonce = CASE ?6 WHEN 'set' THEN ?8 WHEN 'clear' THEN NULL \
             ELSE share_nonce END, \
         shared_at = CASE ?6 WHEN 'set' THEN ?9 WHEN 'clear' THEN NULL ELSE shared_at END, \
         version = version + 1, last_seq = ?10, usage_through = ?10, search_through = ?11, \
         search_fold = ?12, updated_at = ?9 \
         WHERE id = ?1 RETURNING {THREAD_COLS}"
    );
    let mut stmt = tx.prepare(&sql).map_err(store_err)?;
    let values: &[&dyn ToSql] = &[
        &thread_text,
        &new_state,
        &job,
        &commit.title.as_deref(),
        &commit.description.as_deref(),
        &sharing_op,
        &sharing_level,
        &sharing_nonce,
        &now,
        &new_last_seq,
        &search_through_after,
        &orch_core::search::FOLD_VERSION,
    ];
    let mut cursor = stmt.query(values).map_err(|e| {
        // The unique index of the link's nonce: a capability is one thread's.
        if unique_on(&e, "threads.share_nonce") {
            StoreError::corrupt("share nonce already in use")
        } else {
            store_err(e)
        }
    })?;
    let record = match cursor.next().map_err(store_err)? {
        Some(row) => thread_from_row(row)?,
        None => return Err(StoreError::NotFound),
    };
    drop(cursor);
    if let Some(keys) = &title_keys {
        search::write_thread_keys(tx, thread, "title", keys)?;
    }
    if let Some(keys) = &description_keys {
        search::write_thread_keys(tx, thread, "description", keys)?;
    }
    // The abandoned job's unsent delegations are finished before this commit's own rows exist, in
    // the same transaction (ADR 0036).
    if commit.skip_unsent_delegates {
        skip_unsent_rows(tx, thread, commit.now)?;
    }
    insert_outbox(tx, thread, &owner, commit.outbox, commit.now)?;
    if let Some(update) = &commit.binding {
        update_binding(tx, thread, update, commit.now)?;
    }
    insert_watches_and_timers(tx, thread, &commit.watches, commit.timers, commit.now)?;
    if let Some(lease) = &commit.inbox {
        finish_inbox_row(tx, lease, commit.now)?;
    }
    if let (Some(lease), Some(outcome)) = (&commit.lease, commit.finishes_outbox.clone()) {
        finish_outbox_row(tx, lease, outcome, commit.now)?;
    }
    if has_outbox {
        tracing::trace!(thread = %thread, "outbox rows changed");
    }
    Ok(CommitOutcome::Applied {
        thread: record,
        events,
    })
}

// ------------------------------------------------------------------------------- the claims

/// The per-thread rule: a delegate row is held back while an older delegate of the same thread is
/// `pending` or `inflight`, a steer row likewise waits for an older steer and for no delegate
/// (ADR 0036); cancel, verify, ask, title and description rows are unrestricted (ADR 0026).
const OUTBOX_THREAD_RULE: &str = "(o.kind IN ('cancel', 'verify', 'ask', 'title', 'description') \
     OR NOT EXISTS (SELECT 1 FROM outbox p \
       WHERE p.thread_id = o.thread_id AND p.kind = o.kind AND p.ord < o.ord \
         AND p.status IN ('pending', 'inflight')))";

/// A row of the outbox is due at `?1`: `pending` and past its next attempt, or `inflight` under a
/// lease that has expired.
const OUTBOX_DUE: &str = "((o.status = 'pending' AND o.next_attempt_at <= ?1) \
     OR (o.status = 'inflight' AND o.lease_until <= ?1))";

/// What claiming does to the rows a statement named in `c`: the outbox columns in the order
/// [`outbox_from_row`] reads them, and `ord` last (the caller sorts by it).
macro_rules! outbox_claimed {
    () => {
        "UPDATE outbox SET status = 'inflight', lease_owner = ?2, lease_until = ?3, \
           attempts = attempts + 1, updated_at = ?1 \
         WHERE id IN (SELECT id FROM c) RETURNING id, thread_id, kind, payload, status, attempts, \
           next_attempt_at, lease_owner, lease_until, sent_at, task_id, last_error, created_at, ord"
    };
}

/// [`ClaimOrder::Fifo`]: the oldest due rows first, under the per-thread rule.
fn claim_fifo() -> String {
    format!(
        "WITH c AS MATERIALIZED ( \
           SELECT o.id FROM outbox o WHERE {OUTBOX_DUE} AND {OUTBOX_THREAD_RULE} \
           ORDER BY o.ord LIMIT ?4) {}",
        outbox_claimed!()
    )
}

/// [`ClaimOrder::Fair`] (ADR 0066): per person, their first `4 * limit` candidates with
/// `load = running + rank`, and the cancel rows with load 0, which take no place in anybody's rank;
/// then the `4 * limit` rows with the lowest `(load, ord)`; then `limit` of them, oldest first
/// among equal loads. A row whose rank passes `limit` cannot be among the `limit` first, so the
/// rest of a queue is never ranked.
fn claim_fair() -> String {
    format!(
        "WITH running AS MATERIALIZED ( \
           SELECT owner, count(*) AS n FROM outbox \
           WHERE status = 'inflight' AND lease_until > ?1 GROUP BY owner), \
         cand AS MATERIALIZED ( \
           SELECT o.id, o.ord, o.owner, \
             row_number() OVER (PARTITION BY o.owner ORDER BY o.ord) AS rank \
           FROM outbox o \
           WHERE o.status IN ('pending', 'inflight') AND o.kind <> 'cancel' \
             AND {OUTBOX_DUE} AND {OUTBOX_THREAD_RULE}), \
         ranked AS MATERIALIZED ( \
           SELECT c.id, c.ord, COALESCE(r.n, 0) + c.rank AS load \
           FROM cand c LEFT JOIN running r ON r.owner = c.owner \
           WHERE c.rank <= ?4 * 4 \
           UNION ALL \
           SELECT o.id, o.ord, 0 FROM outbox o \
           WHERE o.kind = 'cancel' AND o.status IN ('pending', 'inflight') AND {OUTBOX_DUE}), \
         picked AS MATERIALIZED ( \
           SELECT id, ord, load FROM ranked ORDER BY load, ord LIMIT ?4 * 4), \
         c AS MATERIALIZED ( \
           SELECT o.id FROM outbox o JOIN picked k ON k.id = o.id \
           WHERE {OUTBOX_DUE} ORDER BY k.load, k.ord LIMIT ?4) {}",
        outbox_claimed!()
    )
}

// ------------------------------------------------------------------------------- the fork family

/// The family of edits of `?1` (a thread), the owner's `?2`: up the edit links to the thread the
/// family started from, then down them. `message` is the first message of a person after the
/// `thread_forked` event of a thread made by an edit.
const FORK_FAMILY: &str = "\
    WITH RECURSIVE up AS ( \
        SELECT id, forked_from, fork_kind, 0 AS depth FROM threads WHERE id = ?1 AND owner = ?2 \
        UNION ALL \
        SELECT p.id, p.forked_from, p.fork_kind, up.depth + 1 \
        FROM threads p JOIN up ON p.id = up.forked_from AND up.fork_kind = 'edit' \
        WHERE p.owner = ?2 AND up.depth < 1000), \
    root AS (SELECT id FROM up ORDER BY depth DESC LIMIT 1), \
    down AS ( \
        SELECT id, 0 AS depth FROM root \
        UNION ALL \
        SELECT c.id, down.depth + 1 \
        FROM threads c JOIN down ON c.forked_from = down.id \
        WHERE c.fork_kind = 'edit' AND c.owner = ?2 AND down.depth < 1000) \
    SELECT t.id, t.forked_from, t.forked_at, t.fork_kind, t.created_at, \
        (SELECT min(e.seq) FROM events e \
         WHERE e.thread_id = t.id AND e.kind = 'user_message' AND e.seq > t.forked_at) AS message \
    FROM threads t JOIN down ON down.id = t.id \
    ORDER BY t.created_at, t.id LIMIT 1000";

fn fork_node_from_row(row: &Row<'_>) -> Result<ForkNode, StoreError> {
    let id = ThreadId(parse_id(&row.get::<_, String>(0).map_err(store_err)?)?);
    let parent: Option<String> = row.get(1).map_err(store_err)?;
    let at: Option<i64> = row.get(2).map_err(store_err)?;
    let kind: Option<String> = row.get(3).map_err(store_err)?;
    let created = from_db(row.get::<_, i64>(4).map_err(store_err)?)?;
    let message: Option<i64> = row.get(5).map_err(store_err)?;
    let kind: Option<ForkKind> = kind.map(|k| parse_enum("fork kind", &k)).transpose()?;
    // A link is an edit whose parent still exists; any other thread starts a family of its own.
    let link = match (kind, parent, at) {
        (Some(ForkKind::Edit), Some(parent), Some(cut)) => Some(EditLink {
            parent: ThreadId(parse_id(&parent)?),
            cut,
            message: message.unwrap_or(cut + 2),
        }),
        _ => None,
    };
    Ok(ForkNode { id, link, created })
}

// ------------------------------------------------------------------------------- the trait

impl ThreadStore for SqliteStore {
    async fn ping(&self) -> Result<(), StoreError> {
        self.call(|conn| scalar(conn, "SELECT 1", &[]).map(|_| ()))
            .await
    }

    async fn create_thread(
        &self,
        new: NewThreadRecord,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        self.call(move |conn| create(conn, new, None, first)).await
    }

    async fn fork_thread(
        &self,
        new: NewThreadRecord,
        origin: ForkOrigin,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        self.call(move |conn| create(conn, new, Some(origin), first))
            .await
    }

    async fn open_child_thread(
        &self,
        new: NewThreadRecord,
        first: Commit,
    ) -> Result<ChildThread, StoreError> {
        let Some(of) = new.child_of.as_ref().filter(|of| of.thread != new.id) else {
            return Err(StoreError::Refused("child_of"));
        };
        if !(1..=MAX_CHILD_SESSION_CHARS).contains(&of.session.chars().count()) {
            return Err(StoreError::Refused("child_session"));
        }
        self.call(move |conn| insert_thread(conn, new, None, first, true))
            .await
    }

    async fn child_threads(
        &self,
        owner: &UserId,
        parent: ThreadId,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        let owner = owner.as_str().to_owned();
        let parent = parent.0.to_string();
        self.call(move |conn| {
            rows(
                conn,
                &format!(
                    "SELECT {THREAD_COLS} FROM threads WHERE owner = ?1 AND child_of = ?2 \
                     ORDER BY created_at, id LIMIT 1000"
                ),
                &[&owner, &parent],
                thread_from_row,
            )
        })
        .await
    }

    async fn fork_family(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> Result<Vec<ForkNode>, StoreError> {
        let owner = owner.as_str().to_owned();
        let id = thread.0.to_string();
        self.call(move |conn| rows(conn, FORK_FAMILY, &[&id, &owner], fork_node_from_row))
            .await
    }

    async fn get_thread(
        &self,
        owner: Option<&UserId>,
        id: ThreadId,
    ) -> Result<Option<ThreadRecord>, StoreError> {
        let owner = owner.map(|o| o.as_str().to_owned());
        let id = id.0.to_string();
        self.call(move |conn| {
            row_opt(
                conn,
                &format!(
                    "SELECT {THREAD_COLS} FROM threads WHERE id = ?1 AND (?2 IS NULL OR owner = ?2)"
                ),
                &[&id, &owner.as_deref()],
                thread_from_row,
            )
        })
        .await
    }

    async fn list_threads(
        &self,
        owner: &UserId,
        listing: ThreadListing,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        let owner = owner.as_str().to_owned();
        self.call(move |conn| match listing.order {
            ListOrder::Recent => list_recent(conn, &owner, listing),
            ListOrder::Rail => list_rail(conn, &owner, listing),
        })
        .await
    }

    async fn arrange_thread(
        &self,
        owner: &UserId,
        id: ThreadId,
        change: Arrangement,
        now: Timestamp,
    ) -> Result<ThreadRecord, StoreError> {
        let owner = owner.as_str().to_owned();
        self.call(move |conn| write(conn, |tx| arrange(tx, &owner, id, change, now)))
            .await
    }

    async fn delete_threads(
        &self,
        owner: &UserId,
        threads: &[(ThreadId, i64)],
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let owner = owner.clone();
        let threads = threads.to_vec();
        self.call(move |conn| write(conn, |tx| delete(tx, &owner, &threads, now)))
            .await
    }

    async fn claim_purges(
        &self,
        owner: &str,
        limit: u32,
        lease: Duration,
        now: Timestamp,
    ) -> Result<Vec<ThreadId>, StoreError> {
        let owner = owner.to_owned();
        self.call(move |conn| {
            let until = to_db(plus(now, lease))?;
            let now = to_db(now)?;
            let mut claimed: Vec<(i64, String)> = rows(
                conn,
                "WITH c AS MATERIALIZED ( \
                   SELECT thread_id FROM thread_purges \
                   WHERE lease_until IS NULL OR lease_until <= ?1 \
                   ORDER BY deleted_at, thread_id LIMIT ?4) \
                 UPDATE thread_purges SET lease_owner = ?2, lease_until = ?3, \
                   attempts = attempts + 1 \
                 WHERE thread_id IN (SELECT thread_id FROM c) \
                 RETURNING deleted_at, thread_id",
                &[&now, &owner, &until, &i64::from(limit)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0).map_err(store_err)?,
                        row.get::<_, String>(1).map_err(store_err)?,
                    ))
                },
            )?;
            claimed.sort();
            claimed
                .into_iter()
                .map(|(_, id)| parse_id(&id).map(ThreadId))
                .collect()
        })
        .await
    }

    async fn finish_purge(&self, thread: ThreadId) -> Result<(), StoreError> {
        let id = thread.0.to_string();
        self.call(move |conn| {
            exec(
                conn,
                "DELETE FROM thread_purges WHERE thread_id = ?1",
                &[&id],
            )
            .map(|_| ())
        })
        .await
    }

    async fn purges_pending(&self) -> Result<u64, StoreError> {
        self.call(|conn| {
            scalar(conn, "SELECT count(*) FROM thread_purges", &[])
                .map(|n| u64::try_from(n).unwrap_or(0))
        })
        .await
    }

    async fn commit(
        &self,
        thread: ThreadId,
        expected_version: i64,
        commit: Commit,
    ) -> Result<CommitOutcome, StoreError> {
        self.call(move |conn| write(conn, |tx| commit_in(tx, thread, expected_version, commit)))
            .await
    }

    async fn list_events(
        &self,
        thread: ThreadId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        self.call(move |conn| {
            rows(
                conn,
                "SELECT seq, at, kind, actor, data FROM events \
                 WHERE thread_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
                &[&thread.0.to_string(), &after, &i64::from(limit)],
                |row| event_from_row(thread, row),
            )
        })
        .await
    }

    async fn latest_events(
        &self,
        thread: ThreadId,
        kind: EventKind,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        let kind = kind.as_str();
        self.call(move |conn| {
            rows(
                conn,
                "SELECT seq, at, kind, actor, data FROM events \
                 WHERE thread_id = ?1 AND kind = ?2 ORDER BY seq DESC LIMIT ?3",
                &[&thread.0.to_string(), &kind, &i64::from(limit)],
                |row| event_from_row(thread, row),
            )
        })
        .await
    }

    async fn ui_catalog_event(
        &self,
        thread: ThreadId,
        digest: &str,
    ) -> Result<Option<Event>, StoreError> {
        let digest = digest.to_owned();
        self.call(move |conn| {
            row_opt(
                conn,
                "SELECT seq, at, kind, actor, data FROM events \
                 WHERE thread_id = ?1 AND kind = 'ui_catalog' \
                   AND json_extract(data, '$.digest') = ?2 \
                 ORDER BY seq DESC LIMIT 1",
                &[&thread.0.to_string(), &digest],
                |row| event_from_row(thread, row),
            )
        })
        .await
    }

    async fn thread_pick(&self, thread: ThreadId) -> Result<orch_core::ModelPick, StoreError> {
        let text: Option<Option<String>> = self
            .call(move |conn| {
                row_opt(
                    conn,
                    "SELECT pick FROM threads WHERE id = ?1",
                    &[&thread.0.to_string()],
                    |row| row.get(0).map_err(store_err),
                )
            })
            .await?;
        text.flatten()
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|e| StoreError::corrupt_with("cannot read the thread's pick", e))
            .map(Option::unwrap_or_default)
    }

    async fn set_thread_pick(
        &self,
        owner: &UserId,
        thread: ThreadId,
        pick: orch_core::ModelPick,
    ) -> Result<(), StoreError> {
        let owner = owner.as_str().to_owned();
        let text = (!pick.is_empty())
            .then(|| serde_json::to_string(&pick))
            .transpose()
            .map_err(|e| StoreError::corrupt_with("cannot serialise the thread's pick", e))?;
        self.call(move |conn| {
            write(conn, |tx| {
                let touched = exec(
                    tx,
                    "UPDATE threads SET pick = ?3 WHERE id = ?1 AND owner = ?2",
                    &[&thread.0.to_string(), &owner, &text],
                )?;
                if touched == 0 {
                    return Err(StoreError::NotFound);
                }
                Ok(())
            })
        })
        .await
    }

    async fn get_binding(&self, thread: ThreadId) -> Result<Option<AgentBinding>, StoreError> {
        self.call(move |conn| {
            row_opt(
                conn,
                "SELECT thread_id, agent_id, context_id, task_id, task_state, revision \
                 FROM a2a_bindings WHERE thread_id = ?1",
                &[&thread.0.to_string()],
                binding_from_row,
            )
        })
        .await
    }

    async fn thread_by_share_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Option<ThreadRecord>, StoreError> {
        let nonce = nonce.to_vec();
        self.call(move |conn| {
            row_opt(
                conn,
                &format!("SELECT {THREAD_COLS} FROM threads WHERE share_nonce = ?1"),
                &[&nonce],
                thread_from_row,
            )
        })
        .await
    }

    async fn claim_outbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
        order: ClaimOrder,
    ) -> Result<Vec<OutboxItem>, StoreError> {
        let owner = owner.to_owned();
        self.call(move |conn| {
            let until = to_db(plus(now, lease))?;
            let now = to_db(now)?;
            let sql = match order {
                ClaimOrder::Fair => claim_fair(),
                ClaimOrder::Fifo => claim_fifo(),
            };
            let mut claimed: Vec<(i64, OutboxItem)> = rows(
                conn,
                &sql,
                &[&now, &owner, &until, &i64::from(limit)],
                |row| {
                    let item = outbox_from_row(row)?;
                    let ord: i64 = row.get(13).map_err(store_err)?;
                    Ok((ord, item))
                },
            )?;
            claimed.sort_by_key(|(ord, _)| *ord);
            Ok(claimed.into_iter().map(|(_, item)| item).collect())
        })
        .await
    }

    async fn renew_lease(&self, lease: &Lease, until: Timestamp) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE outbox SET lease_until = ?4 \
                 WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'",
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &to_db(until)?,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn mark_sent(
        &self,
        lease: &Lease,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            write(conn, |tx| {
                let sent = row_opt(
                    tx,
                    "UPDATE outbox SET sent_at = ?4, updated_at = ?4 \
                     WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 \
                       AND status = 'inflight' RETURNING thread_id",
                    &[
                        &lease.id.0.to_string(),
                        &lease.owner,
                        &attempt(lease.attempt),
                        &to_db(now)?,
                    ],
                    |row| row.get::<_, String>(0).map_err(store_err),
                )?;
                let Some(thread) = sent else {
                    return Ok(false);
                };
                let thread = ThreadId(parse_id(&thread)?);
                update_binding(tx, thread, &binding, now)?;
                Ok(true)
            })
        })
        .await
    }

    async fn mark_verify_sent(
        &self,
        lease: &Lease,
        task_id: String,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE outbox SET sent_at = ?4, task_id = ?5, updated_at = ?4 \
                 WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'",
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &to_db(now)?,
                    &task_id,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn retry_outbox(
        &self,
        lease: &Lease,
        next_attempt_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE outbox SET status = 'pending', next_attempt_at = ?4, \
                 lease_owner = NULL, lease_until = NULL, last_error = ?5, \
                 updated_at = CAST(unixepoch('subsec') * 1000000 AS INTEGER) \
                 WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'",
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &to_db(next_attempt_at)?,
                    &error,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn complete_outbox(
        &self,
        lease: &Lease,
        outcome: OutboxFinal,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            let (status, error) = outbox_outcome(outcome);
            exec(
                conn,
                OUTBOX_FINISH,
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &status,
                    &error.as_deref(),
                    &to_db(now)?,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn requeue_as_delegate(&self, lease: &Lease, now: Timestamp) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            write(conn, |tx| {
                let held = row_opt(
                    tx,
                    "SELECT payload FROM outbox \
                     WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 \
                       AND status = 'inflight' AND kind = 'steer'",
                    &[
                        &lease.id.0.to_string(),
                        &lease.owner,
                        &attempt(lease.attempt),
                    ],
                    |row| row.get::<_, String>(0).map_err(store_err),
                )?;
                let Some(held) = held else {
                    return Ok(false);
                };
                let payload: OutboxPayload = serde_json::from_str(&held)
                    .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
                let Some(delegate) = payload.steer_as_delegate() else {
                    return Ok(false);
                };
                let delegate = serde_json::to_string(&delegate)
                    .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
                // `ord` and `created_at` stay: the delegation keeps the steer's place in the
                // thread's order
                exec(
                    tx,
                    "UPDATE outbox SET kind = 'delegate', payload = ?2, status = 'pending', \
                     next_attempt_at = ?3, lease_owner = NULL, lease_until = NULL, updated_at = ?3 \
                     WHERE id = ?1",
                    &[&lease.id.0.to_string(), &delegate, &to_db(now)?],
                )?;
                Ok(true)
            })
        })
        .await
    }

    async fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        self.call(move |conn| write(conn, |tx| skip_unsent_rows(tx, thread, now)))
            .await
    }

    async fn release_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let owner = owner.to_owned();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE outbox SET lease_until = ?2, updated_at = ?2 \
                 WHERE status = 'inflight' AND lease_owner = ?1",
                &[&owner, &to_db(now)?],
            )
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn outbox_stats(&self, now: Timestamp) -> Result<OutboxStats, StoreError> {
        self.call(move |conn| {
            let now = to_db(now)?;
            let stats = row_opt(
                conn,
                "SELECT \
                   count(*) FILTER (WHERE (status = 'pending' AND next_attempt_at <= ?1) \
                                       OR (status = 'inflight' AND lease_until <= ?1)) AS due, \
                   count(*) FILTER (WHERE status = 'pending' AND next_attempt_at > ?1) AS waiting, \
                   count(*) FILTER (WHERE status = 'inflight' AND lease_until > ?1) AS leased, \
                   min(CASE WHEN status = 'pending' AND next_attempt_at <= ?1 THEN next_attempt_at \
                            WHEN status = 'inflight' AND lease_until <= ?1 THEN lease_until END) \
                     AS oldest_due_at \
                 FROM outbox WHERE status IN ('pending', 'inflight')",
                &[&now],
                |row| {
                    let count = |at: usize| -> Result<u64, StoreError> {
                        let n: i64 = row.get(at).map_err(store_err)?;
                        u64::try_from(n)
                            .map_err(|e| StoreError::corrupt_with("negative outbox count", e))
                    };
                    let oldest: Option<i64> = row.get(3).map_err(store_err)?;
                    Ok(OutboxStats {
                        due: count(0)?,
                        waiting: count(1)?,
                        leased: count(2)?,
                        oldest_due_at: oldest.map(from_db).transpose()?,
                    })
                },
            )?;
            stats.ok_or_else(|| {
                StoreError::internal("the outbox statistics statement returned nothing")
            })
        })
        .await
    }

    async fn get_outbox(&self, id: OutboxId) -> Result<Option<OutboxItem>, StoreError> {
        let id = id.0.to_string();
        self.call(move |conn| {
            row_opt(
                conn,
                &format!("SELECT {OUTBOX_COLS} FROM outbox WHERE id = ?1"),
                &[&id],
                outbox_from_row,
            )
        })
        .await
    }

    async fn list_open_outbox(&self, thread: ThreadId) -> Result<Vec<OutboxItem>, StoreError> {
        self.call(move |conn| {
            rows(
                conn,
                &format!(
                    "SELECT {OUTBOX_COLS} FROM outbox WHERE thread_id = ?1 \
                     AND status IN ('pending', 'inflight') ORDER BY ord"
                ),
                &[&thread.0.to_string()],
                outbox_from_row,
            )
        })
        .await
    }

    async fn receive(&self, row: NewInbox, now: Timestamp) -> Result<Received, StoreError> {
        self.call(move |conn| {
            let payload = serde_json::to_string(&row.payload)
                .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
            let now = to_db(now)?;
            let stored = exec(
                conn,
                "INSERT INTO inbox (id, source, idempotency_key, kind, payload, correlation, \
                 status, available_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?7, ?7) \
                 ON CONFLICT (source, idempotency_key) DO NOTHING",
                &[
                    &row.id.0.to_string(),
                    &row.source,
                    &row.idempotency_key,
                    &row.payload.kind(),
                    &payload,
                    &row.correlation.as_deref(),
                    &now,
                ],
            )?;
            Ok(if stored == 1 {
                Received::Stored { id: row.id }
            } else {
                Received::Duplicate
            })
        })
        .await
    }

    async fn claim_inbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<InboxItem>, StoreError> {
        let owner = owner.to_owned();
        self.call(move |conn| {
            let until = to_db(plus(now, lease))?;
            let now = to_db(now)?;
            let mut items = rows(
                conn,
                "WITH c AS MATERIALIZED ( \
                   SELECT id FROM inbox \
                   WHERE (status = 'pending' AND available_at <= ?1) \
                      OR (status = 'inflight' AND lease_until <= ?1) \
                   ORDER BY available_at, id LIMIT ?4) \
                 UPDATE inbox SET status = 'inflight', lease_owner = ?2, lease_until = ?3, \
                   attempts = attempts + 1, updated_at = ?1 \
                 WHERE id IN (SELECT id FROM c) RETURNING id, source, idempotency_key, kind, \
                   payload, correlation, status, available_at, attempts, refunded, lease_owner, \
                   lease_until, parked_at, last_error, created_at",
                &[&now, &owner, &until, &i64::from(limit)],
                inbox_from_row,
            )?;
            items.sort_by_key(|i| (i.available_at, i.id));
            Ok(items)
        })
        .await
    }

    async fn park_inbox(&self, lease: &InboxLease, now: Timestamp) -> Result<Parking, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            write(conn, |tx| {
                let correlation = row_opt(
                    tx,
                    "SELECT correlation FROM inbox WHERE id = ?1 AND lease_owner = ?2 \
                     AND attempts = ?3 AND status = 'inflight'",
                    &[
                        &lease.id.0.to_string(),
                        &lease.owner,
                        &attempt(lease.attempt),
                    ],
                    |row| row.get::<_, Option<String>>(0).map_err(store_err),
                )?;
                let Some(correlation) = correlation else {
                    return Ok(Parking::Lost);
                };
                let watched = match &correlation {
                    Some(key) => {
                        row_opt(tx, "SELECT 1 FROM watches WHERE key = ?1", &[&key], |_| {
                            Ok(())
                        })?
                        .is_some()
                    }
                    None => false,
                };
                let now = to_db(now)?;
                let moved = exec(
                    tx,
                    "UPDATE inbox SET status = CASE WHEN ?4 THEN 'pending' ELSE 'parked' END, \
                     available_at = CASE WHEN ?4 THEN ?5 ELSE available_at END, \
                     parked_at = CASE WHEN ?4 THEN NULL ELSE ?5 END, \
                     refunded = attempts, \
                     lease_owner = NULL, lease_until = NULL, updated_at = ?5 \
                     WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 \
                       AND status = 'inflight'",
                    &[
                        &lease.id.0.to_string(),
                        &lease.owner,
                        &attempt(lease.attempt),
                        &watched,
                        &now,
                    ],
                )?;
                if moved != 1 {
                    return Ok(Parking::Lost);
                }
                Ok(if watched {
                    Parking::Rearmed
                } else {
                    Parking::Parked
                })
            })
        })
        .await
    }

    async fn retry_inbox(
        &self,
        lease: &InboxLease,
        available_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE inbox SET status = 'pending', available_at = ?4, lease_owner = NULL, \
                 lease_until = NULL, last_error = ?5, \
                 updated_at = CAST(unixepoch('subsec') * 1000000 AS INTEGER) \
                 WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'",
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &to_db(available_at)?,
                    &error,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn complete_inbox(
        &self,
        lease: &InboxLease,
        outcome: InboxFinal,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let lease = lease.clone();
        self.call(move |conn| {
            let (status, error) = match outcome {
                InboxFinal::Applied => ("applied", None),
                InboxFinal::Dead { error } => ("dead", Some(error)),
            };
            exec(
                conn,
                "UPDATE inbox SET status = ?4, last_error = COALESCE(?5, last_error), \
                 lease_owner = NULL, lease_until = NULL, updated_at = ?6 \
                 WHERE id = ?1 AND lease_owner = ?2 AND attempts = ?3 AND status = 'inflight'",
                &[
                    &lease.id.0.to_string(),
                    &lease.owner,
                    &attempt(lease.attempt),
                    &status,
                    &error.as_deref(),
                    &to_db(now)?,
                ],
            )
            .map(|n| n == 1)
        })
        .await
    }

    async fn expire_parked_inbox(
        &self,
        parked_at_or_before: Timestamp,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE inbox SET status = 'expired', updated_at = ?2 \
                 WHERE status = 'parked' AND parked_at <= ?1",
                &[&to_db(parked_at_or_before)?, &to_db(now)?],
            )
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn release_inbox_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let owner = owner.to_owned();
        self.call(move |conn| {
            exec(
                conn,
                "UPDATE inbox SET lease_until = ?2, refunded = refunded + 1, updated_at = ?2 \
                 WHERE status = 'inflight' AND lease_owner = ?1",
                &[&owner, &to_db(now)?],
            )
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        })
        .await
    }

    async fn get_watch(&self, key: &str) -> Result<Option<ThreadId>, StoreError> {
        let key = key.to_owned();
        self.call(move |conn| {
            let found = row_opt(
                conn,
                "SELECT thread_id FROM watches WHERE key = ?1",
                &[&key],
                |row| row.get::<_, String>(0).map_err(store_err),
            )?;
            found.map(|id| parse_id(&id).map(ThreadId)).transpose()
        })
        .await
    }

    async fn get_inbox(&self, id: InboxId) -> Result<Option<InboxItem>, StoreError> {
        let id = id.0.to_string();
        self.call(move |conn| {
            row_opt(
                conn,
                &format!("SELECT {INBOX_COLS} FROM inbox WHERE id = ?1"),
                &[&id],
                inbox_from_row,
            )
        })
        .await
    }

    async fn find_inbox(
        &self,
        source: &str,
        idempotency_key: &str,
    ) -> Result<Option<InboxItem>, StoreError> {
        let source = source.to_owned();
        let key = idempotency_key.to_owned();
        self.call(move |conn| {
            row_opt(
                conn,
                &format!(
                    "SELECT {INBOX_COLS} FROM inbox WHERE source = ?1 AND idempotency_key = ?2"
                ),
                &[&source, &key],
                inbox_from_row,
            )
        })
        .await
    }
}
