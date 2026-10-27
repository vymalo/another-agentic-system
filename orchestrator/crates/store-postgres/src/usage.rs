//! The usage ledger of the people over Postgres (ADR 0065, migration `0023`).
//!
//! [`PgStore`] writes it **inside the transaction that appends the event**: `commit` and
//! `create_thread` call [`fold_events`] right after the events are inserted, so the event and the
//! row it changes are committed together or neither is. [`catch_up`] first folds in what the ledger
//! has not been through (`threads.usage_through` behind `last_seq`: events a build without the ledger
//! appended, or the log a thread had before migration `0023`), so the ledger is complete whichever
//! builds wrote and in whatever order. Erasing a thread keeps its rows and clears their `thread_id`
//! in the same transaction as the delete. The reads and the backfill are the [`UsageLedger`] port.
//!
//! The semantics of a row (a total replaces, a call adds while there is no total, an event already
//! folded in changes nothing) are `orch_core::UsageRow::fold`, shared with the in-memory store. Many
//! events fold into a few rows, so a [`Folder`] loads each task's row **once**, folds every event
//! for that task into it in memory, and writes each row that changed **once**: the statements of a
//! catch-up are a function of the tasks it touches and the pages of the log it reads, not of its
//! events (one load, fold and store per event made a thread of 10 000 events a transaction of
//! tens of seconds under the thread's lock).

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::time::Instant;

use orch_core::{
    ModelTokens, SandboxEntry, SandboxEnvironment, SandboxShape, SandboxUsage, ThreadId,
    TokenCounts, UnmeasuredSandbox, UsageChange, UsageMonth, UsageMonthTotals, UsageRow,
    UsageSource, UsageTaskKey, UserId, usage_change, usage_counted_through,
};
use orch_ports::{StoreError, UsageBackfill, UsageLedger};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

use crate::codec::{enum_str, event_from_row, parse_enum};
use crate::error::store_err;
use crate::store::PgStore;

/// How many events of the log one read of a catch-up takes.
const CATCH_UP_PAGE: i64 = 500;

