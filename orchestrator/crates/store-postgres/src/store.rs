//! `ThreadStore` over Postgres.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    EditLink, Event, EventKind, ForkKind, ForkNode, Job, NONCE_LEN, RankError, ThreadId,
    ThreadRecord, UserId, WatchKey, between, spread,
};
use orch_ports::{
    AgentBinding, ArchivedFilter, Arrangement, BindingUpdate, Commit, CommitOutcome, ForkOrigin,
    InboxFinal, InboxId, InboxItem, InboxLease, Lease, ListOrder, NewEvent, NewInbox, NewOutbox,
    NewThreadRecord, NewTimer, OutboxFinal, OutboxId, OutboxItem, OutboxPayload, OutboxStats,
    Parking, Place, Received, SharingChange, StoreError, TIMER_SOURCE, ThreadListing, ThreadStore,
};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::codec::{
    binding_from_row, enum_str, event_from_row, get_ts, get_ts_opt, inbox_cols, inbox_from_row,
    outbox_cols, outbox_from_row, parse_enum, thread_cols, thread_from_row, to_db, ts,
};
use crate::error::{is_unique_violation, migrate_err, store_err};
use crate::wakeup::{CHANNEL_INBOX, CHANNEL_OUTBOX, CHANNEL_THREAD};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Default pool size of [`PgStore::connect`]; one connection is held by the wakeup listener.
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// [`ThreadStore`] on Postgres (sqlx). Cheap to clone: clones share the pool.
///
/// Every write that other processes should react to also sends a `NOTIFY` inside its own
/// transaction (delivered only if it commits): `orch_thread` with the thread id after a
/// commit, `orch_outbox` after new or freed outbox rows, `orch_inbox` after a received or
/// re-armed inbox row. Pair it with
/// [`PgWakeup`](crate::PgWakeup) to hear them.
#[derive(Clone, Debug)]
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connects a pool of [`DEFAULT_MAX_CONNECTIONS`] (10) connections. Does not migrate; call
    /// [`migrate`](Self::migrate) at boot.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] if the database cannot be reached.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        Self::connect_with(url, DEFAULT_MAX_CONNECTIONS).await
    }

    /// Like [`connect`](Self::connect) with an explicit pool size (at least 2 when a
    /// [`PgWakeup`](crate::PgWakeup) shares the pool).
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] if the database cannot be reached.
    pub async fn connect_with(url: &str, max_connections: u32) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .connect(url)
            .await
            .map_err(store_err)?;
        Ok(Self { pool })
    }

    /// Wraps an existing pool (tests use it to pin a `search_path`).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The underlying pool, e.g. to start a [`PgWakeup`](crate::PgWakeup) on it.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Applies the embedded migrations. Idempotent, and safe to call from every replica at
    /// the same time: sqlx serialises runners with a Postgres advisory lock and records what
    /// it applied in `_sqlx_migrations`, so a second runner finds nothing left to do.
    ///
    /// # Errors
    /// [`StoreError::Unavailable`] if the database is unreachable; [`StoreError::Internal`] if
    /// it rejects a migration's statements; [`StoreError::Corrupt`] if an applied migration was
    /// edited afterwards.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        MIGRATOR.run(&self.pool).await.map_err(migrate_err)
    }
}

