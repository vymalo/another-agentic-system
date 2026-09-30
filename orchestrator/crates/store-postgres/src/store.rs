//! `ThreadStore` over Postgres.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{Event, EventKind, Job, ThreadId, ThreadRecord, UserId, WatchKey};
use orch_ports::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, InboxFinal, InboxId, InboxItem, InboxLease,
    Lease, NewEvent, NewInbox, NewOutbox, NewThreadRecord, NewTimer, OutboxFinal, OutboxId,
    OutboxItem, OutboxStats, Parking, Received, StoreError, TIMER_SOURCE, ThreadStore,
};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::codec::{
    binding_from_row, enum_str, event_from_row, get_ts_opt, inbox_cols, inbox_from_row,
    outbox_cols, outbox_from_row, thread_cols, thread_from_row, to_db, ts,
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

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(t)
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
        "UPDATE a2a_bindings SET task_id = COALESCE($2, task_id), \
         task_state = COALESCE($3, task_state), revision = COALESCE($4, revision), \
         updated_at = $5 WHERE thread_id = $1",
    )
    .bind(thread.0)
    .bind(update.task_id.as_deref())
    .bind(task_state)
    .bind(update.revision.as_deref())
    .bind(to_db(now))
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
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        // Creation is version 1 whatever the first commit does.
        let inserted = sqlx::query(
            "INSERT INTO threads (id, owner, title, agent_id, release, state, job, version, \
             last_seq, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 0, $8, $8)",
        )
        .bind(new.id.0)
        .bind(new.owner.as_str())
        .bind(&new.title)
        .bind(new.target.agent_id.as_str())
        .bind(new.target.release.as_deref())
        .bind(enum_str(&first.new_state)?)
        .bind(job_json(first.job.as_ref().unwrap_or(&Job::default()))?)
        .bind(to_db(new.now))
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
        .bind(&new.context_id)
        .bind(to_db(new.now))
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        if let Some(update) = &first.binding {
            update_binding(&mut tx, new.id, update, first.now).await?;
        }
        let has_outbox = !first.outbox.is_empty();
        let events = insert_events(&mut tx, new.id, 1, first.events).await?;
        insert_outbox(&mut tx, new.id, first.outbox, first.now).await?;
        let rearmed =
            insert_watches_and_timers(&mut tx, new.id, &first.watches, first.timers, first.now)
                .await?;
        let last_seq = i64::try_from(events.len()).unwrap_or(i64::MAX);
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

    async fn list_threads(
        &self,
        owner: &UserId,
        before: Option<ThreadId>,
        limit: u32,
    ) -> Result<Vec<ThreadRecord>, StoreError> {
        if let Some(cursor) = before {
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
             ORDER BY id DESC LIMIT $3"
        ))
        .bind(owner.as_str())
        .bind(before.map(|b| b.0))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?
        .iter()
        .map(thread_from_row)
        .collect()
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
        let has_outbox = !commit.outbox.is_empty();
        let events = insert_events(&mut tx, thread, last_seq + 1, commit.events).await?;
        let new_last_seq = last_seq + i64::try_from(events.len()).unwrap_or(0);
        // The job is written with the state, under the same lock and version check; a commit
        // without one leaves the stored job alone.
        let job = commit.job.as_ref().map(job_json).transpose()?;
        let row = sqlx::query(concat!(
            "UPDATE threads SET state = $2, job = COALESCE($5, job), version = version + 1, \
             last_seq = $3, updated_at = $4 WHERE id = $1 RETURNING ",
            thread_cols!()
        ))
        .bind(thread.0)
        .bind(enum_str(&commit.new_state)?)
        .bind(new_last_seq)
        .bind(to_db(commit.now))
        .bind(job)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        let record = thread_from_row(&row)?;
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
        // or due time says), which also covers an older row this very statement claims.
        let mut rows: Vec<(i64, PgRow)> = sqlx::query(concat!(
            "WITH c AS ( \
               SELECT o.id FROM outbox o \
               WHERE ((o.status = 'pending' AND o.next_attempt_at <= $1) \
                   OR (o.status = 'inflight' AND o.lease_until <= $1)) \
                 AND (o.kind = 'cancel' OR NOT EXISTS ( \
                       SELECT 1 FROM outbox p \
                       WHERE p.thread_id = o.thread_id AND p.kind = 'delegate' \
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
        let (status, error) = match outcome {
            OutboxFinal::Delivered => ("delivered", None),
            OutboxFinal::Dead { error } => ("dead", Some(error)),
            OutboxFinal::Skipped => ("skipped", None),
        };
        let done = sqlx::query(
            "UPDATE outbox SET status = $4, last_error = COALESCE($5, last_error), \
             lease_owner = NULL, lease_until = NULL, updated_at = $6 \
             WHERE id = $1 AND lease_owner = $2 AND attempts = $3 AND status = 'inflight'",
        )
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

    async fn skip_unsent_delegates(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<u32, StoreError> {
        sqlx::query(
            "UPDATE outbox SET status = 'skipped', lease_owner = NULL, lease_until = NULL, \
             updated_at = $2 \
             WHERE thread_id = $1 AND kind = 'delegate' AND sent_at IS NULL \
               AND (status = 'pending' OR (status = 'inflight' AND lease_until < $2))",
        )
        .bind(thread.0)
        .bind(to_db(now))
        .execute(&self.pool)
        .await
        .map(|r| u32::try_from(r.rows_affected()).unwrap_or(u32::MAX))
        .map_err(store_err)
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
