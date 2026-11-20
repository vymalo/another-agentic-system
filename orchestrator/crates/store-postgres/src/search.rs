//! The search index of a person's threads over Postgres (ADR 0068, migration `0025`).
//!
//! [`PgStore`] writes it **inside the transaction that appends the event**: `commit` and
//! `create_thread` call [`index_events`] right after the events are inserted, so an event and its words
//! are committed together or neither is. [`catch_up_before_commit`] first indexes some of what the index has
//! not been through (`threads.search_through` behind `last_seq`: events a build without the index
//! appended, or the log a thread had before migration `0025`), **at most
//! [`COMMIT_CATCH_UP_EVENTS`] events per commit**, and advances the watermark only as far as it got; the
//! backfill finishes the rest. A thread stored under another [`FOLD_VERSION`] is unindexed: its rows are
//! deleted and it starts again from `0`. Erasing a thread removes its words in the same transaction: `message_terms` cascades from
//! `events`, and the title columns are on the row.
//!
//! The rules (what a word is, its key, which events are searched) are `orch_core::search`; this module
//! stores the keys it makes with `array_to_tsvector` (lexemes as given: no parser, no dictionary, no
//! locale) and asks for the keys of a query as `'key':*` joined with `&`. **A query's text never reaches
//! SQL**: only [`SearchTerms`], whose every term is letters and numbers, and it is checked here once more.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use orch_core::search::{
    COMMIT_CATCH_UP_EVENTS, FOLD_VERSION, MAX_MESSAGE_HITS, MessageChange, SearchTerms,
    document_keys, message_change,
};
use orch_core::{Event, ThreadId, UserId};
use orch_ports::{
    MatchedMessage, SearchBackfill, SearchRank, SearchRequest, SearchRow, SearchRows, StoreError,
    ThreadSearch,
};
use sqlx::{PgConnection, Row};

use crate::codec::{event_from_row, thread_cols, thread_from_row};
use crate::error::store_err;
use crate::store::{PgStore, archived_name};

/// How many events of the log one read of a catch-up takes.
const CATCH_UP_PAGE: i64 = 500;

/// How long a whole search may run, **all its statements together**, before it is abandoned (a hostile
/// query, a database in trouble): each statement may use only what the ones before it left.
const SEARCH_BUDGET: Duration = Duration::from_secs(2);

/// What is left of a deadline, in whole milliseconds, at least one: `None` once it has passed.
fn remaining_ms(deadline: Instant, now: Instant) -> Option<u64> {
    let left = deadline.checked_duration_since(now)?;
    if left.is_zero() {
        return None;
    }
    Some(u64::try_from(left.as_millis()).unwrap_or(u64::MAX).max(1))
}

/// Gives the next statement of the transaction what is left of the search's budget as its
/// `statement_timeout` (`set_config(.., true)` is `SET LOCAL`: it ends with the transaction, so a pooler
/// in transaction mode and every other statement of the pool are untouched).
async fn arm(tx: &mut PgConnection, deadline: Instant) -> Result<(), StoreError> {
    let Some(ms) = remaining_ms(deadline, Instant::now()) else {
        return Err(StoreError::unavailable(BudgetSpent));
    };
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{ms}ms"))
        .execute(tx)
        .await
        .map_err(search_err)?;
    Ok(())
}

/// The search ran out of its time.
#[derive(Debug, thiserror::Error)]
#[error("the search used up its time")]
struct BudgetSpent;

/// How long the backfill waits for a thread's lock before it leaves the thread for a later run.
const BACKFILL_LOCK_TIMEOUT: &str = "5s";

/// The `tsquery` text of `terms`: `'a':* & 'b':*`. Every term is checked to be letters and numbers
/// only, so it cannot hold a quote, a backslash, an operator or a space; a term that is not is an
/// internal error and never reaches the database.
fn tsquery(terms: &SearchTerms) -> Result<String, StoreError> {
    let checked = SearchTerms::from_keys(terms.as_slice().to_vec())
        .map_err(|e| StoreError::internal(Box::new(e)))?;
    Ok(checked
        .as_slice()
        .iter()
        .map(|t| format!("'{t}':*"))
        .collect::<Vec<_>>()
        .join(" & "))
}