impl PgStore {
    /// Inserts a thread, for [`ThreadStore::create_thread`] and [`ThreadStore::fork_thread`]: for a
    /// fork, the parent's events up to the cut are copied in before the first commit's.
    async fn create(
        &self,
        new: NewThreadRecord,
        fork: Option<ForkOrigin>,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        let cut = fork.map_or(0, |o| o.cut);
        if let Some(origin) = fork {
            // KEY SHARE: the parent cannot be deleted under us, and its own commits go on.
            let parent: Option<i64> = sqlx::query_scalar(
                "SELECT last_seq FROM threads WHERE id = $1 AND owner = $2 FOR KEY SHARE",
            )
            .bind(origin.parent.0)
            .bind(new.owner.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            let Some(last_seq) = parent else {
                rollback(tx).await;
                return Err(StoreError::NotFound);
            };
            if origin.cut < 0 || last_seq < origin.cut {
                rollback(tx).await;
                return Err(StoreError::corrupt("the cut is beyond the parent's log"));
            }
        }
        if let Some(parent) = new.rail_parent {
            // KEY SHARE: the thread it is nested under cannot go under us.
            let top_level = sqlx::query(
                "SELECT 1 FROM threads WHERE id = $1 AND owner = $2 AND rail_parent IS NULL \
                 FOR KEY SHARE",
            )
            .bind(parent.0)
            .bind(new.owner.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            if top_level.is_none() {
                rollback(tx).await;
                return Err(StoreError::corrupt(
                    "a thread is nested under a top-level thread of its owner's",
                ));
            }
        }
        let hidden = fork.is_some_and(|o| o.kind == ForkKind::Edit);
        let rail_rank = if new.rail_parent.is_some() || hidden {
            // a row the list does not rank: it takes the rank of the first, and burns no key
            lowest_rank(&mut tx, new.owner.as_str(), None)
                .await?
                .unwrap_or_else(|| "i".to_owned())
        } else {
            new_rank(&mut tx, new.owner.as_str(), new.id, Place::Top).await?
        };
        // Creation is version 1 whatever the first commit does.
        let inserted = sqlx::query(
            "INSERT INTO threads (id, owner, title, description, agent_id, release, state, job, \
             version, last_seq, created_at, updated_at, forked_from, forked_at, fork_kind, \
             rail_parent, rail_rank) \
             VALUES ($1, $2, $3, $13, $4, $5, $6, $7, 1, $9, $8, $8, $10, $11, $12, $14, $15)",
        )
        .bind(new.id.0)
        .bind(new.owner.as_str())
        .bind(&new.title)
        .bind(new.target.agent_id.as_str())
        .bind(new.target.release.as_deref())
        .bind(enum_str(&first.new_state)?)
        .bind(job_json(first.job.as_ref().unwrap_or(&Job::default()))?)
        .bind(to_db(new.now))
        .bind(cut)
        .bind(fork.map(|o| o.parent.0))
        .bind(fork.map(|o| o.cut))
        .bind(fork.map(|o| o.kind.as_str()))
        .bind(new.description.as_deref().filter(|d| !d.is_empty()))
        .bind(new.rail_parent.map(|p| p.0))
        .bind(&rail_rank)
        .execute(&mut *tx)
        .await;
        if let Err(e) = inserted {
            return Err(if is_unique_violation(&e, None) {
                StoreError::corrupt("thread id already exists")
            } else {
                store_err(e)
            });
        }
        sqlx::query(
            "INSERT INTO a2a_bindings (thread_id, agent_id, context_id, updated_at) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(new.id.0)
        .bind(new.target.agent_id.as_str())
        .bind(new.context_id.as_deref())
        .bind(to_db(new.now))
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        if let Some(update) = &first.binding {
            update_binding(&mut tx, new.id, update, first.now).await?;
        }
        let has_outbox = !first.outbox.is_empty();
        if let Some(origin) = fork {
            // The parent's events as they are: the same seq, time, actor and data. Events up to
            // the cut never change, so no lock on them is needed.
            let copied = sqlx::query(
                "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
                 SELECT $1, seq, at, kind, actor, data FROM events \
                 WHERE thread_id = $2 AND seq <= $3",
            )
            .bind(new.id.0)
            .bind(origin.parent.0)
            .bind(origin.cut)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?
            .rows_affected();
            if i64::try_from(copied).ok() != Some(origin.cut) {
                rollback(tx).await;
                return Err(StoreError::corrupt(
                    "the parent's log has a gap before the cut",
                ));
            }
        }
        let events = insert_events(&mut tx, new.id, cut + 1, first.events).await?;
        insert_outbox(&mut tx, new.id, first.outbox, first.now).await?;
        let rearmed =
            insert_watches_and_timers(&mut tx, new.id, &first.watches, first.timers, first.now)
                .await?;
        let last_seq = cut + i64::try_from(events.len()).unwrap_or(i64::MAX - cut);
        let row = sqlx::query(concat!(
            "UPDATE threads SET last_seq = $2, updated_at = $3 WHERE id = $1 RETURNING ",
            thread_cols!()
        ))
        .bind(new.id.0)
        .bind(last_seq)
        .bind(to_db(first.now))
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        let record = thread_from_row(&row)?;
        notify_thread(&mut tx, new.id).await?;
        if has_outbox {
            notify_outbox(&mut *tx).await?;
        }
        if rearmed {
            notify_inbox(&mut *tx).await?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok((record, events))
    }
}

/// Which threads a listing may name, as the SQL reads it: `$1` is the owner, `$2` whether the
/// threads made by an edit are listed.
macro_rules! listed {
    () => {
        "t.owner = $1 AND ($2 OR t.fork_kind IS DISTINCT FROM 'edit')"
    };
}

/// The three sections in order (pinned, the rest, archived); the `sec` of a row.
macro_rules! section {
    () => {
        "CASE WHEN t.archived_at IS NOT NULL THEN 2 \
         WHEN t.pinned_at IS NOT NULL THEN 0 ELSE 1 END"
    };
}

/// What follows the cursor in the order of the units, with `$4` the cursor's section, `$5` its
/// rank, `$6` its archived time and `$7` its id; with none, `$4` to `$7` are all null.
macro_rules! after_cursor {
    () => {
        "AND (u.sec > $4 \
           OR (u.sec = $4 AND $4 < 2 AND (u.rail_rank > $5 COLLATE \"C\" \
               OR (u.rail_rank = $5 COLLATE \"C\" AND u.id < $7))) \
           OR (u.sec = $4 AND $4 = 2 AND (u.archived_at < $6 \
               OR (u.archived_at = $6 AND u.id < $7))))"
    };
}

macro_rules! no_cursor {
    () => {
        "AND ($4::int IS NULL AND $5::text IS NULL AND $6::timestamptz IS NULL \
           AND $7::uuid IS NULL)"
    };
}

/// The top-level units of a [`ListOrder::Rail`] listing under a filter (ADR 0042), after the
/// cursor or from the start, in the owner's order: by section, then rank (ties newest first) for
/// the pinned and the rest, by archived time for the archived.
macro_rules! units {
    ($filter:expr, $cursor:expr) => {
        concat!(
            "SELECT ",
            thread_cols!(),
            " FROM (SELECT t.*, ",
            section!(),
            " AS sec FROM threads t WHERE ",
            listed!(),
            " AND ",
            $filter,
            ") u WHERE u.owner = $1 ",
            $cursor,
            " ORDER BY u.sec, CASE WHEN u.sec < 2 THEN u.rail_rank END, \
             u.archived_at DESC NULLS LAST, u.id DESC LIMIT $3"
        )
    };
}

/// The units a filter lists: top-level rows, minus the archived (`exclude`), all (`include`), or
/// the archived whose block is not (`only`).
macro_rules! unit_filter_exclude {
    () => {
        "t.rail_parent IS NULL AND t.archived_at IS NULL"
    };
}
macro_rules! unit_filter_include {
    () => {
        "t.rail_parent IS NULL"
    };
}
macro_rules! unit_filter_only {
    () => {
        "t.archived_at IS NOT NULL AND (t.rail_parent IS NULL OR NOT EXISTS ( \
         SELECT 1 FROM threads p WHERE p.id = t.rail_parent AND p.archived_at IS NOT NULL))"
    };
}

/// The query of the cursor's own position, for a filter: it is none when the filter does not list
/// the cursor as a unit.
macro_rules! cursor_position {
    ($filter:expr) => {
        concat!(
            "SELECT ",
            section!(),
            " AS sec, t.rail_rank, t.archived_at, t.id FROM threads t WHERE ",
            listed!(),
            " AND ",
            $filter,
            " AND t.id = $3"
        )
    };
}

impl PgStore {
    async fn list_recent(
        &self,
        owner: &UserId,
        listing: &ThreadListing,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        if let Some(cursor) = listing.before {
            let known = sqlx::query("SELECT 1 FROM threads WHERE id = $1 AND owner = $2")
                .bind(cursor.0)
                .bind(owner.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(store_err)?;
            if known.is_none() {
                return Ok(Vec::new());
            }
        }
        sqlx::query(concat!(
            "SELECT ",
            thread_cols!(),
            " FROM threads WHERE owner = $1 AND ($2::uuid IS NULL OR id < $2) \
             AND ($4 OR fork_kind IS DISTINCT FROM 'edit') \
             AND (($5 = 'include') OR (($5 = 'only') = (archived_at IS NOT NULL))) \
             ORDER BY id DESC LIMIT $3"
        ))
        .bind(owner.as_str())
        .bind(listing.before.map(|b| b.0))
        .bind(i64::from(listing.limit))
        .bind(listing.include_edits)
        .bind(archived_name(listing.archived))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(thread_from_row)
        .collect()
    }

    /// The owner's list in their own order: the page of top-level rows, then the children of those.
    async fn list_rail(
        &self,
        owner: &UserId,
        listing: &ThreadListing,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        // Where the cursor stands in the order: its section, rank, archived time and id. A cursor
        // the filter does not list as a unit (unknown, foreign, nested) has no page after it.
        let cursor = match listing.before {
            None => None,
            Some(id) => {
                let position = match listing.archived {
                    ArchivedFilter::Exclude => cursor_position!(unit_filter_exclude!()),
                    ArchivedFilter::Include => cursor_position!(unit_filter_include!()),
                    ArchivedFilter::Only => cursor_position!(unit_filter_only!()),
                };
                let row = sqlx::query(position)
                    .bind(owner.as_str())
                    .bind(listing.include_edits)
                    .bind(id.0)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(store_err)?;
                let Some(row) = row else {
                    return Ok(Vec::new());
                };
                Some((
                    row.try_get::<i32, _>("sec").map_err(store_err)?,
                    row.try_get::<String, _>("rail_rank").map_err(store_err)?,
                    get_ts_opt(&row, "archived_at")?,
                    row.try_get::<uuid::Uuid, _>("id").map_err(store_err)?,
                ))
            }
        };
        let query = match (listing.archived, cursor.is_some()) {
            (ArchivedFilter::Exclude, true) => units!(unit_filter_exclude!(), after_cursor!()),
            (ArchivedFilter::Exclude, false) => units!(unit_filter_exclude!(), no_cursor!()),
            (ArchivedFilter::Include, true) => units!(unit_filter_include!(), after_cursor!()),
            (ArchivedFilter::Include, false) => units!(unit_filter_include!(), no_cursor!()),
            (ArchivedFilter::Only, true) => units!(unit_filter_only!(), after_cursor!()),
            (ArchivedFilter::Only, false) => units!(unit_filter_only!(), no_cursor!()),
        };
        let (sec, rank, at, id) = match cursor {
            Some((sec, rank, at, id)) => (Some(sec), Some(rank), at, Some(id)),
            None => (None, None, None, None),
        };
        let units = sqlx::query(query)
            .bind(owner.as_str())
            .bind(listing.include_edits)
            .bind(i64::from(listing.limit))
            .bind(sec)
            .bind(rank)
            .bind(at.map(to_db))
            .bind(id)
            .fetch_all(&self.pool)
            .await
            .map_err(store_err)?
            .iter()
            .map(thread_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        if units.is_empty() {
            return Ok(units);
        }
        let ids: Vec<uuid::Uuid> = units.iter().map(|u| u.id.0).collect();
        let children = sqlx::query(concat!(
            "SELECT ",
            thread_cols!(),
            " FROM threads WHERE owner = $1 AND rail_parent = ANY($2) \
             AND ($3 OR fork_kind IS DISTINCT FROM 'edit') AND ($4 OR archived_at IS NULL) \
             ORDER BY id DESC"
        ))
        .bind(owner.as_str())
        .bind(&ids)
        .bind(listing.include_edits)
        .bind(listing.archived != ArchivedFilter::Exclude)
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(thread_from_row)
        .collect::<Result<Vec<_>, _>>()?;
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
}

fn archived_name(filter: ArchivedFilter) -> &'static str {
    match filter {
        ArchivedFilter::Exclude => "exclude",
        ArchivedFilter::Only => "only",
        ArchivedFilter::Include => "include",
    }
}

/// The lowest rank of the owner's top-level rows, leaving `except` out.
async fn lowest_rank(
    tx: &mut Tx,
    owner: &str,
    except: Option<ThreadId>,
) -> Result<Option<String>, StoreError> {
    sqlx::query_scalar(
        "SELECT min(rail_rank) FROM threads \
         WHERE owner = $1 AND rail_parent IS NULL AND ($2::uuid IS NULL OR id <> $2)",
    )
    .bind(owner)
    .bind(except.map(|e| e.0))
    .fetch_one(&mut **tx)
    .await
    .map_err(store_err)
}

/// The row right after (or before) the position `(rank, id)` in a section of the owner's list: the
/// top-level, listed, unarchived rows, pinned or not, by rank with ties newest first. `skip` is the
/// row being moved, nobody's neighbour. Its id and its rank.
async fn neighbour(
    tx: &mut Tx,
    owner: &str,
    pinned: bool,
    at: (&str, uuid::Uuid),
    skip: Option<ThreadId>,
    next: bool,
) -> Result<Option<(uuid::Uuid, String)>, StoreError> {
    // The row after (or before) a position, among the section's rows: by rank, ties newest first.
    let query = if next {
        "SELECT id, rail_rank FROM threads \
             WHERE owner = $1 AND rail_parent IS NULL AND archived_at IS NULL \
               AND fork_kind IS DISTINCT FROM 'edit' AND (pinned_at IS NOT NULL) = $2 \
               AND ($5::uuid IS NULL OR id <> $5) \
               AND (rail_rank > $3 COLLATE \"C\" OR (rail_rank = $3 COLLATE \"C\" AND id < $4)) \
             ORDER BY rail_rank ASC, id DESC LIMIT 1"
    } else {
        "SELECT id, rail_rank FROM threads \
             WHERE owner = $1 AND rail_parent IS NULL AND archived_at IS NULL \
               AND fork_kind IS DISTINCT FROM 'edit' AND (pinned_at IS NOT NULL) = $2 \
               AND ($5::uuid IS NULL OR id <> $5) \
               AND (rail_rank < $3 COLLATE \"C\" OR (rail_rank = $3 COLLATE \"C\" AND id > $4)) \
             ORDER BY rail_rank DESC, id ASC LIMIT 1"
    };
    let row = sqlx::query(query)
        .bind(owner)
        .bind(pinned)
        .bind(at.0)
        .bind(at.1)
        .bind(skip.map(|s| s.0))
        .fetch_optional(&mut **tx)
        .await
        .map_err(store_err)?;
    row.map(|r| {
        Ok((
            r.try_get("id").map_err(store_err)?,
            r.try_get("rail_rank").map_err(store_err)?,
        ))
    })
    .transpose()
}

/// Writes the owner's top-level ranks again, evenly spread, in the order they have now.
async fn respread(tx: &mut Tx, owner: &str) -> Result<(), StoreError> {
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM threads WHERE owner = $1 AND rail_parent IS NULL \
         ORDER BY rail_rank, id DESC",
    )
    .bind(owner)
    .fetch_all(&mut **tx)
    .await
    .map_err(store_err)?;
    let ranks = spread(ids.len());
    sqlx::query(
        "UPDATE threads t SET rail_rank = v.rank FROM unnest($1::uuid[], $2::text[]) AS v(id, rank) \
         WHERE t.id = v.id",
    )
    .bind(&ids)
    .bind(&ranks)
    .execute(&mut **tx)
    .await
    .map_err(store_err)?;
    Ok(())
}

/// The key of the slot `place` names for `moving` among the owner's rows. Where no key fits (two
/// neighbours of one rank, or the cap), the owner's ranks are re-spread first, in the same
/// transaction.
async fn new_rank(
    tx: &mut Tx,
    owner: &str,
    moving: ThreadId,
    place: Place,
) -> Result<String, StoreError> {
    for attempt in 0..2 {
        let (lower, upper) = match place {
            Place::Top => (None, lowest_rank(tx, owner, Some(moving)).await?),
            Place::Before(anchor) | Place::After(anchor) => {
                let anchored = sqlx::query(
                    "SELECT rail_rank, pinned_at IS NOT NULL AS pinned FROM threads \
                     WHERE id = $1 AND owner = $2 AND rail_parent IS NULL \
                       AND archived_at IS NULL AND fork_kind IS DISTINCT FROM 'edit'",
                )
                .bind(anchor.0)
                .bind(owner)
                .fetch_optional(&mut **tx)
                .await
                .map_err(store_err)?
                .ok_or(StoreError::Refused("bad_anchor"))?;
                let rank: String = anchored.try_get("rail_rank").map_err(store_err)?;
                let pinned: bool = anchored.try_get("pinned").map_err(store_err)?;
                let at = (rank.as_str(), anchor.0);
                if matches!(place, Place::Before(_)) {
                    let before = neighbour(tx, owner, pinned, at, Some(moving), false).await?;
                    (before.map(|(_, r)| r), Some(rank.clone()))
                } else {
                    let after = neighbour(tx, owner, pinned, at, Some(moving), true).await?;
                    (Some(rank.clone()), after.map(|(_, r)| r))
                }
            }
        };
        match between(lower.as_deref(), upper.as_deref()) {
            Ok(key) => return Ok(key),
            Err(RankError::Order | RankError::TooLong) if attempt == 0 => {
                respread(tx, owner).await?;
            }
            Err(e) => return Err(StoreError::corrupt(e.to_string())),
        }
    }
    Err(StoreError::corrupt("no rank fits after a re-spread"))
}

/// Whether `rec` is already where `place` puts it, in its own section.
async fn already_there(
    tx: &mut Tx,
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
        Place::Top => neighbour(tx, owner, pinned, at, None, false)
            .await?
            .is_none(),
        Place::Before(a) => neighbour(tx, owner, pinned, at, None, true)
            .await?
            .is_some_and(|(id, _)| id == a.0),
        Place::After(a) => neighbour(tx, owner, pinned, at, None, false)
            .await?
            .is_some_and(|(id, _)| id == a.0),
    })
}

/// Where an ejected row goes: right after the block it left, unless that block is pinned or
/// archived, which the row is not: then on top of the unpinned ones.
async fn after_block(tx: &mut Tx, owner: &str, rec: &ThreadRecord) -> Result<Place, StoreError> {
    let Some(parent) = rec.rail_parent else {
        return Ok(Place::Top);
    };
    let kept = sqlx::query(
        "SELECT 1 FROM threads WHERE id = $1 AND owner = $2 AND pinned_at IS NULL \
         AND archived_at IS NULL AND fork_kind IS DISTINCT FROM 'edit'",
    )
    .bind(parent.0)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await
    .map_err(store_err)?;
    Ok(if kept.is_some() {
        Place::After(parent)
    } else {
        Place::Top
    })
}

/// [`ThreadStore::arrange_thread`] in a transaction.
async fn arrange(
    tx: &mut Tx,
    owner: &UserId,
    id: ThreadId,
    change: Arrangement,
    now: Timestamp,
) -> Result<ThreadRecord, StoreError> {
    let row = sqlx::query(concat!(
        "SELECT ",
        thread_cols!(),
        " FROM threads WHERE id = $1 AND owner = $2 FOR UPDATE"
    ))
    .bind(id.0)
    .bind(owner.as_str())
    .fetch_optional(&mut **tx)
    .await
    .map_err(store_err)?
    .ok_or(StoreError::NotFound)?;
    let rec = thread_from_row(&row)?;
    let who = owner.as_str();
    let nested = rec.rail_parent.is_some();
    let unnest = change.unnest && nested;
    if nested && !unnest && (change.pinned == Some(true) || change.place.is_some()) {
        return Err(StoreError::Refused("nested_row"));
    }
    if let Some(Place::Before(a) | Place::After(a)) = change.place {
        let usable = a != id
            && sqlx::query(
                "SELECT 1 FROM threads WHERE id = $1 AND owner = $2 AND rail_parent IS NULL \
                 AND archived_at IS NULL AND fork_kind IS DISTINCT FROM 'edit'",
            )
            .bind(a.0)
            .bind(who)
            .fetch_optional(&mut **tx)
            .await
            .map_err(store_err)?
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
        Some(place) if in_place && already_there(tx, who, &rec, place).await? => None,
        Some(place) => Some(place),
        None if pin_changed => Some(Place::Top),
        None if unnest => Some(after_block(tx, who, &rec).await?),
        None => None,
    };
    if slot.is_none() && !archive_changed && !pin_changed && !unnest {
        return Ok(rec);
    }
    let rail_rank = match slot {
        Some(place) => new_rank(tx, who, id, place).await?,
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
    let row = sqlx::query(concat!(
        "UPDATE threads SET pinned_at = $2, archived_at = $3, rail_parent = $4, rail_rank = $5 \
         WHERE id = $1 RETURNING ",
        thread_cols!()
    ))
    .bind(id.0)
    .bind(pinned_at.map(to_db))
    .bind(archived_at.map(to_db))
    .bind(rail_parent.map(|p| p.0))
    .bind(&rail_rank)
    .fetch_one(&mut **tx)
    .await
    .map_err(store_err)?;
    thread_from_row(&row)
}

/// [`ThreadStore::delete_threads`] in a transaction.
async fn delete(
    tx: &mut Tx,
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
    // Locked in id order, whatever order the caller named them: two deletes of overlapping sets
    // cannot wait on each other. A fork or a commit that meets the lock waits, and finds the row
    // gone (or the version moved) when it gets it.
    let locked: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "SELECT id, version FROM threads WHERE id = ANY($1) AND owner = $2 ORDER BY id FOR UPDATE",
    )
    .bind(&ids)
    .bind(owner.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(store_err)?;
    if locked.len() != ids.len() {
        return Err(StoreError::NotFound);
    }
    let moved = threads.iter().any(|(id, version)| {
        locked
            .iter()
            .find(|(locked_id, _)| *locked_id == id.0)
            .is_none_or(|(_, v)| v != version)
    });
    if moved {
        return Err(StoreError::VersionConflict);
    }
    // An edit of one that goes that the caller did not name: it would be left with the log it was
    // cut from gone and no way for the person to see it. (One made while we hold the locks waits.)
    let missed = sqlx::query(
        "SELECT 1 FROM threads \
         WHERE fork_kind = 'edit' AND forked_from = ANY($1) AND NOT (id = ANY($1)) LIMIT 1",
    )
    .bind(&ids)
    .fetch_optional(&mut **tx)
    .await
    .map_err(store_err)?;
    if missed.is_some() {
        return Err(StoreError::VersionConflict);
    }
    // The threads nested under one that goes take its place in the owner's list: top-level, with
    // its rank (children are told apart by `id`, newest first, as the list reads them), its pin and
    // its archive. Before the delete, which would only null `rail_parent`.
    sqlx::query(
        "UPDATE threads c SET rail_parent = NULL, rail_rank = p.rail_rank, \
           pinned_at = p.pinned_at, archived_at = COALESCE(c.archived_at, p.archived_at) \
         FROM threads p \
         WHERE c.rail_parent = p.id AND p.id = ANY($1) AND NOT (c.id = ANY($1))",
    )
    .bind(&ids)
    .execute(&mut **tx)
    .await
    .map_err(store_err)?;
    // The timers of the inbox name the thread in their payload and have no foreign key.
    let names: Vec<String> = ids.iter().map(ToString::to_string).collect();
    sqlx::query("DELETE FROM inbox WHERE source = $1 AND payload ->> 'thread' = ANY($2)")
        .bind(TIMER_SOURCE)
        .bind(&names)
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    // The promise that the files will go, in the transaction that deletes the log.
    sqlx::query(
        "INSERT INTO thread_purges (thread_id, deleted_at) \
         SELECT unnest($1::uuid[]), $2 ON CONFLICT (thread_id) DO NOTHING",
    )
    .bind(&ids)
    .bind(to_db(now))
    .execute(&mut **tx)
    .await
    .map_err(store_err)?;
    // Cascades the events, outbox, binding and watches; nulls `forked_from` of the forks.
    sqlx::query("DELETE FROM threads WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// The family of edits of `$1` (a thread), the owner's `$2`: up the edit links to the thread the
/// family started from, then down them. `message` is the first message of a person after the
/// `thread_forked` event of a thread made by an edit.
const FORK_FAMILY: &str = "\
    WITH RECURSIVE up AS ( \
        SELECT id, forked_from, fork_kind, 0 AS depth FROM threads WHERE id = $1 AND owner = $2 \
        UNION ALL \
        SELECT p.id, p.forked_from, p.fork_kind, up.depth + 1 \
        FROM threads p JOIN up ON p.id = up.forked_from AND up.fork_kind = 'edit' \
        WHERE p.owner = $2 AND up.depth < 1000), \
    root AS (SELECT id FROM up ORDER BY depth DESC LIMIT 1), \
    down AS ( \
        SELECT id, 0 AS depth FROM root \
        UNION ALL \
        SELECT c.id, down.depth + 1 \
        FROM threads c JOIN down ON c.forked_from = down.id \
        WHERE c.fork_kind = 'edit' AND c.owner = $2 AND down.depth < 1000) \
    SELECT t.id, t.forked_from, t.forked_at, t.fork_kind, t.created_at, \
        (SELECT min(e.seq) FROM events e \
         WHERE e.thread_id = t.id AND e.kind = 'user_message' AND e.seq > t.forked_at) AS message \
    FROM threads t JOIN down ON down.id = t.id \
    ORDER BY t.created_at, t.id LIMIT 1000";

fn fork_node_from_row(row: &PgRow) -> Result<ForkNode, StoreError> {
    let id = ThreadId(row.try_get("id").map_err(store_err)?);
    let parent: Option<uuid::Uuid> = row.try_get("forked_from").map_err(store_err)?;
    let at: Option<i64> = row.try_get("forked_at").map_err(store_err)?;
    let kind: Option<String> = row.try_get("fork_kind").map_err(store_err)?;
    let message: Option<i64> = row.try_get("message").map_err(store_err)?;
    let created = row
        .try_get::<jiff_sqlx::Timestamp, _>("created_at")
        .map_err(store_err)?
        .to_jiff();
    let kind: Option<ForkKind> = kind.map(|k| parse_enum("fork kind", &k)).transpose()?;
    // A link is an edit whose parent still exists; any other thread starts a family of its own.
    let link = match (kind, parent, at) {
        (Some(ForkKind::Edit), Some(parent), Some(cut)) => Some(EditLink {
            parent: ThreadId(parent),
            cut,
            message: message.unwrap_or(cut + 2),
        }),
        _ => None,
    };
    Ok(ForkNode { id, link, created })
}

/// What a commit does to the thread's share, as the UPDATE reads it: `set`, `clear`, or NULL to
/// leave it.
fn sharing_op(change: Option<SharingChange>) -> Option<&'static str> {
    change.map(|c| match c {
        SharingChange::Set { .. } => "set",
        SharingChange::Clear => "clear",
    })
}

fn sharing_level(change: Option<SharingChange>) -> Option<&'static str> {
    match change {
        Some(SharingChange::Set { level, .. }) => Some(level.as_str()),
        Some(SharingChange::Clear) | None => None,
    }
}

fn sharing_nonce(change: Option<SharingChange>) -> Option<Vec<u8>> {
    match change {
        Some(SharingChange::Set { nonce, .. }) => Some(nonce.as_bytes().to_vec()),
        Some(SharingChange::Clear) | None => None,
    }
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(t)
}

/// The statement that ends a claimed outbox row: `$1` id, `$2` owner, `$3` attempt, `$4` status,
/// `$5` the error to keep, `$6` the time.
const OUTBOX_FINISH: &str = "UPDATE outbox SET status = $4, last_error = COALESCE($5, last_error), \
     lease_owner = NULL, lease_until = NULL, updated_at = $6 \
     WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'";

fn outbox_outcome(outcome: OutboxFinal) -> (&'static str, Option<String>) {
    match outcome {
        OutboxFinal::Delivered => ("delivered", None),
        OutboxFinal::Dead { error } => ("dead", Some(error)),
        OutboxFinal::Skipped => ("skipped", None),
    }
}

/// Ends the claimed row inside the commit's transaction (the claim was checked, and is held, above).
async fn finish_outbox_row(
    tx: &mut Tx,
    lease: &Lease,
    outcome: OutboxFinal,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (status, error) = outbox_outcome(outcome);
    sqlx::query(OUTBOX_FINISH)
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .bind(status)
        .bind(error)
        .bind(to_db(now))
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// `outbox.attempts` is an `integer`; a lease past `i32::MAX` matches no row.
fn attempt(lease: &Lease) -> i32 {
    i32::try_from(lease.attempt).unwrap_or(i32::MAX)
}

fn job_json(job: &Job) -> Result<serde_json::Value, StoreError> {
    serde_json::to_value(job).map_err(|e| StoreError::corrupt_with("cannot serialise the job", e))
}

type Tx = Transaction<'static, Postgres>;

async fn notify_thread(tx: &mut Tx, thread: ThreadId) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CHANNEL_THREAD)
        .bind(thread.to_string())
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(store_err)
}

/// Finishes the thread's unsent `delegate` and `steer` rows as `skipped`: those `pending`, and
/// those `inflight` whose lease expired before `now`. Returns the count.
async fn skip_unsent_rows<'e, E: sqlx::PgExecutor<'e>>(
    exec: E,
    thread: ThreadId,
    now: Timestamp,
) -> Result<u32, StoreError> {
    sqlx::query(
        "UPDATE outbox SET status = 'skipped', lease_owner = NULL, lease_until = NULL, \
         updated_at = $2 \
         WHERE thread_id = $1 AND kind IN ('delegate', 'steer') AND sent_at IS NULL \
           AND (status = 'pending' OR (status = 'inflight' AND lease_until < $2))",
    )
    .bind(thread.0)
    .bind(to_db(now))
    .execute(exec)
    .await
    .map(|r| u32::try_from(r.rows_affected()).unwrap_or(u32::MAX))
    .map_err(store_err)
}

async fn notify_outbox<'e, E: sqlx::PgExecutor<'e>>(exec: E) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(CHANNEL_OUTBOX)
        .execute(exec)
        .await
        .map(|_| ())
        .map_err(store_err)
}

async fn notify_inbox<'e, E: sqlx::PgExecutor<'e>>(exec: E) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(CHANNEL_INBOX)
        .execute(exec)
        .await
        .map(|_| ())
        .map_err(store_err)
}

/// `attempts` columns are `integer`; a lease past `i32::MAX` matches no row.
fn inbox_attempt(lease: &InboxLease) -> i32 {
    i32::try_from(lease.attempt).unwrap_or(i32::MAX)
}

/// Inserts the commit's watches and, in the same transaction, re-arms the parked inbox rows
/// they match; then inserts its timers. Returns whether any inbox row became claimable now.
///
/// Each watch key is locked (a transaction-scoped advisory lock) before it is inserted, and
/// `park_inbox` takes the same lock before it looks for the watch: of a commit adding a watch
/// and a worker parking a row for it, one goes first, and the second sees the first's
/// committed result. Without the lock both could read "no watch" / "no parked row" from their
/// own snapshots and leave the row parked behind a watch until it expires. Keys are locked in
/// sorted order, so two commits that both add several watches do not deadlock on the keys.
///
/// That is not a proof that nothing here can deadlock. A stale `park_inbox` (its claim was taken
/// over) holds the lock of key K and may wait on its inbox row, which the commit of the worker
/// that took the row over holds while that commit wants K. Postgres detects such a cycle, aborts
/// one of the two (`40P01`) and the store maps it to a transient error, which the caller
/// retries: the aborted transaction wrote nothing.
///
/// The re-arm locks the parked rows it changes in id order and skips a row that another
/// transaction holds (`FOR UPDATE SKIP LOCKED`), so it never waits for a row lock. Skipping is
/// safe: a parked row can only be locked by an expiry (which ends it as `expired`, where a
/// watch does not concern it any more), and the rows a `park_inbox` of the same key is
/// locking are `inflight`, not parked.
async fn insert_watches_and_timers(
    tx: &mut Tx,
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
            lock_watch(tx, key).await?;
            let inserted = sqlx::query(
                "INSERT INTO watches (key, thread_id, created_at) VALUES ($1, $2, $3) \
                 ON CONFLICT (key) DO NOTHING",
            )
            .bind(*key)
            .bind(thread.0)
            .bind(to_db(now))
            .execute(&mut **tx)
            .await
            .map_err(store_err)?
            .rows_affected()
                == 1;
            if !inserted {
                warn_if_watched_by_another(tx, key, thread).await?;
            }
        }
        // A parked report was waiting, not failing: its claims so far are refunded.
        rearmed = sqlx::query(
            "WITH c AS ( \
               SELECT id FROM inbox WHERE status = 'parked' AND correlation = ANY($1) \
               ORDER BY id FOR UPDATE SKIP LOCKED) \
             UPDATE inbox SET status = 'pending', available_at = $2, parked_at = NULL, \
               refunded = attempts, updated_at = $2 \
             WHERE id IN (SELECT id FROM c)",
        )
        .bind(&keys)
        .bind(to_db(now))
        .execute(&mut **tx)
        .await
        .map_err(store_err)?
        .rows_affected()
            > 0;
    }
    for timer in timers {
        let payload = serde_json::to_value(timer.payload(thread))
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        sqlx::query(
            "INSERT INTO inbox (id, source, idempotency_key, kind, payload, status, \
             available_at, created_at, updated_at) \
             VALUES ($1, $2, $3, 'timer', $4, 'pending', $5, $6, $6) \
             ON CONFLICT (source, idempotency_key) DO NOTHING",
        )
        .bind(timer.id.0)
        .bind(TIMER_SOURCE)
        .bind(timer.idempotency_key(thread))
        .bind(payload)
        .bind(to_db(timer.due(now)))
        .bind(to_db(now))
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    }
    Ok(rearmed)
}

