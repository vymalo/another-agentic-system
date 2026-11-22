//! Conversions between the port types and their SQLite columns.
//!
//! The representation is stated in `migrations/0001_init.sql`: a uuid is its canonical text, a time
//! is microseconds since the Unix epoch, JSON is text, bytes are a blob. This module is the one
//! place that knows it, and every statement selects its columns in the order the readers here
//! expect (a trailing extra column, such as `ord`, comes after them).

use jiff::{Timestamp, Unit};
use orch_core::{
    Actor, AgentId, AgentTarget, ChildOf, Event, EventBody, EventKind, ForkedFrom, Job, NONCE_LEN,
    ShareLevel, ShareNonce, ThreadId, ThreadRecord, ThreadShare, UserId,
};
use orch_ports::{
    AgentBinding, InboxId, InboxItem, OutboxId, OutboxItem, OutboxPayload, StoreError,
};
use rusqlite::Row;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::store_err;

/// The columns of `threads`, in the order [`thread_from_row`] reads them.
pub(crate) const THREAD_COLS: &str = "id, owner, title, description, agent_id, release, state, job, \
     version, last_seq, created_at, updated_at, forked_from, forked_at, fork_kind, visibility, \
     share_nonce, shared_at, pinned_at, archived_at, rail_parent, rail_rank, child_of, \
     child_session";

/// The columns of `outbox`, in the order [`outbox_from_row`] reads them.
pub(crate) const OUTBOX_COLS: &str = "id, thread_id, kind, payload, status, attempts, \
     next_attempt_at, lease_owner, lease_until, sent_at, task_id, last_error, created_at";

/// The columns of `inbox`, in the order [`inbox_from_row`] reads them.
pub(crate) const INBOX_COLS: &str = "id, source, idempotency_key, kind, payload, correlation, \
     status, available_at, attempts, refunded, lease_owner, lease_until, parked_at, last_error, \
     created_at";

/// A time as this store keeps it: rounded to the microsecond, so that what is read back is what was
/// written (SQLite keeps the integer it is given, and a time has more digits than that).
pub(crate) fn ts(t: Timestamp) -> Timestamp {
    t.round(Unit::Microsecond).unwrap_or(t)
}

/// What is written for a time.
pub(crate) fn to_db(t: Timestamp) -> Result<i64, StoreError> {
    // `as_microsecond` is already an `i64`: nothing can fail here, and the `Result` stays so the
    // call sites keep one shape for every column.
    Ok(ts(t).as_microsecond())
}

/// The time a column holds.
pub(crate) fn from_db(us: i64) -> Result<Timestamp, StoreError> {
    Timestamp::from_microsecond(us)
        .map_err(|e| StoreError::corrupt_with("a stored time is not a time", e))
}

fn get_ts(row: &Row<'_>, at: usize) -> Result<Timestamp, StoreError> {
    from_db(row.get::<_, i64>(at).map_err(store_err)?)
}

fn get_ts_opt(row: &Row<'_>, at: usize) -> Result<Option<Timestamp>, StoreError> {
    row.get::<_, Option<i64>>(at)
        .map_err(store_err)?
        .map(from_db)
        .transpose()
}

fn get_text(row: &Row<'_>, at: usize) -> Result<String, StoreError> {
    row.get::<_, String>(at).map_err(store_err)
}

fn get_text_opt(row: &Row<'_>, at: usize) -> Result<Option<String>, StoreError> {
    row.get::<_, Option<String>>(at).map_err(store_err)
}

fn get_id(row: &Row<'_>, at: usize) -> Result<uuid::Uuid, StoreError> {
    let text = get_text(row, at)?;
    uuid::Uuid::parse_str(&text)
        .map_err(|e| StoreError::corrupt_with("a stored id is not a uuid", e))
}

fn get_id_opt(row: &Row<'_>, at: usize) -> Result<Option<uuid::Uuid>, StoreError> {
    get_text_opt(row, at)?
        .map(|text| {
            uuid::Uuid::parse_str(&text)
                .map_err(|e| StoreError::corrupt_with("a stored id is not a uuid", e))
        })
        .transpose()
}

fn get_json(row: &Row<'_>, at: usize) -> Result<serde_json::Value, StoreError> {
    let text = get_text(row, at)?;
    serde_json::from_str(&text).map_err(|e| StoreError::corrupt_with("a stored row is not JSON", e))
}

