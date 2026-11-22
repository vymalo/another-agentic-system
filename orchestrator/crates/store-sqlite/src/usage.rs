//! The usage ledger of the people over SQLite (ADR 0065, migration `0002`).
//!
//! [`SqliteStore`] writes it **inside the transaction that appends the event**: `commit`,
//! `create_thread` and `fork_thread` call [`fold_events`] right after the events are inserted, so
//! an event and the row it changes are committed together or neither is. [`catch_up`] first folds
//! in what the ledger has not been through (`threads.usage_through` behind `last_seq`: events a
//! build without the ledger appended), so the ledger is complete whichever builds wrote and in
//! whatever order. Erasing a thread keeps its rows and clears their `thread_id` in the same
//! transaction as the delete ([`forget_threads`]). The reads and the backfill are the
//! [`UsageLedger`] port.
//!
//! The semantics of a row (a total replaces, a call adds while there is no total, an event already
//! folded in changes nothing) are `orch_core::UsageRow::fold`, shared with the Postgres and the
//! in-memory stores. Many events fold into a few rows, so a [`Folder`] loads each task's row
//! **once**, folds every event for that task into it in memory, and writes each row that changed
//! **once**: the statements of a catch-up are a function of the tasks it touches and the pages of
//! the log it reads, not of its events.
//!
//! Two differences from `orch-store-postgres/src/usage.rs`, both because this store is one file
//! with one writer:
//!
//! * a write replaces a task's lines and sandbox entries whole instead of diffing them (the
//!   children are a handful of rows, and there is no second writer to lose a row to), while the
//!   property that matters is kept: one load and one write per task, not per event;
//! * a backfill never waits for a thread's lock, because nothing else can hold it: `skipped` is
//!   always 0 here.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use orch_core::{
    Event, ModelTokens, SandboxEntry, SandboxUsage, ThreadId, TokenCounts, UsageChange, UsageMonth,
    UsageMonthTotals, UsageRow, UsageSource, UsageTaskKey, UserId, usage_change,
    usage_counted_through,
};
use orch_ports::{StoreError, UsageBackfill, UsageLedger};
use rusqlite::{Connection, Row, ToSql};

use crate::SqliteStore;
use crate::codec::{event_from_row, parse_enum};
use crate::error::store_err;
use crate::store::{
    dynamic_exec, exec, parse_id, placeholders, row_opt, rows, scalar, texts, write,
};

/// How many events of the log one read of a catch-up takes.
const CATCH_UP_PAGE: i64 = 500;

/// The columns of `usage_tasks`, in the order [`task_from_row`] reads them.
const TASK_COLUMNS: &str = "id, owner, thread_id, agent, task, month, source, seq, \
     sandbox_environment, sandbox_measured";

/// A count as the `INTEGER` SQLite keeps. Counts are capped at `i64::MAX` before they get here.
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

/// A task's row of `usage_tasks` with its id, before its lines and sandbox entries are read.
struct TaskRow {
    id: i64,
    row: UsageRow,
    measured: bool,
}