/// A key that is watched already stays with its first thread. When the thread that asked for it
/// now is another one, the reports for the key will never reach it: say so.
async fn warn_if_watched_by_another(
    tx: &mut Tx,
    key: &str,
    thread: ThreadId,
) -> Result<(), StoreError> {
    let owner: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT thread_id FROM watches WHERE key = $1")
            .bind(key)
            .fetch_optional(&mut **tx)
            .await
            .map_err(store_err)?;
    if let Some(owner) = owner.filter(|owner| *owner != thread.0) {
        tracing::warn!(
            watch = key,
            watched_by = %ThreadId(owner),
            asked_by = %thread,
            "a watch key is owned by another thread; this thread will not hear its reports"
        );
    }
    Ok(())
}

/// Takes the transaction-scoped lock that serialises adding a watch and parking for it.
async fn lock_watch(tx: &mut Tx, key: &str) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("orch:watch:{key}"))
        .execute(&mut **tx)
        .await
        .map(|_| ())
        .map_err(store_err)
}

/// Inserts `events` as `first_seq..`, returning them as stored.
async fn insert_events(
    tx: &mut Tx,
    thread: ThreadId,
    first_seq: i64,
    events: Vec<NewEvent>,
) -> Result<Vec<Event>, StoreError> {
    let mut stored = Vec::with_capacity(events.len());
    for (offset, new) in (0_i64..).zip(events) {
        let seq = first_seq + offset;
        let actor = serde_json::to_value(&new.actor)
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data, idempotency_key) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(thread.0)
        .bind(seq)
        .bind(to_db(new.at))
        .bind(new.body.kind().as_str())
        .bind(actor)
        .bind(new.body.data_value())
        .bind(new.idempotency_key.as_deref())
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            if is_unique_violation(&e, Some("events_idempotency")) {
                StoreError::corrupt("the same idempotency key twice in one commit")
            } else {
                store_err(e)
            }
        })?;
        stored.push(Event {
            seq,
            thread_id: thread,
            at: ts(new.at),
            actor: new.actor,
            body: new.body,
        });
    }
    Ok(stored)
}