/// A count as a `bigint`. Counts are capped at `i64::MAX` before they get here.
fn db(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn opt_db(n: Option<u64>) -> Option<i64> {
    n.map(db)
}

fn count(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

fn opt_count(n: Option<i64>) -> Option<u64> {
    n.map(count)
}

/// A sum the database made (`numeric`, read as text), as the largest `u64` it fits.
fn sum(text: &str) -> u64 {
    text.split('.')
        .next()
        .and_then(|whole| whole.parse::<u128>().ok())
        .map_or(0, |n| u64::try_from(n).unwrap_or(u64::MAX))
}

fn opt_sum(text: Option<String>) -> Option<u64> {
    text.as_deref().map(sum)
}

/// The rows of the tasks a transaction folds usage events into. A task's row is loaded the first
/// time an event names it, every event for it is folded in memory by [`UsageRow::fold`] (the rule the
/// in-memory store applies too), and [`Folder::flush`] writes each row that changed once, so the
/// statements are by task and not by event. Tasks are independent, so the order of events across
/// tasks does not matter; the order within a task is the order they were applied in.
#[derive(Default)]
pub(crate) struct Folder {
    tasks: BTreeMap<UsageTaskKey, Task>,
}

/// A task's row as the database holds it and as the events folded so far leave it.
struct Task {
    /// The id and row in the database; none for a task that has no row yet.
    stored: Option<(i64, UsageRow)>,
    /// The row after the events folded so far; none while no event has changed it.
    folded: Option<UsageRow>,
}

impl Folder {
    /// Folds `change` into its task's row, loading the row if this is the first change for it.
    async fn apply(
        &mut self,
        conn: &mut PgConnection,
        change: &UsageChange,
    ) -> Result<(), StoreError> {
        let task = match self.tasks.entry(change.key.clone()) {
            Entry::Occupied(task) => task.into_mut(),
            Entry::Vacant(slot) => slot.insert(Task {
                stored: load(conn, &change.key).await?,
                folded: None,
            }),
        };
        let current = task
            .folded
            .as_ref()
            .or_else(|| task.stored.as_ref().map(|(_, row)| row));
        if let Some(row) = UsageRow::fold(current, change) {
            task.folded = Some(row);
        }
        Ok(())
    }

    /// Writes the rows that changed, one write for each; returns how many.
    async fn flush(self, conn: &mut PgConnection) -> Result<usize, StoreError> {
        let mut written = 0;
        for task in self.tasks.into_values() {
            if let Some(row) = task.folded {
                store(conn, task.stored, &row).await?;
                written += 1;
            }
        }
        Ok(written)
    }
}

/// Applies what the usage events among `events` change, in order. The events are those of a thread
/// of `owner`, already inserted in this transaction.
pub(crate) async fn fold_events(
    conn: &mut PgConnection,
    owner: &UserId,
    events: &[orch_core::Event],
) -> Result<(), StoreError> {
    let mut folder = Folder::default();
    for event in events {
        if let Some(change) = usage_change(owner, event) {
            folder.apply(conn, &change).await?;
        }
    }
    folder.flush(conn).await?;
    Ok(())
}

/// Folds the usage events of `thread` with `after < seq <= to` that the ledger has not been through,
/// oldest first; returns how many it folded. The log is read in pages and folded into the tasks'
/// rows in memory, and each row that changed is written once at the end (see [`Folder`]); one line
/// of the log says the thread, how many events, how many tasks and how long it took (no content).
/// An event that does not decode is left out and logged (its seq and thread, never its content):
/// the ledger is not a reason for a conversation to stop, and a log that cannot be read is a fault
/// of its own.
pub(crate) async fn catch_up(
    conn: &mut PgConnection,
    thread: ThreadId,
    owner: &UserId,
    after: i64,
    to: i64,
) -> Result<u64, StoreError> {
    let started = Instant::now();
    let mut folder = Folder::default();
    let mut cursor = after;
    let mut folded = 0_u64;
    while cursor < to {
        let rows = sqlx::query(
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = $1 AND seq > $2 AND seq <= $3 \
             AND kind IN ('model_usage', 'model_usage_total') ORDER BY seq LIMIT $4",
        )
        .bind(thread.0)
        .bind(cursor)
        .bind(to)
        .bind(CATCH_UP_PAGE)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_err)?;
        let Some(last) = rows.last() else {
            break;
        };
        cursor = last.try_get("seq").map_err(store_err)?;
        for row in &rows {
            match event_from_row(thread, row) {
                Ok(event) => {
                    if let Some(change) = usage_change(owner, &event) {
                        folder.apply(conn, &change).await?;
                        folded += 1;
                    }
                }
                Err(_) => tracing::warn!(
                    %thread,
                    seq = row.try_get::<i64, _>("seq").unwrap_or(0),
                    "a usage event does not decode and is not counted in the ledger"
                ),
            }
        }
    }
    let tasks = folder.flush(conn).await?;
    if folded > 0 {
        tracing::info!(
            %thread,
            events = folded,
            tasks,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "the usage ledger caught up with the log"
        );
    }
    Ok(folded)
}

/// The rows of `thread` become ones that name no thread: the usage stays, the link goes.
pub(crate) async fn forget_threads(
    conn: &mut PgConnection,
    threads: &[uuid::Uuid],
) -> Result<(), StoreError> {
    sqlx::query("UPDATE usage_tasks SET thread_id = NULL WHERE thread_id = ANY($1)")
        .bind(threads)
        .execute(conn)
        .await
        .map_err(store_err)?;
    Ok(())
}

const TASK_COLUMNS: &str =
    "id, owner, thread_id, agent, task, month, source, seq, sandbox_environment, sandbox_measured";

/// The row of a task of a thread that exists, with its id.
async fn load(
    conn: &mut PgConnection,
    key: &UsageTaskKey,
) -> Result<Option<(i64, UsageRow)>, StoreError> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT {TASK_COLUMNS} FROM usage_tasks WHERE thread_id = $1 AND agent = $2 AND task = $3"
    )))
    .bind(key.thread.0)
    .bind(&key.agent)
    .bind(&key.task)
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_err)?;
    match row {
        Some(row) => Ok(Some(with_children(conn, &row).await?)),
        None => Ok(None),
    }
}

