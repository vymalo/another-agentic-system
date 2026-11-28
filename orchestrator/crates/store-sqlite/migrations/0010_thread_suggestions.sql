-- The suggestions of a job (ADR 0035, amended 2026-11-28), the SQLite form of Postgres migration
-- 0039: one more event kind (`thread_suggested`) and one more outbox kind (`suggestions`).
--
-- SQLite cannot alter a CHECK constraint, so the two tables are rebuilt as its documented
-- procedure says (create the new table, copy, drop, rename, recreate the indexes). The runner turns
-- the foreign keys off around a migration for exactly this: `message_terms` points at `events`
-- (`outbox` points at `threads`), and with the keys on dropping `events` would cascade its rows
-- away. `outbox` is rebuilt for its own CHECK, not for a reference to `events`.
--
-- No row changes meaning: every column is copied as it was.
--
-- Never edit an applied migration; add a new one.

CREATE TABLE events_new (
    thread_id       TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL CHECK (seq >= 1),
    at              INTEGER NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN (
        'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact',
        'thread_state', 'error', 'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework',
        'job_started', 'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked',
        'thread_described', 'thread_suggested', 'tools_attached', 'tools_detached',
        'ask_started', 'ask_finished', 'thread_shared', 'thread_unshared', 'model_usage',
        'model_usage_total', 'child_session_opened', 'child_session_reported',
        'child_session_closed', 'child_session_start_asked', 'child_session_start_ended',
        'child_session_close_asked', 'child_session_state', 'trusted_request')),
    actor           TEXT NOT NULL,
    data            TEXT NOT NULL,
    idempotency_key TEXT,
    PRIMARY KEY (thread_id, seq)
);
INSERT INTO events_new (thread_id, seq, at, kind, actor, data, idempotency_key)
    SELECT thread_id, seq, at, kind, actor, data, idempotency_key FROM events;
DROP TABLE events;
ALTER TABLE events_new RENAME TO events;
CREATE UNIQUE INDEX events_idempotency ON events (thread_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE outbox_new (
    id              TEXT PRIMARY KEY,
    ord             INTEGER NOT NULL UNIQUE,
    thread_id       TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    owner           TEXT,
    kind            TEXT NOT NULL CHECK (kind IN (
        'delegate', 'cancel', 'verify', 'title', 'description', 'suggestions', 'steer', 'ask')),
    payload         TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN (
        'pending', 'inflight', 'delivered', 'dead', 'skipped')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    lease_owner     TEXT,
    lease_until     INTEGER,
    sent_at         INTEGER,
    task_id         TEXT,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    refunded        INTEGER NOT NULL DEFAULT 0,
    CHECK (owner IS NOT NULL OR status NOT IN ('pending', 'inflight'))
);
INSERT INTO outbox_new (id, ord, thread_id, owner, kind, payload, status, attempts,
                        next_attempt_at, lease_owner, lease_until, sent_at, task_id, last_error,
                        created_at, updated_at, refunded)
    SELECT id, ord, thread_id, owner, kind, payload, status, attempts,
           next_attempt_at, lease_owner, lease_until, sent_at, task_id, last_error,
           created_at, updated_at, refunded
      FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_new RENAME TO outbox;
CREATE INDEX outbox_open ON outbox (ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_open_thread ON outbox (thread_id, ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_open_owner ON outbox (owner, ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_inflight_owner ON outbox (owner, lease_until) WHERE status = 'inflight';
CREATE INDEX outbox_open_cancel ON outbox (ord)
    WHERE kind = 'cancel' AND status IN ('pending', 'inflight');