async fn insert_outbox(
    tx: &mut Tx,
    thread: ThreadId,
    rows: Vec<NewOutbox>,
    now: Timestamp,
) -> Result<(), StoreError> {
    for row in rows {
        let payload = serde_json::to_value(&row.payload)
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        sqlx::query(
            "INSERT INTO outbox (id, thread_id, kind, payload, status, next_attempt_at, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, 'pending', $5, $5, $5)",
        )
        .bind(row.id.0)
        .bind(thread.0)
        .bind(enum_str(&row.payload.kind())?)
        .bind(payload)
        .bind(to_db(now))
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    }
    Ok(())
}

async fn update_binding(
    tx: &mut Tx,
    thread: ThreadId,
    update: &BindingUpdate,
    now: Timestamp,
) -> Result<(), StoreError> {
    let task_state = update.task_state.as_ref().map(enum_str).transpose()?;
    sqlx::query(
        // The context is adopted once: the first one the agent assigned stands (ADR 0055).
        "UPDATE a2a_bindings SET task_id = COALESCE($2, task_id), \
         task_state = COALESCE($3, task_state), revision = COALESCE($4, revision), \
         context_id = COALESCE(context_id, $6), updated_at = $5 WHERE thread_id = $1",
    )
    .bind(thread.0)
    .bind(update.task_id.as_deref())
    .bind(task_state)
    .bind(update.revision.as_deref())
    .bind(to_db(now))
    .bind(update.context_id.as_deref().filter(|c| !c.is_empty()))
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(store_err)
}

