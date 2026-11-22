//! The search index of a person's threads over SQLite (ADR 0068, migration `0002`).
//!
//! [`SqliteStore`] writes it **inside the transaction that appends the event**: `commit`,
//! `create_thread` and `fork_thread` call [`index_events`] right after the events are inserted, so
//! an event and its words are committed together or neither is. [`catch_up_before_commit`] first
//! indexes some of what the index has not been through (`threads.search_through` behind `last_seq`:
//! events a build without the index appended), **at most
//! [`COMMIT_CATCH_UP_EVENTS`](orch_core::search::COMMIT_CATCH_UP_EVENTS) events per commit**, and
//! advances the watermark only as far as it got; the backfill finishes the rest. A thread stored
//! under another [`FOLD_VERSION`] is unindexed: its rows are deleted and it starts again from `0`.
//! Erasing a thread removes its words in the same transaction: `message_terms` cascades from
//! `events`, and `thread_terms` cascades from `threads`.
//!
//! The rules (what a word is, its key, which events are searched) are `orch_core::search`. **This
//! module is only the index of the keys it makes** (`document_keys`) and is asked for the keys of a
//! query (`SearchTerms::as_slice`), so the candidates it finds are the contract's whatever the
//! database's collation or text-search configuration: there is no FTS5 here (it is not compiled
//! into every bundled SQLite, and its folding is not `fold`'s), no dictionary, no locale, and no
//! stemming. A key is stored as it comes out of `fold` and matched by prefix with `substr`, which
//! is a byte comparison; a query's text never reaches SQL, only the terms of a [`SearchTerms`],
//! whose every term is letters and numbers, checked once more before it is bound.

use std::collections::HashSet;

use orch_core::search::{
    COMMIT_CATCH_UP_EVENTS, FOLD_VERSION, MAX_MESSAGE_HITS, MessageChange, SearchTerms,
    document_keys, message_change,
};
use orch_core::{Event, ThreadId, UserId};
use orch_ports::{
    MatchedMessage, SearchBackfill, SearchRank, SearchRequest, SearchRow, SearchRows, StoreError,
    ThreadSearch,
};
use rusqlite::Connection;
use rusqlite::types::Value;

use crate::SqliteStore;
use crate::codec::{event_from_row, thread_from_row};
use crate::error::store_err;
use crate::store::{
    archived_name, dynamic_rows, exec, numbered, parse_id, row_opt, rows, scalar, texts, write,
};

/// How many events of the log one read of a catch-up takes.
const CATCH_UP_PAGE: i64 = 500;