/// Writes what one event changes in the index of messages of `thread`: a later final message deletes
/// the earlier rows of its `messageId`, and the message's words are stored when it has any.
async fn index_change(
    conn: &mut PgConnection,
    thread: ThreadId,
    owner: &UserId,
    seq: i64,
    change: MessageChange<'_>,
) -> Result<bool, StoreError> {
    if let Some(id) = change.replaces {
        sqlx::query(
            "DELETE FROM message_terms WHERE thread_id = $1 AND message_id = $2 AND seq < $3",
        )
        .bind(thread.0)
        .bind(id)
        .bind(seq)
        .execute(&mut *conn)
        .await
        .map_err(store_err)?;
    }
    let Some(text) = change.text else {
        return Ok(false);
    };
    let keys = document_keys(text);
    if keys.is_empty() {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO message_terms (thread_id, seq, owner, message_id, terms) \
         VALUES ($1, $2, $3, $4, array_to_tsvector($5::text[])) \
         ON CONFLICT (thread_id, seq) DO UPDATE SET owner = EXCLUDED.owner, \
         message_id = EXCLUDED.message_id, terms = EXCLUDED.terms",
    )
    .bind(thread.0)
    .bind(seq)
    .bind(owner.as_str())
    .bind(change.replaces)
    .bind(&keys)
    .execute(&mut *conn)
    .await
    .map_err(store_err)?;
    Ok(true)
}

/// Indexes the searched messages among `events`, in order. The events are those of a thread of `owner`,
/// already inserted in this transaction.
pub(crate) async fn index_events(
    conn: &mut PgConnection,
    thread: ThreadId,
    owner: &UserId,
    events: &[Event],
) -> Result<(), StoreError> {
    for event in events {
        if let Some(change) = message_change(event) {
            index_change(conn, thread, owner, event.seq, change).await?;
        }
    }
    Ok(())
}

/// The keys to store for a title or a description, as the column's value: `None` leaves a column as it is.
pub(crate) fn keys_of(text: Option<&str>) -> Vec<String> {
    text.map(document_keys).unwrap_or_default()
}

/// Deletes every row of the index of `thread` (a thread stored under another fold version, or a rebuild).
pub(crate) async fn delete_index(
    conn: &mut PgConnection,
    thread: ThreadId,
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM message_terms WHERE thread_id = $1")
        .bind(thread.0)
        .execute(conn)
        .await
        .map_err(store_err)?;
    Ok(())
}

/// What a commit owes the index of the old log before it indexes its own events: at most
/// [`COMMIT_CATCH_UP_EVENTS`] events after `through`, never past `last_seq` (the thread's last event
/// before the commit). Returns the seq the index is through afterwards: `last_seq` when the old log
/// is done, else exactly where it stopped, so the watermark has no gap and a later commit or the
/// backfill continues from there.
pub(crate) async fn catch_up_before_commit(
    conn: &mut PgConnection,
    thread: ThreadId,
    owner: &UserId,
    through: i64,
    last_seq: i64,
) -> Result<i64, StoreError> {
    let to = last_seq.min(through.saturating_add(COMMIT_CATCH_UP_EVENTS));
    if through < to {
        catch_up(conn, thread, owner, through, to, last_seq).await?;
    }
    Ok(to.max(through))
}

/// The ids, among `ids`, of agent messages that a **later** final message (above `to`, up to `end`)
/// replaces: indexing the earlier one now would only be undone when the catch-up reaches the later.
async fn replaced_later(
    conn: &mut PgConnection,
    thread: ThreadId,
    to: i64,
    end: i64,
    ids: &[String],
) -> Result<HashSet<String>, StoreError> {
    if ids.is_empty() || to >= end {
        return Ok(HashSet::new());
    }
    let found: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT data->>'messageId' FROM events \
         WHERE thread_id = $1 AND seq > $2 AND seq <= $3 AND kind = 'agent_message' \
         AND data->>'final' = 'true' AND data->>'messageId' = ANY($4)",
    )
    .bind(thread.0)
    .bind(to)
    .bind(end)
    .bind(ids)
    .fetch_all(conn)
    .await
    .map_err(store_err)?;
    Ok(found.into_iter().collect())
}

