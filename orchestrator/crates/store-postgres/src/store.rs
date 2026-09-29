//! `ThreadStore` over Postgres.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{Event, ThreadId, ThreadRecord, UserId};
use orch_ports::{
    AgentBinding, BindingUpdate, Commit, CommitOutcome, NewEvent, NewOutbox, NewThreadRecord,
    OutboxFinal, OutboxId, OutboxItem, StoreError, ThreadStore,
};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Postgres, Row, Transaction};

use crate::codec::{
    binding_from_row, enum_str, event_from_row, outbox_cols, outbox_from_row, thread_cols,
    thread_from_row, to_db, ts,
};
use crate::error::{is_unique_violation, store_err};
use crate::wakeup::{CHANNEL_OUTBOX, CHANNEL_THREAD};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Default pool size of [`PgStore::connect`]; one connection is held by the wakeup listener.
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// [`ThreadStore`] on Postgres (sqlx). Cheap to clone: clones share the pool.
///
/// Every write that other processes should react to also sends a `NOTIFY` inside its own
/// transaction (delivered only if it commits): `orch_thread` with the thread id after a
/// commit, `orch_outbox` after new or freed outbox rows. Pair it with
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
    /// [`StoreError::Unavailable`] if the database rejects a migration or is unreachable;
    /// [`StoreError::Corrupt`] if an applied migration was edited afterwards.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        MIGRATOR.run(&self.pool).await.map_err(|e| match e {
            sqlx::migrate::MigrateError::Execute(e) => store_err(e),
            other => StoreError::Corrupt(format!("migration: {other}")),
        })
    }
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(t)
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
        let actor =
            serde_json::to_value(&new.actor).map_err(|e| StoreError::Corrupt(e.to_string()))?;
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
                StoreError::Corrupt("the same idempotency key twice in one commit".to_owned())
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
        let payload =
            serde_json::to_value(&row.payload).map_err(|e| StoreError::Corrupt(e.to_string()))?;
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
            "INSERT INTO threads (id, owner, title, agent_id, release, state, version, last_seq, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, 1, 0, $7, $7)",
        )
        .bind(new.id.0)
        .bind(new.owner.as_str())
        .bind(&new.title)
        .bind(new.target.agent_id.as_str())
        .bind(new.target.release.as_deref())
        .bind(enum_str(&first.new_state)?)
        .bind(to_db(new.now))
        .execute(&mut *tx)
        .await;
        if let Err(e) = inserted {
            return Err(if is_unique_violation(&e, None) {
                StoreError::Corrupt("thread id already exists".to_owned())
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
        let locked = sqlx::query("SELECT version, last_seq FROM threads WHERE id = $1 FOR UPDATE")
            .bind(thread.0)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
        let Some(locked) = locked else {
            rollback(tx).await;
            return Err(StoreError::NotFound);
        };
        let version: i64 = locked.try_get("version").map_err(store_err)?;
        let last_seq: i64 = locked.try_get("last_seq").map_err(store_err)?;
        if version != expected_version {
            rollback(tx).await;
            return Err(StoreError::VersionConflict);
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
        let row = sqlx::query(concat!(
            "UPDATE threads SET state = $2, version = version + 1, last_seq = $3, updated_at = $4 \
             WHERE id = $1 RETURNING ",
            thread_cols!()
        ))
        .bind(thread.0)
        .bind(enum_str(&commit.new_state)?)
        .bind(new_last_seq)
        .bind(to_db(commit.now))
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        let record = thread_from_row(&row)?;
        insert_outbox(&mut tx, thread, commit.outbox, commit.now).await?;
        if let Some(update) = &commit.binding {
            update_binding(&mut tx, thread, update, commit.now).await?;
        }
        notify_thread(&mut tx, thread).await?;
        if has_outbox {
            notify_outbox(&mut *tx).await?;
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

    async fn renew_lease(
        &self,
        id: OutboxId,
        owner: &str,
        until: Timestamp,
    ) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE outbox SET lease_until = $3 \
             WHERE id = $1 AND lease_owner = $2 AND status = 'inflight'",
        )
        .bind(id.0)
        .bind(owner)
        .bind(to_db(until))
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn mark_sent(
        &self,
        id: OutboxId,
        owner: &str,
        binding: BindingUpdate,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut tx = self.pool.begin().await.map_err(store_err)?;
        let sent = sqlx::query(
            "UPDATE outbox SET sent_at = $3, updated_at = $3 \
             WHERE id = $1 AND lease_owner = $2 AND status = 'inflight' RETURNING thread_id",
        )
        .bind(id.0)
        .bind(owner)
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
        id: OutboxId,
        owner: &str,
        next_attempt_at: Timestamp,
        error: String,
    ) -> Result<bool, StoreError> {
        sqlx::query(
            "UPDATE outbox SET status = 'pending', next_attempt_at = $3, lease_owner = NULL, \
             lease_until = NULL, last_error = $4, updated_at = clock_timestamp() \
             WHERE id = $1 AND lease_owner = $2 AND status = 'inflight'",
        )
        .bind(id.0)
        .bind(owner)
        .bind(to_db(next_attempt_at))
        .bind(error)
        .execute(&self.pool)
        .await
        .map(|r| r.rows_affected() == 1)
        .map_err(store_err)
    }

    async fn complete_outbox(
        &self,
        id: OutboxId,
        owner: &str,
        outcome: OutboxFinal,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let (status, error) = match outcome {
            OutboxFinal::Delivered => ("delivered", None),
            OutboxFinal::Dead { error } => ("dead", Some(error)),
            OutboxFinal::Skipped => ("skipped", None),
        };
        let done = sqlx::query(
            "UPDATE outbox SET status = $3, last_error = COALESCE($4, last_error), \
             lease_owner = NULL, lease_until = NULL, updated_at = $5 \
             WHERE id = $1 AND lease_owner = $2 AND status = 'inflight'",
        )
        .bind(id.0)
        .bind(owner)
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
}