/// `(?1), (?2), ...`: one row of one column per term, for the `VALUES` list of the terms' CTE.
/// SQLite wants every row parenthesized, so `?1, ?2` alone is a syntax error.
fn value_rows(from: usize, n: usize) -> String {
    (from..from + n)
        .map(|i| format!("(?{i})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The keys to store for a title or a description: none for a text that is not there.
pub(crate) fn keys_of(text: Option<&str>) -> Vec<String> {
    text.map(document_keys).unwrap_or_default()
}

/// Replaces the keys of `thread`'s title or description (`kind`) with `keys`.
pub(crate) fn write_thread_keys(
    conn: &Connection,
    thread: ThreadId,
    kind: &str,
    keys: &[String],
) -> Result<(), StoreError> {
    let id = thread.0.to_string();
    exec(
        conn,
        "DELETE FROM thread_terms WHERE thread_id = ?1 AND kind = ?2",
        &[&id, &kind],
    )?;
    for key in keys {
        exec(
            conn,
            "INSERT INTO thread_terms (thread_id, kind, key) VALUES (?1, ?2, ?3) \
             ON CONFLICT DO NOTHING",
            &[&id, &kind, key],
        )?;
    }
    Ok(())
}

/// Deletes every row of the index of `thread` (a thread stored under another fold version, or a
/// rebuild).
pub(crate) fn delete_index(conn: &Connection, thread: ThreadId) -> Result<(), StoreError> {
    let id = thread.0.to_string();
    exec(
        conn,
        "DELETE FROM message_terms WHERE thread_id = ?1",
        &[&id],
    )?;
    exec(
        conn,
        "DELETE FROM thread_terms WHERE thread_id = ?1",
        &[&id],
    )?;
    Ok(())
}

/// Writes what one event changes in the index of messages of `thread`: a later final message
/// deletes the earlier rows of its `messageId`, and the message's keys are stored when it has any.
fn index_change(
    conn: &Connection,
    thread: ThreadId,
    owner: &UserId,
    seq: i64,
    change: MessageChange<'_>,
) -> Result<bool, StoreError> {
    let id = thread.0.to_string();
    if let Some(replaced) = change.replaces {
        exec(
            conn,
            "DELETE FROM message_terms WHERE thread_id = ?1 AND message_id = ?2 AND seq < ?3",
            &[&id, &replaced, &seq],
        )?;
    }
    let Some(text) = change.text else {
        return Ok(false);
    };
    let keys = document_keys(text);
    if keys.is_empty() {
        return Ok(false);
    }
    for key in keys {
        exec(
            conn,
            "INSERT INTO message_terms (thread_id, seq, owner, message_id, key) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (thread_id, seq, key) DO UPDATE SET owner = excluded.owner, \
             message_id = excluded.message_id",
            &[&id, &seq, &owner.as_str(), &change.replaces, &key],
        )?;
    }
    Ok(true)
}

/// Indexes the searched messages among `events`, in order. The events are those of a thread of
/// `owner`, already inserted in this transaction.
pub(crate) fn index_events(
    conn: &Connection,
    thread: ThreadId,
    owner: &UserId,
    events: &[Event],
) -> Result<(), StoreError> {
    for event in events {
        if let Some(change) = message_change(event) {
            index_change(conn, thread, owner, event.seq, change)?;
        }
    }
    Ok(())
}

/// What a commit owes the index of the old log before it indexes its own events: at most
/// [`COMMIT_CATCH_UP_EVENTS`] events after `through`, never past `last_seq` (the thread's last
/// event before the commit). Returns the seq the index is through afterwards: `last_seq` when the
/// old log is done, else exactly where it stopped, so the watermark has no gap and a later commit
/// or the backfill continues from there.
pub(crate) fn catch_up_before_commit(
    conn: &Connection,
    thread: ThreadId,
    owner: &UserId,
    through: i64,
    last_seq: i64,
) -> Result<i64, StoreError> {
    let to = last_seq.min(through.saturating_add(COMMIT_CATCH_UP_EVENTS));
    if through < to {
        catch_up(conn, thread, owner, through, to, last_seq)?;
    }
    Ok(to.max(through))
}

/// The ids, among `ids`, of agent messages that a **later** final message (above `to`, up to `end`)
/// replaces: indexing the earlier one now would only be undone when the catch-up reaches the later.
fn replaced_later(
    conn: &Connection,
    thread: ThreadId,
    to: i64,
    end: i64,
    ids: &[String],
) -> Result<HashSet<String>, StoreError> {
    if ids.is_empty() || to >= end {
        return Ok(HashSet::new());
    }
    let mut values: Vec<Value> = vec![
        Value::Text(thread.0.to_string()),
        Value::Integer(to),
        Value::Integer(end),
    ];
    values.extend(texts(ids));
    let found: Vec<Option<String>> = dynamic_rows(
        conn,
        &format!(
            "SELECT DISTINCT json_extract(data, '$.messageId') FROM events \
             WHERE thread_id = ?1 AND seq > ?2 AND seq <= ?3 AND kind = 'agent_message' \
             AND json_extract(data, '$.final') = 1 AND json_extract(data, '$.messageId') \
             IN ({})",
            numbered(4, ids.len())
        ),
        values,
        |row| row.get(0).map_err(store_err),
    )?;
    Ok(found.into_iter().flatten().collect())
}

/// Indexes the searched messages of `thread` with `after < seq <= to`, oldest first; returns how
/// many messages are in that range of the index afterwards (a message a later one replaced is not
/// one). The log is read in pages (only the two kinds that carry words). `end` is the last seq of
/// the log the caller has not indexed yet beyond `to` (`to` itself when the range runs to the end):
/// a message that a final message of `(to, end]` replaces is not indexed. An event that does not
/// decode is left out and logged (its seq and thread, never its content): the index is not a reason
/// for a conversation to stop.
pub(crate) fn catch_up(
    conn: &Connection,
    thread: ThreadId,
    owner: &UserId,
    after: i64,
    to: i64,
    end: i64,
) -> Result<u64, StoreError> {
    let thread_text = thread.0.to_string();
    let mut cursor = after;
    while cursor < to {
        let batch: Vec<(i64, Result<Event, StoreError>)> = rows(
            conn,
            "SELECT seq, at, kind, actor, data FROM events \
             WHERE thread_id = ?1 AND seq > ?2 AND seq <= ?3 \
             AND kind IN ('user_message', 'agent_message') ORDER BY seq LIMIT ?4",
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
        let mut events = Vec::with_capacity(batch.len());
        for (seq, event) in batch {
            match event {
                Ok(event) => events.push(event),
                Err(_) => tracing::warn!(
                    %thread,
                    seq,
                    "a message does not decode and is not in the search index"
                ),
            }
        }
        let ids: Vec<String> = events
            .iter()
            .filter_map(|e| message_change(e).and_then(|c| c.replaces.map(str::to_owned)))
            .collect();
        let later = replaced_later(conn, thread, to, end, &ids)?;
        for event in &events {
            let Some(change) = message_change(event) else {
                continue;
            };
            if change.replaces.is_some_and(|id| later.contains(id)) {
                continue;
            }
            index_change(conn, thread, owner, event.seq, change)?;
        }
    }
    let stored = scalar(
        conn,
        "SELECT count(*) FROM (SELECT DISTINCT seq FROM message_terms \
         WHERE thread_id = ?1 AND seq > ?2 AND seq <= ?3)",
        &[&thread_text, &after, &to],
    )?;
    Ok(u64::try_from(stored).unwrap_or(0))
}

/// The `(thread_id, seq)` of the messages of `thread` that hold every term, newest first.
fn matching_messages(
    conn: &Connection,
    thread: ThreadId,
    terms: &SearchTerms,
    cut: Option<i64>,
) -> Result<Vec<i64>, StoreError> {
    let n = terms.len();
    let mut values: Vec<Value> = terms
        .as_slice()
        .iter()
        .map(|t| Value::Text(t.clone()))
        .collect();
    values.push(Value::Text(thread.0.to_string()));
    values.push(cut.map_or(Value::Null, Value::Integer));
    dynamic_rows(
        conn,
        &format!(
            "WITH q(term) AS (VALUES {}) \
             SELECT m.seq FROM message_terms m \
             JOIN q ON substr(m.key, 1, length(q.term)) = q.term \
             WHERE m.thread_id = ?{thread} AND (?{cut} IS NULL OR m.seq > ?{cut}) \
             GROUP BY m.seq HAVING count(DISTINCT q.term) = (SELECT count(*) FROM q) \
             ORDER BY m.seq DESC",
            value_rows(1, n),
            thread = n + 1,
            cut = n + 2,
        ),
        values,
        |row| row.get(0).map_err(store_err),
    )
}

/// The predicate "every term of `q` is a prefix of a key of the `kind` document of `alias`".
fn document_matches(alias: &str, kind: &str) -> String {
    format!(
        "(SELECT count(*) FROM q WHERE EXISTS (SELECT 1 FROM thread_terms k \
         WHERE k.thread_id = {alias}.id AND k.kind = '{kind}' \
           AND substr(k.key, 1, length(q.term)) = q.term)) = (SELECT count(*) FROM q)"
    )
}

/// The subquery of the threads that have a message holding every term, in one document.
fn message_matches(owner: usize) -> String {
    format!(
        "(SELECT m.thread_id FROM message_terms m JOIN threads f ON f.id = m.thread_id \
         JOIN q ON substr(m.key, 1, length(q.term)) = q.term \
         WHERE m.owner = ?{owner} AND (f.forked_from IS NULL OR m.seq > f.forked_at) \
         GROUP BY m.thread_id, m.seq HAVING count(DISTINCT q.term) = (SELECT count(*) FROM q))"
    )
}

/// The threads of the page, in order: rank descending, then thread id descending.
fn page_sql(terms: usize) -> String {
    let owner = terms + 1;
    let edits = terms + 2;
    let archived = terms + 3;
    let fold = terms + 4;
    let rank = terms + 5;
    let id = terms + 6;
    let limit = terms + 7;
    let title = document_matches("t", "title");
    let description = document_matches("t", "description");
    format!(
        "WITH q(term) AS (VALUES {}), s AS ( \
           SELECT {cols}, CASE WHEN {title} THEN 2 WHEN {description} THEN 1 ELSE 0 END AS hit_rank \
           FROM threads t \
           WHERE t.owner = ?{owner} AND (?{edits} OR t.fork_kind IS NOT 'edit') \
             AND ((?{archived} = 'include') \
               OR ((?{archived} = 'only') = (t.archived_at IS NOT NULL))) \
             AND t.search_fold = ?{fold} \
             AND ({title} OR {description} OR t.id IN {messages}) \
         ) \
         SELECT * FROM s \
         WHERE (?{rank} IS NULL OR s.hit_rank < ?{rank} \
           OR (s.hit_rank = ?{rank} AND s.id < ?{id})) \
         ORDER BY s.hit_rank DESC, s.id DESC LIMIT ?{limit}",
        value_rows(1, terms),
        cols = crate::codec::THREAD_COLS,
        messages = message_matches(owner),
    )
}

impl ThreadSearch for SqliteStore {
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
        // The terms are the only text that reaches the statement, and `SearchTerms` has checked
        // each one: letters and numbers, 2 to 64 code points, at most 8 of them.
        let terms = request.terms.clone();
        let limit = i64::from(request.limit.max(1));
        let mut values: Vec<Value> = terms
            .as_slice()
            .iter()
            .map(|t| Value::Text(t.clone()))
            .collect();
        values.push(Value::Text(owner.as_str().to_owned()));
        values.push(Value::Integer(i64::from(request.include_edits)));
        values.push(Value::Text(archived_name(request.archived).to_owned()));
        values.push(Value::Integer(i64::from(FOLD_VERSION)));
        values.push(
            request
                .after
                .map_or(Value::Null, |p| Value::Integer(i64::from(p.rank.number()))),
        );
        values.push(
            request
                .after
                .map_or(Value::Null, |p| Value::Text(p.thread.0.to_string())),
        );
        values.push(Value::Integer(limit + 1));
        let sql = page_sql(terms.len());
        self.call(move |conn| {
            let found: Vec<SearchRow> = dynamic_rows(conn, &sql, values, |row| {
                let rank: i64 = row.get(24).map_err(store_err)?;
                let rank = u8::try_from(rank)
                    .ok()
                    .and_then(SearchRank::from_number)
                    .ok_or_else(|| StoreError::corrupt("a search rank is out of range"))?;
                Ok(SearchRow {
                    thread: thread_from_row(row)?,
                    rank,
                    messages: Vec::new(),
                    message_hits: 0,
                })
            })?;
            let more = i64::try_from(found.len()).is_ok_and(|n| n > limit);
            let page = usize::try_from(limit).unwrap_or(usize::MAX);
            let mut found: Vec<SearchRow> = found.into_iter().take(page).collect();
            for row in &mut found {
                // the events a fork copied are its parent's words while the parent exists
                let cut = row
                    .thread
                    .forked_from
                    .as_ref()
                    .and_then(|f| f.thread_id.map(|_| f.seq));
                let hits = matching_messages(conn, row.thread.id, &terms, cut)?;
                let count = u32::try_from(hits.len()).unwrap_or(u32::MAX);
                for seq in hits.iter().take(MAX_MESSAGE_HITS) {
                    let event = row_opt(
                        conn,
                        "SELECT seq, at, kind, actor, data FROM events \
                         WHERE thread_id = ?1 AND seq = ?2",
                        &[&row.thread.id.0.to_string(), seq],
                        |r| event_from_row(row.thread.id, r),
                    )?;
                    // the words are the log's, so a row of the index that is behind the log cannot
                    // show a message that is not the one it says
                    let Some(event) = event else { continue };
                    let Some(change) = message_change(&event) else {
                        continue;
                    };
                    let Some(text) = change.text else { continue };
                    if terms.matches(text) {
                        row.messages.push(MatchedMessage {
                            seq: event.seq,
                            role: change.role,
                            at: event.at,
                            text: text.to_owned(),
                        });
                    }
                }
                row.messages.sort_by_key(|m| std::cmp::Reverse(m.seq));
                row.message_hits = count.max(u32::try_from(row.messages.len()).unwrap_or(0));
            }
            Ok(SearchRows { rows: found, more })
        })
        .await
    }

    async fn backfill_search(
        &self,
        after: Option<ThreadId>,
        limit: u32,
        rebuild: bool,
    ) -> Result<SearchBackfill, StoreError> {
        let limit = limit.max(1);
        self.call(move |conn| {
            let mut page = SearchBackfill::default();
            let after = after.map(|t| t.0.to_string());
            let due: Vec<String> = rows(
                conn,
                "SELECT id FROM threads WHERE (?1 IS NULL OR id > ?1) \
                 AND (?3 OR search_through < last_seq OR search_fold <> ?4) \
                 ORDER BY id LIMIT ?2",
                &[
                    &after.as_deref(),
                    &i64::from(limit),
                    &rebuild,
                    &FOLD_VERSION,
                ],
                |row| row.get(0).map_err(store_err),
            )?;
            let full = usize::try_from(limit).is_ok_and(|n| due.len() == n);
            page.next = match due.last().filter(|_| full) {
                Some(id) => Some(ThreadId(parse_id(id)?)),
                None => None,
            };
            for id in due {
                let thread = ThreadId(parse_id(&id)?);
                let indexed = write(conn, |tx| {
                    let locked = row_opt(
                        tx,
                        "SELECT owner, last_seq, search_through, search_fold, title, description \
                         FROM threads WHERE id = ?1",
                        &[&id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0).map_err(store_err)?,
                                row.get::<_, i64>(1).map_err(store_err)?,
                                row.get::<_, i64>(2).map_err(store_err)?,
                                row.get::<_, i32>(3).map_err(store_err)?,
                                row.get::<_, String>(4).map_err(store_err)?,
                                row.get::<_, Option<String>>(5).map_err(store_err)?,
                            ))
                        },
                    )?;
                    let Some((owner, last_seq, mut through, fold, title, description)) = locked
                    else {
                        return Ok(None); // erased since
                    };
                    if rebuild || fold != FOLD_VERSION {
                        // keys made by other rules, or a rebuild asked for: from the start, rows
                        // replaced
                        delete_index(tx, thread)?;
                        through = 0;
                    } else if through >= last_seq {
                        return Ok(None); // a commit caught it up since the page was read
                    }
                    let owner = UserId::new(&owner);
                    let indexed = catch_up(tx, thread, &owner, through, last_seq, last_seq)?;
                    exec(
                        tx,
                        "UPDATE threads SET search_through = ?2, search_fold = ?3 WHERE id = ?1",
                        &[&id, &last_seq, &FOLD_VERSION],
                    )?;
                    write_thread_keys(tx, thread, "title", &keys_of(Some(&title)))?;
                    write_thread_keys(tx, thread, "description", &keys_of(description.as_deref()))?;
                    Ok(Some(indexed))
                })?;
                if let Some(indexed) = indexed {
                    page.threads += 1;
                    page.messages += indexed;
                }
            }
            Ok(page)
        })
        .await
    }

    async fn search_backlog(&self, cap: u32) -> Result<u32, StoreError> {
        self.call(move |conn| {
            let behind = scalar(
                conn,
                "SELECT count(*) FROM (SELECT 1 FROM threads \
                 WHERE search_through < last_seq OR search_fold <> ?1 LIMIT ?2)",
                &[&FOLD_VERSION, &i64::from(cap)],
            )?;
            Ok(u32::try_from(behind).unwrap_or(u32::MAX))
        })
        .await
    }
}

/// What a query names, for the tests of the predicate: the three kinds a document can be.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_document_predicate_counts_its_terms() {
        // The shape of the predicate is the contract: one key row per term, and the count of terms
        // that have one is the count of terms.
        let title = document_matches("t", "title");
        assert!(title.contains("k.kind = 'title'"));
        assert!(title.contains("= (SELECT count(*) FROM q)"));
        assert!(message_matches(3).contains("m.owner = ?3"));
        assert!(message_matches(3).contains("HAVING count(DISTINCT q.term)"));
    }

    #[test]
    fn a_page_numbers_every_parameter() {
        let sql = page_sql(2);
        // terms ?1 ?2, then owner, edits, archived, fold, rank, id, limit
        for n in 1..=9 {
            assert!(sql.contains(&format!("?{n}")), "?{n} is bound: {sql}");
        }
        assert!(sql.contains("ORDER BY s.hit_rank DESC, s.id DESC"));
    }

    #[test]
    fn the_keys_of_a_text_are_the_cores() {
        assert_eq!(keys_of(None), Vec::<String>::new());
        assert_eq!(keys_of(Some("")), Vec::<String>::new());
        assert_eq!(keys_of(Some("Le Cœur")), vec!["coeur", "le"]);
    }
}