/// The snake_case wire spelling of a unit-variant enum, as a text column keeps it.
pub(crate) fn enum_str<T: Serialize>(value: &T) -> Result<String, StoreError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        Ok(other) => Err(StoreError::corrupt(format!(
            "enum did not serialise to a string: {other}"
        ))),
        Err(e) => Err(StoreError::corrupt_with("enum does not serialise", e)),
    }
}

pub(crate) fn parse_enum<T: DeserializeOwned>(what: &str, s: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| StoreError::corrupt_with(format!("unknown {what} {s:?}"), e))
}

/// The job ledger as the column keeps it.
pub(crate) fn job_json(job: &Job) -> Result<String, StoreError> {
    serde_json::to_string(job).map_err(|e| StoreError::corrupt_with("cannot serialise the job", e))
}

/// A thread row, read from [`THREAD_COLS`].
pub(crate) fn thread_from_row(row: &Row<'_>) -> Result<ThreadRecord, StoreError> {
    let state = get_text(row, 6)?;
    let job = get_json(row, 7)?;
    let job: Job =
        serde_json::from_value(job).map_err(|e| StoreError::corrupt_with("thread job", e))?;
    let fork_kind = get_text_opt(row, 14)?;
    let forked_at: Option<i64> = row.get(13).map_err(store_err)?;
    let forked_from = match (fork_kind, forked_at) {
        (Some(kind), Some(seq)) => Some(ForkedFrom {
            thread_id: get_id_opt(row, 12)?.map(ThreadId),
            seq,
            kind: parse_enum("fork kind", &kind)?,
        }),
        (None, None) => None,
        _ => {
            return Err(StoreError::corrupt(
                "a thread row has half of a fork origin",
            ));
        }
    };
    let share = share_from_row(row)?;
    let child_of = match (get_id_opt(row, 22)?, get_text_opt(row, 23)?) {
        (Some(parent), Some(session)) => Some(ChildOf {
            thread: ThreadId(parent),
            session,
        }),
        (None, None) => None,
        _ => {
            return Err(StoreError::corrupt(
                "a thread row has half of a child_of (parent and session disagree)",
            ));
        }
    };
    Ok(ThreadRecord {
        id: ThreadId(get_id(row, 0)?),
        owner: UserId::new(&get_text(row, 1)?),
        title: get_text(row, 2)?,
        description: get_text_opt(row, 3)?.filter(|d| !d.is_empty()),
        target: AgentTarget {
            agent_id: AgentId::new(get_text(row, 4)?),
            release: get_text_opt(row, 5)?,
        },
        state: parse_enum("thread state", &state)?,
        job,
        version: row.get(8).map_err(store_err)?,
        forked_from,
        share,
        pinned_at: get_ts_opt(row, 18)?,
        archived_at: get_ts_opt(row, 19)?,
        rail_parent: get_id_opt(row, 20)?.map(ThreadId),
        child_of,
        rail_rank: get_text(row, 21)?,
        last_seq: row.get(9).map_err(store_err)?,
        created_at: get_ts(row, 10)?,
        updated_at: get_ts(row, 11)?,
    })
}

/// The share a thread row holds (ADR 0040). A row with half of one is corrupt, never private, so
/// that nothing is ever served by half of a share.
fn share_from_row(row: &Row<'_>) -> Result<Option<ThreadShare>, StoreError> {
    let visibility = get_text(row, 15)?;
    let nonce: Option<Vec<u8>> = row.get(16).map_err(store_err)?;
    let shared_at = get_ts_opt(row, 17)?;
    match (visibility.as_str(), nonce, shared_at) {
        ("private", None, None) => Ok(None),
        (level @ ("internal" | "public"), Some(nonce), Some(shared_at)) => {
            let bytes: [u8; NONCE_LEN] = nonce
                .try_into()
                .map_err(|_| StoreError::corrupt("a share nonce is not 16 bytes"))?;
            Ok(Some(ThreadShare {
                level: parse_enum::<ShareLevel>("share level", level)?,
                nonce: ShareNonce::new(bytes),
                shared_at,
            }))
        }
        _ => Err(StoreError::corrupt(
            "a thread row has half of a share (visibility, nonce and time disagree)",
        )),
    }
}