/// A task row with its lines and sandbox entries.
async fn with_children(
    conn: &mut PgConnection,
    row: &PgRow,
) -> Result<(i64, UsageRow), StoreError> {
    let id: i64 = row.try_get("id").map_err(store_err)?;
    let lines = sqlx::query(
        "SELECT provider, model, input_tokens, output_tokens, total_tokens, reasoning_tokens, \
         cached_input_tokens, cache_write_input_tokens FROM usage_lines WHERE task_id = $1 \
         ORDER BY provider COLLATE \"C\", model COLLATE \"C\"",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_err)?;
    // only a sandbox that was measured has entries
    let measured: bool = row.try_get("sandbox_measured").map_err(store_err)?;
    let entries = if measured {
        sqlx::query(
            "SELECT cpu_millis, memory_bytes, commands, milliseconds FROM usage_sandbox \
             WHERE task_id = $1 ORDER BY ord",
        )
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_err)?
    } else {
        Vec::new()
    };
    Ok((id, to_row(row, &lines, &entries)?))
}

fn to_row(task: &PgRow, lines: &[PgRow], entries: &[PgRow]) -> Result<UsageRow, StoreError> {
    let get_err = store_err;
    let source: String = task.try_get("source").map_err(get_err)?;
    let month: String = task.try_get("month").map_err(get_err)?;
    let environment: Option<String> = task.try_get("sandbox_environment").map_err(get_err)?;
    let measured: bool = task.try_get("sandbox_measured").map_err(get_err)?;
    let sandbox = match environment {
        None => None,
        Some(word) => {
            let environment: SandboxEnvironment = parse_enum("sandbox environment", &word)?;
            let usage = if measured {
                Some(
                    entries
                        .iter()
                        .map(|e| {
                            Ok(SandboxEntry {
                                cpu_millis: opt_count(e.try_get("cpu_millis").map_err(get_err)?),
                                memory_bytes: opt_count(
                                    e.try_get("memory_bytes").map_err(get_err)?,
                                ),
                                commands: count(e.try_get("commands").map_err(get_err)?),
                                milliseconds: count(e.try_get("milliseconds").map_err(get_err)?),
                            })
                        })
                        .collect::<Result<Vec<_>, StoreError>>()?,
                )
            } else {
                None
            };
            Some(SandboxUsage { environment, usage })
        }
    };
    let tokens = lines
        .iter()
        .map(|l| {
            let provider: String = l.try_get("provider").map_err(get_err)?;
            Ok(ModelTokens {
                provider: Some(provider).filter(|p| !p.is_empty()),
                model: l.try_get("model").map_err(get_err)?,
                tokens: TokenCounts {
                    input_tokens: count(l.try_get("input_tokens").map_err(get_err)?),
                    output_tokens: count(l.try_get("output_tokens").map_err(get_err)?),
                    total_tokens: count(l.try_get("total_tokens").map_err(get_err)?),
                    reasoning_tokens: opt_count(l.try_get("reasoning_tokens").map_err(get_err)?),
                    cached_input_tokens: opt_count(
                        l.try_get("cached_input_tokens").map_err(get_err)?,
                    ),
                    cache_write_input_tokens: opt_count(
                        l.try_get("cache_write_input_tokens").map_err(get_err)?,
                    ),
                },
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(UsageRow {
        owner: UserId::new(&task.try_get::<String, _>("owner").map_err(get_err)?),
        key: UsageTaskKey {
            // a row that names no thread is never loaded by key; the nil id marks one read by `owner`
            thread: ThreadId(
                task.try_get::<Option<uuid::Uuid>, _>("thread_id")
                    .map_err(get_err)?
                    .unwrap_or_default(),
            ),
            agent: task.try_get("agent").map_err(get_err)?,
            task: task.try_get("task").map_err(get_err)?,
        },
        month: month
            .parse::<UsageMonth>()
            .map_err(|e| StoreError::corrupt_with("a usage row has a month that is not one", e))?,
        source: UsageSource::from_word(&source)
            .ok_or_else(|| StoreError::corrupt("a usage row has a source that is not one"))?,
        seq: task.try_get("seq").map_err(get_err)?,
        tokens,
        sandbox,
    })
}

/// Writes `row`: a new task, or the task `stored` replaced. Only what differs from `stored` is
/// written: the lines whose counts changed (an upsert), the lines that disappeared (a total can name
/// other models than the calls it replaces), and the sandbox entries when they changed.
async fn store(
    conn: &mut PgConnection,
    stored: Option<(i64, UsageRow)>,
    row: &UsageRow,
) -> Result<(), StoreError> {
    let (environment, measured) = match &row.sandbox {
        None => (None, false),
        Some(sandbox) => (
            Some(enum_str(&sandbox.environment)?),
            sandbox.usage.is_some(),
        ),
    };
    let (id, before) = match stored {
        Some((id, before)) => {
            sqlx::query(
                "UPDATE usage_tasks SET month = $2, source = $3, seq = $4, \
                 sandbox_environment = $5, sandbox_measured = $6 WHERE id = $1",
            )
            .bind(id)
            .bind(row.month.to_string())
            .bind(row.source.as_str())
            .bind(row.seq)
            .bind(environment.as_deref())
            .bind(measured)
            .execute(&mut *conn)
            .await
            .map_err(store_err)?;
            (id, Some(before))
        }
        None => {
            let id = sqlx::query_scalar(
                "INSERT INTO usage_tasks (owner, thread_id, agent, task, month, source, seq, \
                 sandbox_environment, sandbox_measured) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
            )
            .bind(row.owner.as_str())
            .bind(row.key.thread.0)
            .bind(&row.key.agent)
            .bind(&row.key.task)
            .bind(row.month.to_string())
            .bind(row.source.as_str())
            .bind(row.seq)
            .bind(environment.as_deref())
            .bind(measured)
            .fetch_one(&mut *conn)
            .await
            .map_err(store_err)?;
            (id, None)
        }
    };
    let before_lines = before.as_ref().map_or(&[][..], |b| b.tokens.as_slice());
    for line in row.tokens.iter().filter(|l| !before_lines.contains(l)) {
        let t = &line.tokens;
        sqlx::query(
            "INSERT INTO usage_lines (task_id, provider, model, input_tokens, output_tokens, \
             total_tokens, reasoning_tokens, cached_input_tokens, cache_write_input_tokens) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (task_id, provider, model) DO UPDATE SET \
             input_tokens = EXCLUDED.input_tokens, output_tokens = EXCLUDED.output_tokens, \
             total_tokens = EXCLUDED.total_tokens, reasoning_tokens = EXCLUDED.reasoning_tokens, \
             cached_input_tokens = EXCLUDED.cached_input_tokens, \
             cache_write_input_tokens = EXCLUDED.cache_write_input_tokens",
        )
        .bind(id)
        .bind(line.provider.as_deref().unwrap_or(""))
        .bind(&line.model)
        .bind(db(t.input_tokens))
        .bind(db(t.output_tokens))
        .bind(db(t.total_tokens))
        .bind(opt_db(t.reasoning_tokens))
        .bind(opt_db(t.cached_input_tokens))
        .bind(opt_db(t.cache_write_input_tokens))
        .execute(&mut *conn)
        .await
        .map_err(store_err)?;
    }
    // a total replaces the whole row's lines: the models it does not name are gone
    let (gone_providers, gone_models): (Vec<&str>, Vec<&str>) = before_lines
        .iter()
        .filter(|old| {
            !row.tokens
                .iter()
                .any(|l| l.provider == old.provider && l.model == old.model)
        })
        .map(|old| (old.provider.as_deref().unwrap_or(""), old.model.as_str()))
        .unzip();
    if !gone_models.is_empty() {
        sqlx::query(
            "DELETE FROM usage_lines WHERE task_id = $1 AND (provider, model) IN \
             (SELECT * FROM unnest($2::text[], $3::text[]))",
        )
        .bind(id)
        .bind(&gone_providers)
        .bind(&gone_models)
        .execute(&mut *conn)
        .await
        .map_err(store_err)?;
    }
    let entries = sandbox_entries(row);
    if before.as_ref().map(sandbox_entries).unwrap_or_default() != entries {
        sqlx::query("DELETE FROM usage_sandbox WHERE task_id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await
            .map_err(store_err)?;
        for (ord, entry) in (0_i16..).zip(entries) {
            sqlx::query(
                "INSERT INTO usage_sandbox (task_id, ord, cpu_millis, memory_bytes, commands, \
                 milliseconds) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(ord)
            .bind(opt_db(entry.cpu_millis))
            .bind(opt_db(entry.memory_bytes))
            .bind(db(entry.commands))
            .bind(db(entry.milliseconds))
            .execute(&mut *conn)
            .await
            .map_err(store_err)?;
        }
    }
    Ok(())
}

/// The sandbox entries a row keeps in `usage_sandbox`: those of a measured sandbox, none otherwise.
fn sandbox_entries(row: &UsageRow) -> Vec<SandboxEntry> {
    row.sandbox
        .as_ref()
        .and_then(|s| s.usage.clone())
        .unwrap_or_default()
}

impl UsageLedger for PgStore {
    async fn month(
        &self,
        owner: &UserId,
        month: UsageMonth,
    ) -> Result<UsageMonthTotals, StoreError> {
        // one snapshot for the four reads, so a commit between them cannot make them disagree
        let mut tx = self.pool().begin().await.map_err(store_err)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let who = owner.as_str();
        let when = month.to_string();
        let tasks: i64 =
            sqlx::query_scalar("SELECT count(*) FROM usage_tasks WHERE owner = $1 AND month = $2")
                .bind(who)
                .bind(&when)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
        let lines = sqlx::query(
            "SELECT l.provider, l.model, \
             SUM(l.input_tokens)::text AS input_tokens, SUM(l.output_tokens)::text AS output_tokens, \
             SUM(l.total_tokens)::text AS total_tokens, SUM(l.reasoning_tokens)::text AS reasoning_tokens, \
             SUM(l.cached_input_tokens)::text AS cached_input_tokens, \
             SUM(l.cache_write_input_tokens)::text AS cache_write_input_tokens \
             FROM usage_tasks t JOIN usage_lines l ON l.task_id = t.id \
             WHERE t.owner = $1 AND t.month = $2 GROUP BY l.provider, l.model",
        )
        .bind(who)
        .bind(&when)
        .fetch_all(&mut *tx)
        .await
        .map_err(store_err)?;
        let shapes = sqlx::query(
            "SELECT t.sandbox_environment AS environment, s.cpu_millis, s.memory_bytes, \
             SUM(s.commands)::text AS commands, SUM(s.milliseconds)::text AS milliseconds \
             FROM usage_tasks t JOIN usage_sandbox s ON s.task_id = t.id \
             WHERE t.owner = $1 AND t.month = $2 GROUP BY t.sandbox_environment, s.cpu_millis, s.memory_bytes",
        )
        .bind(who)
        .bind(&when)
        .fetch_all(&mut *tx)
        .await
        .map_err(store_err)?;
        let unmeasured = sqlx::query(
            "SELECT sandbox_environment AS environment, count(*) AS tasks FROM usage_tasks \
             WHERE owner = $1 AND month = $2 AND sandbox_environment IS NOT NULL \
             AND NOT sandbox_measured GROUP BY sandbox_environment",
        )
        .bind(who)
        .bind(&when)
        .fetch_all(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;

        let models = lines
            .iter()
            .map(|l| {
                let provider: String = l.try_get("provider").map_err(store_err)?;
                let text = |col: &str| l.try_get::<String, _>(col).map_err(store_err);
                let part = |col: &str| l.try_get::<Option<String>, _>(col).map_err(store_err);
                Ok(ModelTokens {
                    provider: Some(provider).filter(|p| !p.is_empty()),
                    model: l.try_get("model").map_err(store_err)?,
                    tokens: TokenCounts {
                        input_tokens: sum(&text("input_tokens")?),
                        output_tokens: sum(&text("output_tokens")?),
                        total_tokens: sum(&text("total_tokens")?),
                        reasoning_tokens: opt_sum(part("reasoning_tokens")?),
                        cached_input_tokens: opt_sum(part("cached_input_tokens")?),
                        cache_write_input_tokens: opt_sum(part("cache_write_input_tokens")?),
                    },
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let sandbox = shapes
            .iter()
            .map(|s| {
                let word: String = s.try_get("environment").map_err(store_err)?;
                Ok(SandboxShape {
                    environment: parse_enum("sandbox environment", &word)?,
                    cpu_millis: opt_count(s.try_get("cpu_millis").map_err(store_err)?),
                    memory_bytes: opt_count(s.try_get("memory_bytes").map_err(store_err)?),
                    commands: sum(&s.try_get::<String, _>("commands").map_err(store_err)?),
                    milliseconds: sum(&s
                        .try_get::<String, _>("milliseconds")
                        .map_err(store_err)?),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let unmeasured = unmeasured
            .iter()
            .map(|u| {
                let word: String = u.try_get("environment").map_err(store_err)?;
                Ok(UnmeasuredSandbox {
                    environment: parse_enum("sandbox environment", &word)?,
                    tasks: count(u.try_get("tasks").map_err(store_err)?),
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(UsageMonthTotals::new(
            month,
            count(tasks),
            models,
            sandbox,
            unmeasured,
        ))
    }

    async fn thread_rows(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let mut conn = self.pool().acquire().await.map_err(store_err)?;
        let tasks = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TASK_COLUMNS} FROM usage_tasks WHERE owner = $1 AND thread_id = $2 \
             ORDER BY agent COLLATE \"C\", task COLLATE \"C\""
        )))
        .bind(owner.as_str())
        .bind(thread.0)
        .fetch_all(&mut *conn)
        .await
        .map_err(store_err)?;
        let mut rows = Vec::with_capacity(tasks.len());
        for task in &tasks {
            rows.push(with_children(&mut conn, task).await?.1);
        }
        Ok(rows)
    }

    async fn backfill(
        &self,
        after: Option<ThreadId>,
        limit: u32,
    ) -> Result<UsageBackfill, StoreError> {
        let limit = limit.max(1);
        // `GREATEST(usage_through, COALESCE(forked_at, 0))` is `orch_core::usage_counted_through` in
        // SQL: the page is chosen with it and each thread is judged again with the Rust function
        // under the thread's lock, so the two must say the same.
        let due: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM threads WHERE ($1::uuid IS NULL OR id > $1) \
             AND GREATEST(usage_through, COALESCE(forked_at, 0)) < last_seq ORDER BY id LIMIT $2",
        )
        .bind(after.map(|t| t.0))
        .bind(i64::from(limit))
        .fetch_all(self.pool())
        .await
        .map_err(store_err)?;
        let mut page = UsageBackfill {
            next: (due.len() == limit as usize)
                .then(|| due.last().copied().map(ThreadId))
                .flatten(),
            ..UsageBackfill::default()
        };
        for id in due {
            let mut tx = self.pool().begin().await.map_err(store_err)?;
            // the lock every commit of the thread takes: the page and the service can run together
            let locked = sqlx::query(
                "SELECT owner, last_seq, usage_through, forked_at FROM threads \
                 WHERE id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_err)?;
            let Some(locked) = locked else {
                continue; // erased since
            };
            let owner = UserId::new(&locked.try_get::<String, _>("owner").map_err(store_err)?);
            let last_seq: i64 = locked.try_get("last_seq").map_err(store_err)?;
            let through: i64 = locked.try_get("usage_through").map_err(store_err)?;
            let cut: Option<i64> = locked.try_get("forked_at").map_err(store_err)?;
            let from = usage_counted_through(through, cut);
            if from >= last_seq {
                continue; // a commit caught it up since the page was read
            }
            let folded = catch_up(&mut tx, ThreadId(id), &owner, from, last_seq).await?;
            sqlx::query("UPDATE threads SET usage_through = $2 WHERE id = $1")
                .bind(id)
                .bind(last_seq)
                .execute(&mut *tx)
                .await
                .map_err(store_err)?;
            tx.commit().await.map_err(store_err)?;
            page.threads += 1;
            page.events += folded;
        }
        Ok(page)
    }
}