/// Marks the inbox row `applied` (the caller holds it: its claim was checked `FOR UPDATE`).
async fn finish_inbox_row(
    tx: &mut Tx,
    lease: &InboxLease,
    now: Timestamp,
) -> Result<(), StoreError> {
    sqlx::query(
        "UPDATE inbox SET status = 'applied', lease_owner = NULL, lease_until = NULL, \
         updated_at = $2 WHERE id = $1",
    )
    .bind(lease.id.0)
    .bind(to_db(now))
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(store_err)
}

async fn rollback(tx: Tx) {
    // The transaction is abandoned either way; a failed ROLLBACK closes the connection.
    let _ = tx.rollback().await;
}

impl ThreadStore for PgStore {
    async fn ping(&self) -> Result<(), StoreError> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(store_err)
    }

    async fn create_thread(
        &self,
        new: NewThreadRecord,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        self.create(new, None, first).await
    }

    async fn fork_thread(
        &self,
        new: NewThreadRecord,
        origin: ForkOrigin,
        first: Commit,
    ) -> Result<(ThreadRecord, Vec<Event>), StoreError> {
        self.create(new, Some(origin), first).await
    }

    async fn fork_family(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> Result<Vec<ForkNode>, StoreError> {
        // Up from `thread` along edit links to the thread the family started from, then down from
        // it along edit links. The depth bounds are belts: a thread's parent is older than it, so
        // the links cannot loop.
        sqlx::query(FORK_FAMILY)
            .bind(thread.0)
            .bind(owner.as_str())
            .fetch_all(&self.pool)
            .await
            .map_err(store_err)?
            .iter()
            .map(fork_node_from_row)
            .collect()
    }

    async fn get_thread(
        &self,
        owner: Option<&UserId>,
        id: ThreadId,
    ) -> Result<Option<ThreadRecord>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            thread_cols!(),
            " FROM threads WHERE id = $1 AND ($2::text IS NULL OR owner = $2)"
        ))
        .bind(id.0)
        .bind(owner.map(UserId::as_str))
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(thread_from_row)
        .transpose()
    }

    async fn thread_by_share_nonce(
        &self,
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Option<ThreadRecord>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            thread_cols!(),
            " FROM threads WHERE share_nonce = $1"
        ))
        .bind(nonce.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(thread_from_row)
        .transpose()
    }

    async fn list_threads(
        &self,
        owner: &UserId,
        listing: ThreadListing,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        match listing.order {
            ListOrder::Recent => self.list_recent(owner, &listing).await,
            ListOrder::Rail => self.list_rail(owner, &listing).await,
        }
    }

    async fn arrange_thread(
        &self,
        owner: &UserId,
        id: ThreadId,
        change: Arrangement,
        now: Timestamp,
    ) -> Result<ThreadRecord, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        let outcome = arrange(&mut tx, owner, id, change, now).await;
        match outcome {
            Ok(record) => {
                tx.commit().await.map_err(store_err)?;
                Ok(record)
            }
            Err(e) => {
                rollback(tx).await;
                Err(e)
            }
        }
    }

    async fn delete_threads(
        &self,
        owner: &UserId,
        threads: &[(ThreadId, i64)],
        now: Timestamp,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        match delete(&mut tx, owner, threads, now).await {
            Ok(()) => tx.commit().await.map_err(store_err),
            Err(e) => {
                rollback(tx).await;
                Err(e)
            }
        }
    }

    async fn claim_purges(
        &self,
        owner: &str,
        limit: u32,
        lease: Duration,
        now: Timestamp,
    ) -> Result<Vec<ThreadId>, StoreError> {
        // One statement, as `claim_inbox`: candidates are picked oldest first under
        // `FOR UPDATE SKIP LOCKED`, so concurrent claimers get disjoint rows.
        let rows = sqlx::query(
            "WITH c AS ( \
               SELECT thread_id FROM thread_purges \
               WHERE lease_until IS NULL OR lease_until <= $1 \
               ORDER BY deleted_at, thread_id LIMIT $4 FOR UPDATE SKIP LOCKED) \
             UPDATE thread_purges SET lease_owner = $2, lease_until = $3, attempts = attempts + 1 \
             WHERE thread_id IN (SELECT thread_id FROM c) RETURNING deleted_at, thread_id",
        )
        .bind(to_db(now))
        .bind(owner)
        .bind(to_db(plus(ts(now), lease)))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        let mut rows = rows
            .iter()
            .map(|row| {
                Ok((
                    get_ts(row, "deleted_at")?,
                    row.try_get::<uuid::Uuid, _>("thread_id")
                        .map_err(store_err)?,
                ))
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        rows.sort();
        Ok(rows.into_iter().map(|(_, id)| ThreadId(id)).collect())
    }

    async fn finish_purge(&self, thread: ThreadId) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM thread_purges WHERE thread_id = $1")
            .bind(thread.0)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(store_err)
    }

    async fn purges_pending(&self) -> Result<u64, StoreError> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_purges")
            .fetch_one(&self.pool)
            .await
            .map_err(store_err)?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    async fn commit(
        &self,
        thread: ThreadId,
        expected_version: i64,
        commit: Commit,
    ) -> Result<CommitOutcome, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        // The row lock serialises every writer of this thread: the seq counter below cannot
        // be raced, so seq has no gaps and no duplicates (READ COMMITTED is enough).
        let locked =
            sqlx::query("SELECT version, last_seq, state FROM threads WHERE id = $1 FOR UPDATE")
                .bind(thread.0)
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_err)?;
        let Some(locked) = locked else {
            rollback(tx).await;
            return Err(StoreError::NotFound);
        };
        if let Some(lease) = &commit.lease {
            // Lock order is thread, then outbox row, then binding; nothing takes a thread lock
            // while holding an outbox row lock (outbox statements only touch outbox and
            // binding rows, and change no foreign key), so this cannot deadlock. FOR SHARE
            // keeps the claim in place until this transaction ends: a claimer's UPDATE waits
            // (or, with SKIP LOCKED, passes the row by), so the check cannot go stale before
            // the write below.
            let held = sqlx::query(
                "SELECT 1 FROM outbox WHERE id = $1 AND thread_id = $2 AND lease_owner = $3 \
                 AND attempts = $4 AND status = 'inflight' FOR SHARE",
            )
            .bind(lease.id.0)
            .bind(thread.0)
            .bind(&lease.owner)
            .bind(attempt(lease))
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            if held.is_none() {
                rollback(tx).await;
                return Ok(CommitOutcome::Fenced);
            }
        }
        if let Some(lease) = &commit.inbox {
            // FOR UPDATE: the row is marked applied below, so its claim must not move before
            // this transaction ends. Lock order is thread, outbox row, inbox row. The inbox
            // statements outside a commit lock one inbox row (claim with SKIP LOCKED, retry,
            // complete, release, expiry) or a watch key and then a row (`park_inbox`), never a
            // thread, so no cycle through the thread lock exists. One does through the watch
            // key: a stale `park_inbox` holding key K waits for this row while this commit,
            // holding the row, wants K in `insert_watches_and_timers`. Postgres detects such a
            // cycle (`40P01`), aborts one side, and the store maps that to a transient error.
            let held = sqlx::query(
                "SELECT 1 FROM inbox WHERE id = $1 AND lease_owner = $2 AND attempts = $3 \
                 AND status = 'inflight' FOR UPDATE",
            )
            .bind(lease.id.0)
            .bind(&lease.owner)
            .bind(inbox_attempt(lease))
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            if held.is_none() {
                rollback(tx).await;
                return Ok(CommitOutcome::Fenced);
            }
        }
        let version: i64 = locked.try_get("version").map_err(store_err)?;
        let last_seq: i64 = locked.try_get("last_seq").map_err(store_err)?;
        let state: String = locked.try_get("state").map_err(store_err)?;
        if version != expected_version {
            rollback(tx).await;
            return Err(StoreError::VersionConflict);
        }
        if commit.only_finishes_inbox() && enum_str(&commit.new_state)? == state {
            // Nothing to write to the thread. The claim and the version were checked above (a
            // "nothing to do" is only as good as the thread it was decided on); the row is
            // finished and the thread is left alone: no version bump, no wakeup.
            if let Some(lease) = &commit.inbox {
                finish_inbox_row(&mut tx, lease, commit.now).await?;
            }
            let row = sqlx::query(concat!(
                "SELECT ",
                thread_cols!(),
                " FROM threads WHERE id = $1"
            ))
            .bind(thread.0)
            .fetch_one(&mut *tx)
            .await
            .map_err(store_err)?;
            let record = thread_from_row(&row)?;
            tx.commit().await.map_err(store_err)?;
            return Ok(CommitOutcome::Applied {
                thread: record,
                events: Vec::new(),
            });
        }
        let keys: Vec<&str> = commit
            .events
            .iter()
            .filter_map(|e| e.idempotency_key.as_deref())
            .collect();
        if !keys.is_empty() {
            let seen = sqlx::query(
                "SELECT 1 FROM events WHERE thread_id = $1 AND idempotency_key = ANY($2) LIMIT 1",
            )
            .bind(thread.0)
            .bind(&keys)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            if seen.is_some() {
                rollback(tx).await;
                return Ok(CommitOutcome::Duplicate);
            }
        }
        let has_outbox = !commit.outbox.is_empty() || commit.finishes_outbox.is_some();
        let events = insert_events(&mut tx, thread, last_seq + 1, commit.events).await?;
        let new_last_seq = last_seq + i64::try_from(events.len()).unwrap_or(0);
        // The job is written with the state, under the same lock and version check; a commit
        // without one leaves the stored job alone.
        let job = commit.job.as_ref().map(job_json).transpose()?;
        let row = sqlx::query(concat!(
            "UPDATE threads SET state = $2, job = COALESCE($5, job), title = COALESCE($6, title), \
             description = CASE WHEN $7::text IS NULL THEN description ELSE NULLIF($7, '') END, \
             visibility = CASE $8::text WHEN 'set' THEN $9::text WHEN 'clear' THEN 'private' \
                 ELSE visibility END, \
             share_nonce = CASE $8::text WHEN 'set' THEN $10::bytea WHEN 'clear' THEN NULL \
                 ELSE share_nonce END, \
             shared_at = CASE $8::text WHEN 'set' THEN $4 WHEN 'clear' THEN NULL \
                 ELSE shared_at END, \
             version = version + 1, last_seq = $3, updated_at = $4 WHERE id = $1 RETURNING ",
            thread_cols!()
        ))
        .bind(thread.0)
        .bind(enum_str(&commit.new_state)?)
        .bind(new_last_seq)
        .bind(to_db(commit.now))
        .bind(job)
        .bind(commit.title.as_deref())
        .bind(commit.description.as_deref())
        .bind(sharing_op(commit.sharing))
        .bind(sharing_level(commit.sharing))
        .bind(sharing_nonce(commit.sharing))
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            // The unique index of the link's nonce: a capability is one thread's.
            if is_unique_violation(&e, Some("threads_share_nonce")) {
                StoreError::corrupt("share nonce already in use")
            } else {
                store_err(e)
            }
        })?;
        let record = thread_from_row(&row)?;
        // The abandoned job's unsent delegations are finished before this commit's own rows
        // exist, in the same transaction (ADR 0036).
        if commit.skip_unsent_delegates {
            skip_unsent_rows(&mut *tx, thread, commit.now).await?;
        }
        insert_outbox(&mut tx, thread, commit.outbox, commit.now).await?;
        if let Some(update) = &commit.binding {
            update_binding(&mut tx, thread, update, commit.now).await?;
        }
        let rearmed =
            insert_watches_and_timers(&mut tx, thread, &commit.watches, commit.timers, commit.now)
                .await?;
        if let Some(lease) = &commit.inbox {
            finish_inbox_row(&mut tx, lease, commit.now).await?;
        }
        if let (Some(lease), Some(outcome)) = (&commit.lease, commit.finishes_outbox.clone()) {
            finish_outbox_row(&mut tx, lease, outcome, commit.now).await?;
        }
        notify_thread(&mut tx, thread).await?;
        if has_outbox {
            notify_outbox(&mut *tx).await?;
        }
        if rearmed {
            notify_inbox(&mut *tx).await?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(CommitOutcome::Applied {
            thread: record,
            events,
        })
    }

    async fn list_events(
        &self,
        thread: ThreadId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        sqlx::query(
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
        )
        .bind(thread.0)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(|row| event_from_row(thread, row))
        .collect()
    }

    async fn ui_catalog_event(
        &self,
        thread: ThreadId,
        digest: &str,
    ) -> Result<Option<Event>, StoreError> {
        sqlx::query(
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = $1 AND kind = 'ui_catalog' AND data ->> 'digest' = $2 \
             ORDER BY seq DESC LIMIT 1",
        )
        .bind(thread.0)
        .bind(digest)
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(|row| event_from_row(thread, row))
        .transpose()
    }

    async fn latest_events(
        &self,
        thread: ThreadId,
        kind: EventKind,
        limit: u32,
    ) -> Result<Vec<Event>, StoreError> {
        sqlx::query(
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = $1 AND kind = $2 ORDER BY seq DESC LIMIT $3",
        )
        .bind(thread.0)
        .bind(kind.as_str())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(|row| event_from_row(thread, row))
        .collect()
    }

    async fn get_binding(&self, thread: ThreadId) -> Result<Option<AgentBinding>, StoreError> {
        sqlx::query(
            "SELECT thread_id, agent_id, context_id, task_id, task_state, revision \
             FROM a2a_bindings WHERE thread_id = $1",
        )
        .bind(thread.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(binding_from_row)
        .transpose()
    }

    async fn claim_outbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<OutboxItem>, StoreError> {
        // One statement. Candidates are picked oldest first under `FOR UPDATE SKIP LOCKED`,
        // so concurrent claimers get disjoint rows. A delegate row is held back while an
        // older delegate of the same thread is `pending` or `inflight` (whatever its lease
        // or due time says), which also covers an older row this very statement claims; a steer
        // row likewise waits for an older steer, and for no delegate (ADR 0036); an ask row waits
        // for nothing, as a verify row does (ADR 0026).
        let mut rows: Vec<(i64, PgRow)> = sqlx::query(concat!(
            "WITH c AS ( \
               SELECT o.id FROM outbox o \
               WHERE ((o.status = 'pending' AND o.next_attempt_at <= $1) \
                   OR (o.status = 'inflight' AND o.lease_until <= $1)) \
                 AND (o.kind IN ('cancel', 'verify', 'ask', 'title', 'description') OR NOT EXISTS ( \
                       SELECT 1 FROM outbox p \
                       WHERE p.thread_id = o.thread_id AND p.kind = o.kind \
                         AND p.ord < o.ord AND p.status IN ('pending', 'inflight'))) \
               ORDER BY o.ord LIMIT $4 FOR UPDATE OF o SKIP LOCKED) \
             UPDATE outbox SET status = 'inflight', lease_owner = $2, lease_until = $3, \
               attempts = attempts + 1, updated_at = $1 \
             WHERE id IN (SELECT id FROM c) RETURNING ord, ",
            outbox_cols!()
        ))
        .bind(to_db(now))
        .bind(owner)
        .bind(to_db(plus(ts(now), lease)))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .into_iter()
        .map(|row| row.try_get::<i64, _>("ord").map(|ord| (ord, row)))
        .collect::<Result<_, _>>()
        .map_err(store_err)?;
        rows.sort_by_key(|(ord, _)| *ord);
        rows.iter().map(|(_, row)| outbox_from_row(row)).collect()
    }

    async fn renew_lease(&self, lease: &Lease, until: Timestamp) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE outbox SET lease_until = $4 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .bind(to_db(until))
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn mark_sent(
        &self,
        lease: &Lease,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        let sent = sqlx::query(
            "UPDATE outbox SET sent_at = $4, updated_at = $4 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight' \
             RETURNING thread_id",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .bind(to_db(now))
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let Some(sent) = sent else {
            rollback(tx).await;
            return Ok(false);
        };
        let thread = ThreadId(sent.try_get("thread_id").map_err(store_err)?);
        update_binding(&mut tx, thread, &binding, now).await?;
        tx.commit().await.map_err(store_err)?;
        Ok(true)
    }

    async fn mark_verify_sent(
        &self,
        lease: &Lease,
        task_id: String,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE outbox SET sent_at = $4, task_id = $5, updated_at = $4 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .bind(to_db(now))
        .bind(task_id)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn retry_outbox(
        &self,
        lease: &Lease,
        next_attempt_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE outbox SET status = 'pending', next_attempt_at = $4, lease_owner = NULL, \
             lease_until = NULL, last_error = $5, updated_at = clock_timestamp() \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .bind(to_db(next_attempt_at))
        .bind(error)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn complete_outbox(
        &self,
        lease: &Lease,
        outcome: OutboxFinal,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let (status, error) = outbox_outcome(outcome);
        let done = sqlx::query(OUTBOX_FINISH)
            .bind(lease.id.0)
            .bind(&lease.owner)
            .bind(attempt(lease))
            .bind(status)
            .bind(error)
            .bind(to_db(now))
            .execute(&self.pool)
            .await
            .map(|r| r.rows_affected() == 1)
            .map_err(store_err)?;
        if done {
            // A finished delegate releases the thread's next one.
            notify_outbox(&self.pool).await?;
        }
        Ok(done)
    }

    async fn requeue_as_delegate(&self, lease: &Lease, now: Timestamp) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        let held = sqlx::query(
            "SELECT payload FROM outbox \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight' \
               AND kind = 'steer' FOR UPDATE",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(attempt(lease))
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let Some(held) = held else {
            rollback(tx).await;
            return Ok(false);
        };
        let payload: serde_json::Value = held.try_get("payload").map_err(store_err)?;
        let payload: OutboxPayload = serde_json::from_value(payload)
            .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
        let Some(delegate) = payload.steer_as_delegate() else {
            rollback(tx).await;
            return Ok(false);
        };
        let delegate = serde_json::to_value(&delegate)
            .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
        // `ord` and `created_at` stay: the delegation keeps the steer's place in the thread's order
        sqlx::query(
            "UPDATE outbox SET kind = 'delegate', payload = $2, status = 'pending', \
             next_attempt_at = $3, lease_owner = NULL, lease_until = NULL, updated_at = $3 \
             WHERE id = $1",
        )
        .bind(lease.id.0)
        .bind(delegate)
        .bind(to_db(now))
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        notify_outbox(&mut *tx).await?;
        tx.commit().await.map_err(store_err)?;
        Ok(true)
    }

    async fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        skip_unsent_rows(&self.pool, thread, now).await
    }

    async fn release_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let n = sqlx::query(
            "UPDATE outbox SET lease_until = $2, updated_at = $2 \
             WHERE status = 'inflight' AND lease_owner = $1",
        )
        .bind(owner)
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| u32::try_from(r.rows_affected()).unwrap_or(u32::MAX))
        .map_err(store_err)?;
        if n > 0 {
            notify_outbox(&self.pool).await?;
        }
        Ok(n)
    }

    async fn outbox_stats(&self, now: Timestamp) -> Result<OutboxStats, StoreError> {
        // One scan of the `outbox_open` partial index (only open rows are in it). The due
        // predicate is the one `claim_outbox` uses. `count(*)` is bigint, and the aggregate of
        // no row is 0 / NULL, so an empty outbox gives zeros and `None`.
        let row = sqlx::query(
            "SELECT \
               count(*) FILTER (WHERE (status = 'pending' AND next_attempt_at <= $1) \
                                   OR (status = 'inflight' AND lease_until <= $1)) AS due, \
               count(*) FILTER (WHERE status = 'pending' AND next_attempt_at > $1) AS waiting, \
               count(*) FILTER (WHERE status = 'inflight' AND lease_until > $1) AS leased, \
               min(CASE WHEN status = 'pending' AND next_attempt_at <= $1 THEN next_attempt_at \
                        WHEN status = 'inflight' AND lease_until <= $1 THEN lease_until END) \
                 AS oldest_due_at \
             FROM outbox WHERE status IN ('pending', 'inflight')",
        )
        .bind(to_db(now))
        .fetch_one(&self.pool)
        .await
        .map_err(store_err)?;
        let count = |name: &str| -> Result<u64, StoreError> {
            let n: i64 = row.try_get(name).map_err(store_err)?;
            u64::try_from(n).map_err(|e| StoreError::corrupt_with("negative outbox count", e))
        };
        Ok(OutboxStats {
            due: count("due")?,
            waiting: count("waiting")?,
            leased: count("leased")?,
            oldest_due_at: get_ts_opt(&row, "oldest_due_at")?,
        })
    }

    async fn get_outbox(&self, id: OutboxId) -> Result<Option<OutboxItem>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            outbox_cols!(),
            " FROM outbox WHERE id = $1"
        ))
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(outbox_from_row)
        .transpose()
    }

    async fn list_open_outbox(&self, thread: ThreadId) -> Result<Vec<OutboxItem>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            outbox_cols!(),
            " FROM outbox WHERE thread_id = $1 AND status IN ('pending', 'inflight') ORDER BY ord"
        ))
        .bind(thread.0)
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(outbox_from_row)
        .collect()
    }

    async fn receive(&self, row: NewInbox, now: Timestamp) -> Result<Received, StoreError> {
        let payload = serde_json::to_value(&row.payload)
            .map_err(|e| StoreError::corrupt_with("cannot serialise", e))?;
        let stored = sqlx::query(
            "INSERT INTO inbox (id, source, idempotency_key, kind, payload, correlation, status, \
             available_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $7, $7) \
             ON CONFLICT (source, idempotency_key) DO NOTHING",
        )
        .bind(row.id.0)
        .bind(&row.source)
        .bind(&row.idempotency_key)
        .bind(row.payload.kind())
        .bind(payload)
        .bind(row.correlation.as_deref())
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)?;
        if !stored {
            return Ok(Received::Duplicate);
        }
        notify_inbox(&self.pool).await?;
        Ok(Received::Stored { id: row.id })
    }

    async fn claim_inbox(
        &self,
        owner: &str,
        now: Timestamp,
        lease: Duration,
        limit: u32,
    ) -> Result<Vec<InboxItem>, StoreError> {
        // One statement, as `claim_outbox`: candidates are picked earliest first under
        // `FOR UPDATE SKIP LOCKED`, so concurrent claimers get disjoint rows, and a timer
        // whose `available_at` is still ahead of `now` is not a candidate.
        let mut items = sqlx::query(concat!(
            "WITH c AS ( \
               SELECT id FROM inbox \
               WHERE (status = 'pending' AND available_at <= $1) \
                  OR (status = 'inflight' AND lease_until <= $1) \
               ORDER BY available_at, id LIMIT $4 FOR UPDATE SKIP LOCKED) \
             UPDATE inbox SET status = 'inflight', lease_owner = $2, lease_until = $3, \
               attempts = attempts + 1, updated_at = $1 \
             WHERE id IN (SELECT id FROM c) RETURNING ",
            inbox_cols!()
        ))
        .bind(to_db(now))
        .bind(owner)
        .bind(to_db(plus(ts(now), lease)))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(inbox_from_row)
        .collect::<Result<Vec<_>, _>>()?;
        items.sort_by_key(|i| (i.available_at, i.id));
        Ok(items)
    }

    async fn park_inbox(&self, lease: &InboxLease, now: Timestamp) -> Result<Parking, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        // The correlation never changes, so it can be read before the row is locked: the lock
        // order is the watch key, then the row (see `insert_watches_and_timers`).
        let row = sqlx::query(
            "SELECT correlation FROM inbox WHERE id = $1 AND lease_owner = $2 AND attempts = $3 \
             AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(inbox_attempt(lease))
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let Some(row) = row else {
            rollback(tx).await;
            return Ok(Parking::Lost);
        };
        let correlation: Option<String> = row.try_get("correlation").map_err(store_err)?;
        if let Some(key) = &correlation {
            lock_watch(&mut tx, key).await?;
        }
        // Under the lock: a watch committed before it is seen here (each statement takes a
        // fresh snapshot), and one that commits after finds this row parked and re-arms it.
        let watched = match &correlation {
            Some(key) => sqlx::query("SELECT 1 FROM watches WHERE key = $1")
                .bind(key)
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_err)?
                .is_some(),
            None => false,
        };
        let moved = sqlx::query(
            "UPDATE inbox SET status = CASE WHEN $4 THEN 'pending' ELSE 'parked' END, \
             available_at = CASE WHEN $4 THEN $5 ELSE available_at END, \
             parked_at = CASE WHEN $4 THEN NULL ELSE $5 END, \
             refunded = attempts, \
             lease_owner = NULL, lease_until = NULL, updated_at = $5 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(inbox_attempt(lease))
        .bind(watched)
        .bind(to_db(now))
        .execute(&mut *tx)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)?;
        if !moved {
            rollback(tx).await;
            return Ok(Parking::Lost);
        }
        if watched {
            notify_inbox(&mut *tx).await?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(if watched {
            Parking::Rearmed
        } else {
            Parking::Parked
        })
    }

    async fn retry_inbox(
        &self,
        lease: &InboxLease,
        available_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE inbox SET status = 'pending', available_at = $4, lease_owner = NULL, \
             lease_until = NULL, last_error = $5, updated_at = clock_timestamp() \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(inbox_attempt(lease))
        .bind(to_db(available_at))
        .bind(error)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn complete_inbox(
        &self,
        lease: &InboxLease,
        outcome: InboxFinal,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let (status, error) = match outcome {
            InboxFinal::Applied => ("applied", None),
            InboxFinal::Dead { error } => ("dead", Some(error)),
        };
        sqlx::query(
            "UPDATE inbox SET status = $4, last_error = COALESCE($5, last_error), \
             lease_owner = NULL, lease_until = NULL, updated_at = $6 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
        .bind(lease.id.0)
        .bind(&lease.owner)
        .bind(inbox_attempt(lease))
        .bind(status)
        .bind(error)
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn expire_parked_inbox(
        &self,
        parked_at_or_before: Timestamp,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        // Rows are locked in id order and a row somebody else holds is left for the next pass,
        // so this statement never waits for a row lock and cannot be part of a deadlock. (A
        // row held by a re-arm is not expired anyway: it is not parked any more.)
        sqlx::query(
            "WITH c AS ( \
               SELECT id FROM inbox WHERE status = 'parked' AND parked_at <= $1 \
               ORDER BY id FOR UPDATE SKIP LOCKED) \
             UPDATE inbox SET status = 'expired', updated_at = $2 \
             WHERE id IN (SELECT id FROM c)",
        )
        .bind(to_db(parked_at_or_before))
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| u32::try_from(r.rows_affected()).unwrap_or(u32::MAX))
        .map_err(store_err)
    }

    async fn release_inbox_leases(&self, owner: &str, now: Timestamp) -> Result<u32, StoreError> {
        let n = sqlx::query(
            "UPDATE inbox SET lease_until = $2, refunded = refunded + 1, updated_at = $2 \
             WHERE status = 'inflight' AND lease_owner = $1",
        )
        .bind(owner)
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| u32::try_from(r.rows_affected()).unwrap_or(u32::MAX))
        .map_err(store_err)?;
        if n > 0 {
            notify_inbox(&self.pool).await?;
        }
        Ok(n)
    }

    async fn get_watch(&self, key: &str) -> Result<Option<ThreadId>, StoreError> {
        let row = sqlx::query("SELECT thread_id FROM watches WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(store_err)?;
        row.map(|r| r.try_get("thread_id").map(ThreadId).map_err(store_err))
            .transpose()
    }

    async fn get_inbox(&self, id: InboxId) -> Result<Option<InboxItem>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            inbox_cols!(),
            " FROM inbox WHERE id = $1"
        ))
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(inbox_from_row)
        .transpose()
    }

    async fn find_inbox(
        &self,
        source: &str,
        idempotency_key: &str,
    ) -> Result<Option<InboxItem>, StoreError> {
        sqlx::query(concat!(
            "SELECT ",
            inbox_cols!(),
            " FROM inbox WHERE source = $1 AND idempotency_key = $2"
        ))
        .bind(source)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(store_err)?
        .as_ref()
        .map(inbox_from_row)
        .transpose()
    }
}