/// Indexes the searched messages of `thread` with `after < seq <= to`, oldest first; returns how many rows
/// of the index are in that range afterwards (a message a later one replaced is not one). The log is read
/// in pages (only the two kinds that carry words). `end` is the last seq of the log the caller has not
/// indexed yet beyond `to` (`to` itself when the range runs to the end): a message that a final message of
/// `(to, end]` replaces is not indexed. An event that does not decode is left out and logged (its seq and
/// thread, never its content): the index is not a reason for a conversation to stop.
pub(crate) async fn catch_up(
    conn: &mut PgConnection,
    thread: ThreadId,
    owner: &UserId,
    after: i64,
    to: i64,
    end: i64,
) -> Result<u64, StoreError> {
    let started = Instant::now();
    let mut cursor = after;
    while cursor < to {
        let rows = sqlx::query(
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = $1 AND seq > $2 AND seq <= $3 \
             AND kind IN ('user_message', 'agent_message') ORDER BY seq LIMIT $4",
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
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            match event_from_row(thread, row) {
                Ok(event) => events.push(event),
                Err(_) => tracing::warn!(
                    %thread,
                    seq = row.try_get::<i64, _>("seq").unwrap_or(0),
                    "a message does not decode and is not in the search index"
                ),
            }
        }
        let ids: Vec<String> = events
            .iter()
            .filter_map(|e| message_change(e).and_then(|c| c.replaces.map(str::to_owned)))
            .collect();
        let later = replaced_later(conn, thread, to, end, &ids).await?;
        for event in &events {
            let Some(change) = message_change(event) else {
                continue;
            };
            if change.replaces.is_some_and(|id| later.contains(id)) {
                continue;
            }
            index_change(conn, thread, owner, event.seq, change).await?;
        }
    }
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM message_terms WHERE thread_id = $1 AND seq > $2 AND seq <= $3",
    )
    .bind(thread.0)
    .bind(after)
    .bind(to)
    .fetch_one(&mut *conn)
    .await
    .map_err(store_err)?;
    let stored = u64::try_from(stored).unwrap_or(0);
    if stored > 0 {
        tracing::info!(
            %thread,
            messages = stored,
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "the search index caught up with the log"
        );
    }
    Ok(stored)
}

/// Whether the error is Postgres' `query_canceled` (`57014`), what `statement_timeout` raises.
fn is_statement_timeout(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "57014")
}

/// Whether the error is Postgres' `lock_not_available` (`55P03`), what `lock_timeout` raises.
fn is_lock_timeout(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code == "55P03")
}

/// A search that could not be answered now: the store is down or the query ran out of time.
fn search_err(err: sqlx::Error) -> StoreError {
    if is_statement_timeout(&err) {
        StoreError::unavailable(err)
    } else {
        store_err(err)
    }
}