/// An event row, read from `seq, at, kind, actor, data`.
pub(crate) fn event_from_row(thread: ThreadId, row: &Row<'_>) -> Result<Event, StoreError> {
    let kind = get_text(row, 2)?;
    let actor = get_json(row, 3)?;
    let data = get_json(row, 4)?;
    let body = EventBody::from_parts(parse_enum::<EventKind>("event kind", &kind)?, data)
        .map_err(|e| StoreError::corrupt_with("event data", e))?;
    let actor: Actor =
        serde_json::from_value(actor).map_err(|e| StoreError::corrupt_with("event actor", e))?;
    Ok(Event {
        seq: row.get(0).map_err(store_err)?,
        thread_id: thread,
        at: get_ts(row, 1)?,
        actor,
        body,
    })
}

/// A binding row, read from `thread_id, agent_id, context_id, task_id, task_state, revision`.
pub(crate) fn binding_from_row(row: &Row<'_>) -> Result<AgentBinding, StoreError> {
    let task_state = get_text_opt(row, 4)?;
    Ok(AgentBinding {
        thread_id: ThreadId(get_id(row, 0)?),
        agent_id: AgentId::new(get_text(row, 1)?),
        context_id: get_text_opt(row, 2)?,
        task_id: get_text_opt(row, 3)?,
        task_state: task_state
            .map(|s| parse_enum("task state", &s))
            .transpose()?,
        revision: get_text_opt(row, 5)?,
    })
}

/// An outbox row, read from [`OUTBOX_COLS`].
pub(crate) fn outbox_from_row(row: &Row<'_>) -> Result<OutboxItem, StoreError> {
    let kind = get_text(row, 2)?;
    let status = get_text(row, 4)?;
    let mut payload = get_json(row, 3)?;
    // A cancel row written before it named its job (ADR 0020) is the bare string `"cancel"`.
    if payload.as_str() == Some("cancel") {
        payload = serde_json::json!({"cancel": {}});
    }
    let payload: OutboxPayload = serde_json::from_value(payload)
        .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
    let attempts: i64 = row.get(5).map_err(store_err)?;
    Ok(OutboxItem {
        id: OutboxId(get_id(row, 0)?),
        thread_id: ThreadId(get_id(row, 1)?),
        kind: parse_enum("outbox kind", &kind)?,
        payload,
        status: parse_enum("outbox status", &status)?,
        attempts: u32::try_from(attempts).unwrap_or(0),
        sent_at: get_ts_opt(row, 9)?,
        task_id: get_text_opt(row, 10)?,
        next_attempt_at: get_ts(row, 6)?,
        lease_owner: get_text_opt(row, 7)?,
        lease_until: get_ts_opt(row, 8)?,
        last_error: get_text_opt(row, 11)?,
        created_at: get_ts(row, 12)?,
    })
}

/// An inbox row, read from [`INBOX_COLS`].
pub(crate) fn inbox_from_row(row: &Row<'_>) -> Result<InboxItem, StoreError> {
    let status = get_text(row, 6)?;
    let attempts: i64 = row.get(8).map_err(store_err)?;
    let refunded: i64 = row.get(9).map_err(store_err)?;
    Ok(InboxItem {
        id: InboxId(get_id(row, 0)?),
        source: get_text(row, 1)?,
        idempotency_key: get_text(row, 2)?,
        kind: get_text(row, 3)?,
        // Left as JSON: a row this build cannot read must not fail the claim of its batch.
        payload: get_json(row, 4)?,
        correlation: get_text_opt(row, 5)?,
        status: parse_enum("inbox status", &status)?,
        available_at: get_ts(row, 7)?,
        attempts: u32::try_from(attempts).unwrap_or(0),
        refunded: u32::try_from(refunded).unwrap_or(0),
        lease_owner: get_text_opt(row, 10)?,
        lease_until: get_ts_opt(row, 11)?,
        parked_at: get_ts_opt(row, 12)?,
        last_error: get_text_opt(row, 13)?,
        created_at: get_ts(row, 14)?,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_time_round_trips_at_the_microsecond() {
        let t: Timestamp = "2026-01-01T00:00:00.0000009Z".parse().unwrap();
        let stored = to_db(t).unwrap();
        assert_eq!(
            from_db(stored).unwrap(),
            "2026-01-01T00:00:00.000001Z".parse::<Timestamp>().unwrap(),
            "rounded up once, and read back as it was stored"
        );
    }
}
