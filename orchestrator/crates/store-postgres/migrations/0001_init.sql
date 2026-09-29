-- Orchestrator schema: threads, per-thread event log, A2A binding, outbox.
-- Applied exactly once: sqlx records it in `_sqlx_migrations` under an advisory lock, so
-- every replica may run migrations at boot. Never edit an applied migration; add a new one.

CREATE TABLE threads (
    id         uuid PRIMARY KEY,
    owner      text NOT NULL,
    title      text NOT NULL,
    agent_id   text NOT NULL,
    release    text,
    state      text NOT NULL CHECK (state IN ('queued', 'working', 'blocked', 'done', 'failed', 'cancelled')),
    -- Optimistic-concurrency version; creation is version 1.
    version    bigint NOT NULL CHECK (version >= 1),
    -- The per-thread seq counter row: bumped in the same transaction as the event inserts,
    -- under the row lock of this very row, so seq has no gaps and no duplicates.
    last_seq   bigint NOT NULL DEFAULT 0 CHECK (last_seq >= 0),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
CREATE INDEX threads_owner_id ON threads (owner, id DESC);

CREATE TABLE events (
    thread_id       uuid NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    seq             bigint NOT NULL CHECK (seq >= 1),
    at              timestamptz NOT NULL,
    kind            text NOT NULL CHECK (kind IN ('user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error')),
    actor           jsonb NOT NULL,
    data            jsonb NOT NULL,
    idempotency_key text,
    PRIMARY KEY (thread_id, seq)
);
CREATE UNIQUE INDEX events_idempotency ON events (thread_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

CREATE TABLE a2a_bindings (
    thread_id  uuid PRIMARY KEY REFERENCES threads (id) ON DELETE CASCADE,
    agent_id   text NOT NULL,
    context_id text NOT NULL,
    task_id    text,
    task_state text,
    revision   text,
    updated_at timestamptz NOT NULL
);

CREATE TABLE outbox (
    id              uuid PRIMARY KEY,
    -- Global insertion order; per thread it matches commit order because writers of one
    -- thread are serialised by the thread row lock.
    ord             bigserial NOT NULL UNIQUE,
    thread_id       uuid NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    kind            text NOT NULL CHECK (kind IN ('delegate', 'cancel')),
    payload         jsonb NOT NULL,
    status          text NOT NULL CHECK (status IN ('pending', 'inflight', 'delivered', 'dead', 'skipped')),
    attempts        integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL,
    lease_owner     text,
    lease_until     timestamptz,
    sent_at         timestamptz,
    last_error      text,
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL
);
CREATE INDEX outbox_open ON outbox (ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_open_thread ON outbox (thread_id, ord) WHERE status IN ('pending', 'inflight');
