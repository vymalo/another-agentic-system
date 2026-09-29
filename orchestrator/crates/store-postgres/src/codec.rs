//! Conversions between the port types and their column representations.

use jiff::{Timestamp, Unit};
use orch_core::{Actor, AgentId, AgentTarget, Event, EventBody, ThreadId, ThreadRecord, UserId};
use orch_ports::{AgentBinding, OutboxId, OutboxItem, OutboxPayload, StoreError};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::Row;
use sqlx::postgres::PgRow;

use crate::error::store_err;

/// Column list of `threads`, in the order [`thread_from_row`] reads them by name.
macro_rules! thread_cols {
    () => {
        "id, owner, title, agent_id, release, state, version, last_seq, created_at, updated_at"
    };
}

/// Column list of `outbox` (without `ord`).
macro_rules! outbox_cols {
    () => {
        "id, thread_id, kind, payload, status, attempts, next_attempt_at, lease_owner, \
         lease_until, sent_at, last_error, created_at"
    };
}

pub(crate) use {outbox_cols, thread_cols};

/// Postgres stores microseconds; rounding up front makes what we return equal what we stored.
pub(crate) fn ts(t: Timestamp) -> Timestamp {
    t.round(Unit::Microsecond).unwrap_or(t)
}

pub(crate) fn to_db(t: Timestamp) -> jiff_sqlx::Timestamp {
    jiff_sqlx::Timestamp::from(ts(t))
}

/// The snake_case wire spelling of a unit-variant enum, as stored in a text column.
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

fn get_ts(row: &PgRow, col: &str) -> Result<Timestamp, StoreError> {
    row.try_get::<jiff_sqlx::Timestamp, _>(col)
        .map(jiff_sqlx::Timestamp::to_jiff)
        .map_err(store_err)
}

pub(crate) fn get_ts_opt(row: &PgRow, col: &str) -> Result<Option<Timestamp>, StoreError> {
    row.try_get::<Option<jiff_sqlx::Timestamp>, _>(col)
        .map(|t| t.map(jiff_sqlx::Timestamp::to_jiff))
        .map_err(store_err)
}

fn get<'r, T>(row: &'r PgRow, col: &str) -> Result<T, StoreError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(col).map_err(store_err)
}

pub(crate) fn thread_from_row(row: &PgRow) -> Result<ThreadRecord, StoreError> {
    let state: String = get(row, "state")?;
    Ok(ThreadRecord {
        id: ThreadId(get(row, "id")?),
        owner: UserId::new(&get::<String>(row, "owner")?),
        title: get(row, "title")?,
        target: AgentTarget {
            agent_id: AgentId::new(get::<String>(row, "agent_id")?),
            release: get(row, "release")?,
        },
        state: parse_enum("thread state", &state)?,
        version: get(row, "version")?,
        last_seq: get(row, "last_seq")?,
        created_at: get_ts(row, "created_at")?,
        updated_at: get_ts(row, "updated_at")?,
    })
}

pub(crate) fn event_from_row(thread: ThreadId, row: &PgRow) -> Result<Event, StoreError> {
    let kind: String = get(row, "kind")?;
    let actor: serde_json::Value = get(row, "actor")?;
    let data: serde_json::Value = get(row, "data")?;
    let body = EventBody::from_parts(parse_enum("event kind", &kind)?, data)
        .map_err(|e| StoreError::corrupt_with("event data", e))?;
    let actor: Actor =
        serde_json::from_value(actor).map_err(|e| StoreError::corrupt_with("event actor", e))?;
    Ok(Event {
        seq: get(row, "seq")?,
        thread_id: thread,
        at: get_ts(row, "at")?,
        actor,
        body,
    })
}

pub(crate) fn binding_from_row(row: &PgRow) -> Result<AgentBinding, StoreError> {
    let task_state: Option<String> = get(row, "task_state")?;
    Ok(AgentBinding {
        thread_id: ThreadId(get(row, "thread_id")?),
        agent_id: AgentId::new(get::<String>(row, "agent_id")?),
        context_id: get(row, "context_id")?,
        task_id: get(row, "task_id")?,
        task_state: task_state
            .map(|s| parse_enum("task state", &s))
            .transpose()?,
        revision: get(row, "revision")?,
    })
}

pub(crate) fn outbox_from_row(row: &PgRow) -> Result<OutboxItem, StoreError> {
    let kind: String = get(row, "kind")?;
    let status: String = get(row, "status")?;
    let payload: serde_json::Value = get(row, "payload")?;
    let payload: OutboxPayload = serde_json::from_value(payload)
        .map_err(|e| StoreError::corrupt_with("outbox payload", e))?;
    let attempts: i32 = get(row, "attempts")?;
    Ok(OutboxItem {
        id: OutboxId(get(row, "id")?),
        thread_id: ThreadId(get(row, "thread_id")?),
        kind: parse_enum("outbox kind", &kind)?,
        payload,
        status: parse_enum("outbox status", &status)?,
        attempts: u32::try_from(attempts).unwrap_or(0),
        sent_at: get_ts_opt(row, "sent_at")?,
        next_attempt_at: get_ts(row, "next_attempt_at")?,
        lease_owner: get(row, "lease_owner")?,
        lease_until: get_ts_opt(row, "lease_until")?,
        last_error: get(row, "last_error")?,
        created_at: get_ts(row, "created_at")?,
    })
}