impl ThreadSearch for PgStore {
    async fn search_threads(
        &self,
        owner: &UserId,
        request: &SearchRequest,
    ) -> Result<SearchRows, StoreError> {
        if request.terms.is_empty() {
            return Ok(SearchRows {
                rows: Vec::new(),
                more: false,
            });
        }
        let query = tsquery(&request.terms)?;
        let limit = i64::from(request.limit.max(1));
        // A read transaction with ONE deadline for all its statements (see `arm`).
        let deadline = Instant::now() + SEARCH_BUDGET;
        let mut tx = self.pool().begin().await.map_err(search_err)?;
        arm(&mut tx, deadline).await?;
        // the threads: the owner's, the ones the list would show, whose title or description matches or
        // that have a message that does (the events a fork copied are not its own while its parent exists)
        let rows = sqlx::query(concat!(
            "WITH q AS (SELECT $2::text::tsquery AS q) \
             SELECT s.* FROM ( \
               SELECT ",
            thread_cols!(),
            ", \
                 CASE WHEN title_terms @@ q.q THEN 2 WHEN description_terms @@ q.q THEN 1 \
                      ELSE 0 END AS hit_rank \
               FROM threads t, q \
               WHERE t.owner = $1 AND ($3 OR t.fork_kind IS DISTINCT FROM 'edit') \
                 AND (($4 = 'include') OR (($4 = 'only') = (t.archived_at IS NOT NULL))) \
                 AND t.search_fold = $8 \
                 AND (title_terms @@ q.q OR description_terms @@ q.q OR t.id IN ( \
                   SELECT m.thread_id FROM message_terms m JOIN threads f ON f.id = m.thread_id \
                   WHERE m.owner = $1 AND m.terms @@ q.q \
                     AND (f.forked_from IS NULL OR m.seq > f.forked_at))) \
             ) s \
             WHERE ($5::int IS NULL OR s.hit_rank < $5 OR (s.hit_rank = $5 AND s.id < $6)) \
             ORDER BY s.hit_rank DESC, s.id DESC LIMIT $7"
        ))
        .bind(owner.as_str())
        .bind(&query)
        .bind(request.include_edits)
        .bind(archived_name(request.archived))
        .bind(request.after.map(|p| i32::from(p.rank.number())))
        .bind(request.after.map(|p| p.thread.0))
        .bind(limit + 1)
        .bind(FOLD_VERSION)
        .fetch_all(&mut *tx)
        .await
        .map_err(search_err)?;
        let more = i64::try_from(rows.len()).is_ok_and(|n| n > limit);
        let page = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut found: Vec<SearchRow> = Vec::with_capacity(rows.len().min(page));
        for row in rows.iter().take(page) {
            let rank: i32 = row.try_get("hit_rank").map_err(store_err)?;
            let rank = u8::try_from(rank)
                .ok()
                .and_then(SearchRank::from_number)
                .ok_or_else(|| StoreError::corrupt("a search rank is out of range"))?;
            found.push(SearchRow {
                thread: thread_from_row(row)?,
                rank,
                messages: Vec::new(),
                message_hits: 0,
            });
        }
        if !found.is_empty() {
            let ids: Vec<uuid::Uuid> = found.iter().map(|r| r.thread.id.0).collect();
            // how many messages match, per thread
            arm(&mut tx, deadline).await?;
            let counts: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
                "SELECT m.thread_id, count(*)::bigint FROM message_terms m \
                 JOIN threads f ON f.id = m.thread_id \
                 WHERE m.thread_id = ANY($2) AND m.owner = $1 AND m.terms @@ $3::text::tsquery \
                   AND (f.forked_from IS NULL OR m.seq > f.forked_at) \
                 GROUP BY m.thread_id",
            )
            .bind(owner.as_str())
            .bind(&ids)
            .bind(&query)
            .fetch_all(&mut *tx)
            .await
            .map_err(search_err)?;
            // the newest three of them, per thread
            arm(&mut tx, deadline).await?;
            let newest: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
                "SELECT x.thread_id, x.seq FROM unnest($2::uuid[]) AS ids(id), LATERAL ( \
                   SELECT m.thread_id, m.seq FROM message_terms m \
                   JOIN threads f ON f.id = m.thread_id \
                   WHERE m.thread_id = ids.id AND m.owner = $1 AND m.terms @@ $3::text::tsquery \
                     AND (f.forked_from IS NULL OR m.seq > f.forked_at) \
                   ORDER BY m.seq DESC LIMIT $4) x",
            )
            .bind(owner.as_str())
            .bind(&ids)
            .bind(&query)
            .bind(i64::try_from(MAX_MESSAGE_HITS).unwrap_or(3))
            .fetch_all(&mut *tx)
            .await
            .map_err(search_err)?;
            // the words of those messages, from the log
            let threads: Vec<uuid::Uuid> = newest.iter().map(|(t, _)| *t).collect();
            let seqs: Vec<i64> = newest.iter().map(|(_, s)| *s).collect();
            arm(&mut tx, deadline).await?;
            let events = sqlx::query(
                "SELECT e.thread_id, e.seq, e.at, e.kind, e.actor, e.data FROM events e \
                 JOIN unnest($1::uuid[], $2::bigint[]) AS w(thread_id, seq) \
                   ON e.thread_id = w.thread_id AND e.seq = w.seq",
            )
            .bind(&threads)
            .bind(&seqs)
            .fetch_all(&mut *tx)
            .await
            .map_err(search_err)?;
            tx.commit().await.map_err(search_err)?;
            for (thread, n) in counts {
                if let Some(row) = found.iter_mut().find(|r| r.thread.id.0 == thread) {
                    row.message_hits = u32::try_from(n).unwrap_or(u32::MAX);
                }
            }
            for row in &events {
                let thread: uuid::Uuid = row.try_get("thread_id").map_err(store_err)?;
                let event = event_from_row(ThreadId(thread), row)?;
                // the words are the log's, so a row of the index that is behind the log cannot show
                // a message that is not the one it says
                let Some(change) = message_change(&event) else {
                    continue;
                };
                let (Some(text), Some(target)) = (
                    change.text,
                    found.iter_mut().find(|r| r.thread.id.0 == thread),
                ) else {
                    continue;
                };
                if request.terms.matches(text) {
                    target.messages.push(MatchedMessage {
                        seq: event.seq,
                        role: change.role,
                        at: event.at,
                        text: text.to_owned(),
                    });
                }
            }
            for row in &mut found {
                row.messages.sort_by_key(|m| std::cmp::Reverse(m.seq));
                row.message_hits = row
                    .message_hits
                    .max(u32::try_from(row.messages.len()).unwrap_or(0));
            }
        } else {
            tx.commit().await.map_err(search_err)?;
        }
        Ok(SearchRows { rows: found, more })
    }

    async fn backfill_search(
        &self,
        after: Option<ThreadId>,
        limit: u32,
        rebuild: bool,
    ) -> Result<SearchBackfill, StoreError> {
        let limit = limit.max(1);
        let mut page = SearchBackfill::default();
        let due: Vec<uuid::Uuid> = sqlx::query_scalar(
            "SELECT id FROM threads WHERE ($1::uuid IS NULL OR id > $1) \
             AND ($3 OR search_through < last_seq OR search_fold <> $4) ORDER BY id LIMIT $2",
        )
        .bind(after.map(|t| t.0))
        .bind(i64::from(limit))
        .bind(rebuild)
        .bind(FOLD_VERSION)
        .fetch_all(self.pool())
        .await
        .map_err(store_err)?;
        page.next = (due.len() == limit as usize)
            .then(|| due.last().copied().map(ThreadId))
            .flatten();
        for id in due {
            let mut tx = self.pool().begin().await.map_err(store_err)?;
            // Never wait long for a thread: a commit that holds the lock for seconds is a thread that is
            // being written to, and its own commits catch it up.
            sqlx::query("SELECT set_config('lock_timeout', $1, true)")
                .bind(BACKFILL_LOCK_TIMEOUT)
                .execute(&mut *tx)
                .await
                .map_err(store_err)?;
            // the lock every commit of the thread takes: the page and the service can run together
            let locked = match sqlx::query(
                "SELECT owner, last_seq, search_through, search_fold, title, description FROM threads \
                 WHERE id = $1 FOR UPDATE",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            {
                Ok(locked) => locked,
                Err(e) if is_lock_timeout(&e) => {
                    tracing::warn!(thread = %ThreadId(id), "search backfill: the thread is locked and is left for a later run");
                    page.skipped += 1;
                    continue; // the transaction is aborted and rolled back when `tx` drops
                }
                Err(e) => return Err(store_err(e)),
            };
            let Some(locked) = locked else {
                continue; // erased since
            };
            let owner = UserId::new(&locked.try_get::<String, _>("owner").map_err(store_err)?);
            let last_seq: i64 = locked.try_get("last_seq").map_err(store_err)?;
            let mut through: i64 = locked.try_get("search_through").map_err(store_err)?;
            let fold: i32 = locked.try_get("search_fold").map_err(store_err)?;
            if rebuild || fold != FOLD_VERSION {
                // keys made by other rules, or a rebuild asked for: from the start, rows replaced
                delete_index(&mut tx, ThreadId(id)).await?;
                through = 0;
            } else if through >= last_seq {
                continue; // a commit caught it up since the page was read
            }
            let title: String = locked.try_get("title").map_err(store_err)?;
            let description: Option<String> = locked.try_get("description").map_err(store_err)?;
            let indexed =
                catch_up(&mut tx, ThreadId(id), &owner, through, last_seq, last_seq).await?;
            sqlx::query(
                "UPDATE threads SET search_through = $2, search_fold = $5, \
                 title_terms = array_to_tsvector($3::text[]), \
                 description_terms = array_to_tsvector($4::text[]) WHERE id = $1",
            )
            .bind(id)
            .bind(last_seq)
            .bind(keys_of(Some(&title)))
            .bind(keys_of(description.as_deref()))
            .bind(FOLD_VERSION)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
            tx.commit().await.map_err(store_err)?;
            page.threads += 1;
            page.messages += indexed;
        }
        Ok(page)
    }

    async fn search_backlog(&self, cap: u32) -> Result<u32, StoreError> {
        // `LIMIT` stops the read at `cap` threads; when nearly every thread is indexed it reads the
        // whole table once (there is no index on these columns: they change with every commit).
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM (SELECT 1 FROM threads \
             WHERE search_through < last_seq OR search_fold <> $1 LIMIT $2) s",
        )
        .bind(FOLD_VERSION)
        .bind(i64::from(cap))
        .fetch_one(self.pool())
        .await
        .map_err(store_err)?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What each statement of a search may use is what the ones before it left of ONE budget.
    #[test]
    fn the_statements_of_a_search_share_one_budget() {
        let start = Instant::now();
        let deadline = start + SEARCH_BUDGET;
        assert_eq!(remaining_ms(deadline, start), Some(2000));
        // the first statement used 1.5 s: the next may use 500 ms, not 2 s again
        assert_eq!(
            remaining_ms(deadline, start + Duration::from_millis(1500)),
            Some(500)
        );
        // never zero while any time is left, and none once it is spent
        assert_eq!(
            remaining_ms(deadline, deadline - Duration::from_micros(10)),
            Some(1)
        );
        assert_eq!(remaining_ms(deadline, deadline), None);
        assert_eq!(
            remaining_ms(deadline, deadline + Duration::from_secs(1)),
            None
        );
    }
}