/// The rows of the tasks a transaction folds usage events into. A task's row is loaded the first
/// time an event names it, every event for it is folded in memory by [`UsageRow::fold`] (the rule
/// every store applies), and [`Folder::flush`] writes each row that changed once.
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
    fn apply(&mut self, conn: &Connection, change: &UsageChange) -> Result<(), StoreError> {
        let task = match self.tasks.entry(change.key.clone()) {
            Entry::Occupied(task) => task.into_mut(),
            Entry::Vacant(slot) => slot.insert(Task {
                stored: load(conn, &change.key)?,
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
    fn flush(self, conn: &Connection) -> Result<usize, StoreError> {
        let mut written = 0;
        for task in self.tasks.into_values() {
            if let Some(row) = task.folded {
                store(conn, task.stored, &row)?;
                written += 1;
            }
        }
        Ok(written)
    }
}

/// Applies what the usage events among `events` change, in order. The events are those of a thread
/// of `owner`, already inserted in this transaction.
pub(crate) fn fold_events(
    conn: &Connection,
    owner: &UserId,
    events: &[Event],
) -> Result<(), StoreError> {
    let mut folder = Folder::default();
    for event in events {
        if let Some(change) = usage_change(owner, event) {
            folder.apply(conn, &change)?;
        }
    }
    folder.flush(conn)?;
    Ok(())
}

/// Folds the usage events of `thread` with `after < seq <= to` that the ledger has not been
/// through, oldest first; returns how many it folded. The log is read in pages and folded into the
/// tasks' rows in memory, and each row that changed is written once (see [`Folder`]). An event
/// that does not decode is left out and logged (its seq and thread, never its content): the ledger
/// is not a reason for a conversation to stop.
pub(crate) fn catch_up(
    conn: &Connection,
    thread: ThreadId,
    owner: &UserId,
    after: i64,
    to: i64,
) -> Result<u64, StoreError> {
    let thread_text = thread.0.to_string();
    let mut folder = Folder::default();
    let mut cursor = after;
    let mut folded = 0_u64;
    while cursor < to {
        // A row that does not decode is kept as an error so the page still moves the cursor past it.
        let batch: Vec<(i64, Result<Event, StoreError>)> = rows(
            conn,
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = ?1 AND seq > ?2 AND seq <= ?3 \
             AND kind IN ('model_usage', 'model_usage_total') ORDER BY seq LIMIT ?4",
            &[&thread_text, &cursor, &to, &CATCH_UP_PAGE],
            |row| {
                let seq: i64 = row.get(0).map_err(store_err)?;
                Ok((seq, event_from_row(thread, row)))
            },
        )?;
        let Some((last, _)) = batch.last() else {
            break;
        };
        cursor = *last;
        for (seq, event) in batch {
            match event {
                Ok(event) => {
                    if let Some(change) = usage_change(owner, &event) {
                        folder.apply(conn, &change)?;
                        folded += 1;
                    }
                }
                Err(_) => tracing::warn!(
                    %thread,
                    seq,
                    "a usage event does not decode and is not counted in the ledger"
                ),
            }
        }
    }
    let tasks = folder.flush(conn)?;
    if folded > 0 {
        tracing::info!(
            %thread,
            events = folded,
            tasks,
            "the usage ledger caught up with the log"
        );
    }
    Ok(folded)
}

/// The rows of `threads` become ones that name no thread: the usage stays, the link goes.
pub(crate) fn forget_threads(conn: &Connection, threads: &[String]) -> Result<(), StoreError> {
    if threads.is_empty() {
        return Ok(());
    }
    let list = placeholders(threads.len());
    dynamic_exec(
        conn,
        &format!("UPDATE usage_tasks SET thread_id = NULL WHERE thread_id IN ({list})"),
        texts(threads),
    )
    .map(|_| ())
}

/// The row of a task of a thread that exists, with its id.
fn load(conn: &Connection, key: &UsageTaskKey) -> Result<Option<(i64, UsageRow)>, StoreError> {
    let found = row_opt(
        conn,
        &format!(
            "SELECT {TASK_COLUMNS} FROM usage_tasks \
             WHERE thread_id = ?1 AND agent = ?2 AND task = ?3"
        ),
        &[&key.thread.0.to_string(), &key.agent, &key.task],
        task_from_row,
    )?;
    match found {
        Some(mut task) => {
            with_children(conn, &mut task)?;
            Ok(Some((task.id, task.row)))
        }
        None => Ok(None),
    }
}

/// A row of [`TASK_COLUMNS`].
fn task_from_row(row: &Row<'_>) -> Result<TaskRow, StoreError> {
    let get = |at: usize| row.get::<_, String>(at).map_err(store_err);
    let thread: Option<String> = row.get(2).map_err(store_err)?;
    let thread = match thread {
        Some(text) => parse_id(&text)?,
        // a row whose thread was erased names none; the nil id marks one read by `owner`
        None => uuid::Uuid::nil(),
    };
    let month: String = get(5)?;
    let source: String = get(6)?;
    let environment: Option<String> = row.get(8).map_err(store_err)?;
    let measured: bool = row.get(9).map_err(store_err)?;
    let sandbox = match environment {
        None => None,
        Some(word) => Some(SandboxUsage {
            environment: parse_enum("sandbox environment", &word)?,
            usage: None,
        }),
    };
    Ok(TaskRow {
        id: row.get(0).map_err(store_err)?,
        row: UsageRow {
            owner: UserId::new(&get(1)?),
            key: UsageTaskKey {
                thread: ThreadId(thread),
                agent: get(3)?,
                task: get(4)?,
            },
            month: month.parse::<UsageMonth>().map_err(|e| {
                StoreError::corrupt_with("a usage row has a month that is not one", e)
            })?,
            source: UsageSource::from_word(&source)
                .ok_or_else(|| StoreError::corrupt("a usage row has a source that is not one"))?,
            seq: row.get(7).map_err(store_err)?,
            tokens: Vec::new(),
            sandbox,
        },
        measured,
    })
}

/// Reads the lines and the sandbox entries of `task`.
fn with_children(conn: &Connection, task: &mut TaskRow) -> Result<(), StoreError> {
    task.row.tokens = rows(
        conn,
        "SELECT provider, model, input_tokens, output_tokens, total_tokens, reasoning_tokens, \
         cached_input_tokens, cache_write_input_tokens FROM usage_lines WHERE task_id = ?1 \
         ORDER BY provider, model",
        &[&task.id],
        |row| {
            let provider: String = row.get(0).map_err(store_err)?;
            Ok(ModelTokens {
                provider: Some(provider).filter(|p| !p.is_empty()),
                model: row.get(1).map_err(store_err)?,
                tokens: TokenCounts {
                    input_tokens: count(row.get(2).map_err(store_err)?),
                    output_tokens: count(row.get(3).map_err(store_err)?),
                    total_tokens: count(row.get(4).map_err(store_err)?),
                    reasoning_tokens: opt_count(row.get(5).map_err(store_err)?),
                    cached_input_tokens: opt_count(row.get(6).map_err(store_err)?),
                    cache_write_input_tokens: opt_count(row.get(7).map_err(store_err)?),
                },
            })
        },
    )?;
    // only a sandbox that was measured has entries; none at all is a measured zero
    if task.measured
        && let Some(sandbox) = &mut task.row.sandbox
    {
        sandbox.usage = Some(rows(
            conn,
            "SELECT cpu_millis, memory_bytes, commands, milliseconds FROM usage_sandbox \
             WHERE task_id = ?1 ORDER BY ord",
            &[&task.id],
            |row| {
                Ok(SandboxEntry {
                    cpu_millis: opt_count(row.get(0).map_err(store_err)?),
                    memory_bytes: opt_count(row.get(1).map_err(store_err)?),
                    commands: count(row.get(2).map_err(store_err)?),
                    milliseconds: count(row.get(3).map_err(store_err)?),
                })
            },
        )?);
    }
    Ok(())
}

/// The rows of `sql`, each with its lines and sandbox entries.
fn task_rows(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<UsageRow>, StoreError> {
    let mut out = Vec::new();
    for mut task in rows(conn, sql, params, task_from_row)? {
        with_children(conn, &mut task)?;
        out.push(task.row);
    }
    Ok(out)
}

/// Writes `row`: a new task, or the task `stored` replaced. The task's lines and sandbox entries
/// are replaced whole (see the module doc); a total that names fewer models than the calls it
/// replaces therefore leaves none of the others behind.
fn store(
    conn: &Connection,
    stored: Option<(i64, UsageRow)>,
    row: &UsageRow,
) -> Result<(), StoreError> {
    let (environment, measured) = match &row.sandbox {
        None => (None, false),
        Some(sandbox) => (Some(sandbox.environment.as_str()), sandbox.usage.is_some()),
    };
    let month = row.month.to_string();
    let thread = row.key.thread.0.to_string();
    let id = match stored {
        Some((id, _)) => {
            exec(
                conn,
                "UPDATE usage_tasks SET month = ?2, source = ?3, seq = ?4, \
                 sandbox_environment = ?5, sandbox_measured = ?6 WHERE id = ?1",
                &[
                    &id,
                    &month,
                    &row.source.as_str(),
                    &row.seq,
                    &environment,
                    &measured,
                ],
            )?;
            id
        }
        None => scalar(
            conn,
            "INSERT INTO usage_tasks (owner, thread_id, agent, task, month, source, seq, \
             sandbox_environment, sandbox_measured) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
            &[
                &row.owner.as_str(),
                &thread,
                &row.key.agent,
                &row.key.task,
                &month,
                &row.source.as_str(),
                &row.seq,
                &environment,
                &measured,
            ],
        )?,
    };
    exec(conn, "DELETE FROM usage_lines WHERE task_id = ?1", &[&id])?;
    for line in &row.tokens {
        let tokens = &line.tokens;
        exec(
            conn,
            "INSERT INTO usage_lines (task_id, provider, model, input_tokens, output_tokens, \
             total_tokens, reasoning_tokens, cached_input_tokens, cache_write_input_tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            &[
                &id,
                &line.provider.as_deref().unwrap_or(""),
                &line.model,
                &db(tokens.input_tokens),
                &db(tokens.output_tokens),
                &db(tokens.total_tokens),
                &opt_db(tokens.reasoning_tokens),
                &opt_db(tokens.cached_input_tokens),
                &opt_db(tokens.cache_write_input_tokens),
            ],
        )?;
    }
    exec(conn, "DELETE FROM usage_sandbox WHERE task_id = ?1", &[&id])?;
    for (ord, entry) in (0_i64..).zip(row.sandbox.iter().flat_map(|s| s.usage.iter().flatten())) {
        exec(
            conn,
            "INSERT INTO usage_sandbox (task_id, ord, cpu_millis, memory_bytes, commands, \
             milliseconds) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            &[
                &id,
                &ord,
                &opt_db(entry.cpu_millis),
                &opt_db(entry.memory_bytes),
                &db(entry.commands),
                &db(entry.milliseconds),
            ],
        )?;
    }
    Ok(())
}

impl UsageLedger for SqliteStore {
    async fn month(
        &self,
        owner: &UserId,
        month: UsageMonth,
    ) -> Result<UsageMonthTotals, StoreError> {
        let owner = owner.as_str().to_owned();
        let when = month.to_string();
        self.call(move |conn| {
            // The sums are `UsageMonthTotals::of`, the function the in-memory store uses, so the two
            // cannot disagree on the parts, the sandbox shapes or the order. SQLite's own `sum` would
            // raise on an integer overflow, and the counts are the core's to cap.
            let rows = task_rows(
                conn,
                &format!("SELECT {TASK_COLUMNS} FROM usage_tasks WHERE owner = ?1 AND month = ?2"),
                &[&owner, &when],
            )?;
            Ok(UsageMonthTotals::of(month, rows.iter()))
        })
        .await
    }

    async fn thread_rows(
        &self,
        owner: &UserId,
        thread: ThreadId,
    ) -> Result<Vec<UsageRow>, StoreError> {
        let owner = owner.as_str().to_owned();
        let thread = thread.0.to_string();
        self.call(move |conn| {
            task_rows(
                conn,
                &format!(
                    "SELECT {TASK_COLUMNS} FROM usage_tasks WHERE owner = ?1 AND thread_id = ?2 \
                     ORDER BY agent, task"
                ),
                &[&owner, &thread],
            )
        })
        .await
    }

    async fn backfill(
        &self,
        after: Option<ThreadId>,
        limit: u32,
    ) -> Result<UsageBackfill, StoreError> {
        let limit = limit.max(1);
        self.call(move |conn| {
            let mut page = UsageBackfill::default();
            if after.is_none() {
                // rows that name a thread an older build erased: that build deleted the thread and
                // knew nothing of the ledger's `thread_id`
                page.detached = u64::try_from(exec(
                    conn,
                    "UPDATE usage_tasks SET thread_id = NULL WHERE thread_id IS NOT NULL \
                     AND NOT EXISTS (SELECT 1 FROM threads t WHERE t.id = usage_tasks.thread_id)",
                    &[],
                )?)
                .unwrap_or(0);
            }
            // `max(usage_through, COALESCE(forked_at, 0))` is `orch_core::usage_counted_through`
            // in SQL: the page is chosen with it and each thread is judged again with the Rust
            // function inside its transaction, so the two must say the same.
            let after = after.map(|t| t.0.to_string());
            let due: Vec<String> = rows(
                conn,
                "SELECT id FROM threads WHERE (?1 IS NULL OR id > ?1) \
                 AND max(usage_through, COALESCE(forked_at, 0)) < last_seq ORDER BY id LIMIT ?2",
                &[&after.as_deref(), &i64::from(limit)],
                |row| row.get(0).map_err(store_err),
            )?;
            page.next = next_page(&due, limit)?;
            for id in due {
                let thread = ThreadId(parse_id(&id)?);
                let folded = write(conn, |tx| {
                    let locked = row_opt(
                        tx,
                        "SELECT owner, last_seq, usage_through, forked_at FROM threads \
                         WHERE id = ?1",
                        &[&id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0).map_err(store_err)?,
                                row.get::<_, i64>(1).map_err(store_err)?,
                                row.get::<_, i64>(2).map_err(store_err)?,
                                row.get::<_, Option<i64>>(3).map_err(store_err)?,
                            ))
                        },
                    )?;
                    let Some((owner, last_seq, through, cut)) = locked else {
                        return Ok(None); // erased since
                    };
                    let from = usage_counted_through(through, cut);
                    if from >= last_seq {
                        return Ok(None); // a commit caught it up since the page was read
                    }
                    let folded = catch_up(tx, thread, &UserId::new(&owner), from, last_seq)?;
                    exec(
                        tx,
                        "UPDATE threads SET usage_through = ?2 WHERE id = ?1",
                        &[&id, &last_seq],
                    )?;
                    Ok(Some(folded))
                })?;
                if let Some(folded) = folded {
                    page.threads += 1;
                    page.events += folded;
                }
            }
            Ok(page)
        })
        .await
    }

    async fn sweep(&self, before: UsageMonth, limit: u32) -> Result<u64, StoreError> {
        let before = before.to_string();
        let limit = i64::from(limit.max(1));
        self.call(move |conn| {
            // One statement: the lines and the sandbox entries go with their task (`ON DELETE
            // CASCADE`). The oldest months first, so a worker can loop in batches; one writer, so
            // there is no `SKIP LOCKED` to skip a task a commit is updating.
            let removed = write(conn, |tx| {
                exec(
                    tx,
                    "DELETE FROM usage_tasks WHERE id IN (SELECT id FROM usage_tasks \
                     WHERE month < ?1 ORDER BY month, id LIMIT ?2)",
                    &[&before, &limit],
                )
            })?;
            Ok(u64::try_from(removed).unwrap_or(0))
        })
        .await
    }
}

/// Where the next page of a backfill starts: the last thread of a page that was full.
fn next_page(due: &[String], limit: u32) -> Result<Option<ThreadId>, StoreError> {
    let full = usize::try_from(limit).is_ok_and(|n| due.len() == n);
    match due.last().filter(|_| full) {
        Some(id) => Ok(Some(ThreadId(parse_id(id)?))),
        None => Ok(None),
    }
}
