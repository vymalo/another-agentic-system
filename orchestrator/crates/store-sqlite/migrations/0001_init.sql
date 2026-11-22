-- The orchestrator's thread store in SQLite (ADR 0076 decisions 3 and 6, sys#382 E2).
--
-- This is not the Postgres schema translated by a tool: the 29 migrations of
-- `orch-store-postgres` are not portable (`jsonb`, `uuid`, `bytea`, `array_to_tsvector`,
-- `NOT VALID`, partial indexes over expressions), so this file states the same tables in
-- SQLite's types, and a later slice adds the tables of the ports that follow (the usage ledger,
-- the search index, the prompt library, people's tool servers, pending authorizations).
--
-- The column representation, fixed here and read by `src/codec.rs`:
--
--   uuid           TEXT, the canonical lowercase hyphenated form. It sorts bytewise exactly as
--                  the 16 bytes do, so `ORDER BY id` is the order of the ids, and UUIDv7 ids sort
--                  by creation time (the listing reads `id DESC`, newest first).
--   timestamptz    INTEGER, microseconds since the Unix epoch. `src/codec.rs` rounds a time to the
--                  microsecond before writing it, so what is read is what was written.
--   jsonb          TEXT, the JSON as serde writes it (`json_extract` reads a member).
--   bytea          BLOB.
--   bigserial      INTEGER, given by `next_ord` inside the writing transaction: one writer means
--                  one counter with no gaps.
--
-- What SQLite has and Postgres had to be told: one process, one writer. Every write of this store
-- is one `BEGIN IMMEDIATE` transaction, so the `FOR UPDATE` / `FOR SHARE` / `SKIP LOCKED` clauses
-- of the Postgres statements have no work to do here and are left out; the guarantees they carried
-- (a claim that cannot be taken twice, a version check that cannot interleave) hold because no
-- second writer can be in the file at once (ADR 0076 decision 6: a thread has one home).
--
-- Never edit an applied migration; add a new one. The runner refuses a file whose hash changed
-- after it was applied (`src/schema.rs`).

CREATE TABLE threads (
    id            TEXT PRIMARY KEY,
    owner         TEXT NOT NULL,
    title         TEXT NOT NULL,
    description   TEXT,
    agent_id      TEXT NOT NULL,
    release       TEXT,
    state         TEXT NOT NULL CHECK (state IN ('queued', 'working', 'verifying', 'blocked', 'done', 'failed', 'cancelled')),
    -- The job ledger, as JSON: the contract `Thread.job` and the private parts of it.
    job           TEXT NOT NULL DEFAULT '{}',
    -- Optimistic concurrency; creation is version 1.
    version       INTEGER NOT NULL CHECK (version >= 1),
    -- The per-thread seq counter: bumped in the transaction that appends the events, so seq has no
    -- gaps and no duplicates.
    last_seq      INTEGER NOT NULL DEFAULT 0 CHECK (last_seq >= 0),
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    -- Where the thread was forked from (ADR 0029). `forked_from` is NULL once the parent is gone;
    -- `forked_at` is the last event copied (the cut) and `fork_kind` is `fork` or `edit`. The three
    -- are absent together for a thread that was not forked.
    forked_from   TEXT REFERENCES threads (id) ON DELETE SET NULL,
    forked_at     INTEGER,
    fork_kind     TEXT,
    -- The share of the thread (ADR 0040): `private` with no nonce, or a level and the 16 bytes the
    -- link is built on.
    visibility    TEXT NOT NULL DEFAULT 'private' CHECK (visibility IN ('private', 'internal', 'public')),
    share_nonce   BLOB,
    shared_at     INTEGER,
    -- The owner's list (ADR 0042), which no event records: pinned, archived, nested under another
    -- thread of the list, and a rank key compared bytewise (SQLite's BINARY collation).
    pinned_at     INTEGER,
    archived_at   INTEGER,
    rail_parent   TEXT REFERENCES threads (id) ON DELETE SET NULL,
    rail_rank     TEXT NOT NULL,
    -- Child sessions (ADR 0072): the parent thread and the session's id. The reference is deferred
    -- because `delete_threads` erases a parent and its named children in one statement.
    child_of      TEXT REFERENCES threads (id) DEFERRABLE INITIALLY DEFERRED,
    child_session TEXT,
    CHECK ((forked_at IS NULL) = (fork_kind IS NULL)),
    CHECK (forked_from IS NULL OR forked_at IS NOT NULL),
    CHECK (forked_at IS NULL OR forked_at >= 0),
    CHECK (fork_kind IS NULL OR fork_kind IN ('fork', 'edit')),
    CHECK ((visibility = 'private') = (share_nonce IS NULL)),
    CHECK ((share_nonce IS NULL) = (shared_at IS NULL)),
    CHECK (share_nonce IS NULL OR length(share_nonce) = 16),
    CHECK ((child_of IS NULL) = (child_session IS NULL)),
    CHECK (child_session IS NULL OR length(child_session) BETWEEN 1 AND 512),
    CHECK (child_of IS NULL OR child_of <> id),
    CHECK (rail_parent IS NULL OR rail_parent <> id),
    CHECK (description IS NULL OR (description <> '' AND length(description) <= 500))
);
CREATE INDEX threads_owner_id ON threads (owner, id DESC);
CREATE INDEX threads_forked_from ON threads (forked_from) WHERE forked_from IS NOT NULL;
CREATE UNIQUE INDEX threads_share_nonce ON threads (share_nonce) WHERE share_nonce IS NOT NULL;
CREATE INDEX threads_rail ON threads (owner, rail_rank) WHERE rail_parent IS NULL;
CREATE INDEX threads_rail_parent ON threads (rail_parent) WHERE rail_parent IS NOT NULL;
CREATE UNIQUE INDEX threads_child ON threads (child_of, child_session) WHERE child_of IS NOT NULL;
CREATE INDEX threads_child_of ON threads (child_of) WHERE child_of IS NOT NULL;

-- The log. `kind` is a closed enum (ADR 0004): a new kind is a new value here, in a migration.
CREATE TABLE events (
    thread_id       TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    seq             INTEGER NOT NULL CHECK (seq >= 1),
    at              INTEGER NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN (
        'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact',
        'thread_state', 'error', 'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework',
        'job_started', 'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked',
        'thread_described', 'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
        'thread_shared', 'thread_unshared', 'model_usage', 'model_usage_total',
        'child_session_opened', 'child_session_reported', 'child_session_closed',
        'child_session_start_asked', 'child_session_start_ended', 'child_session_close_asked',
        'child_session_state', 'trusted_request')),
    actor           TEXT NOT NULL,
    data            TEXT NOT NULL,
    idempotency_key TEXT,
    PRIMARY KEY (thread_id, seq)
);
CREATE UNIQUE INDEX events_idempotency ON events (thread_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE a2a_bindings (
    thread_id  TEXT PRIMARY KEY REFERENCES threads (id) ON DELETE CASCADE,
    agent_id   TEXT NOT NULL,
    context_id TEXT,
    task_id    TEXT,
    task_state TEXT,
    revision   TEXT,
    updated_at INTEGER NOT NULL
);

-- The dispatcher's queue. `ord` is the global insertion order and the per-thread order (writers of
-- one thread are serialised), `owner` is the person the row is for (ADR 0066: the fair claim).
CREATE TABLE outbox (
    id              TEXT PRIMARY KEY,
    ord             INTEGER NOT NULL UNIQUE,
    thread_id       TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    owner           TEXT,
    kind            TEXT NOT NULL CHECK (kind IN (
        'delegate', 'cancel', 'verify', 'title', 'description', 'steer', 'ask')),
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
    -- An open row is claimable by owner, so it must have one (Postgres fills it with a trigger;
    -- here the store writes it).
    CHECK (owner IS NOT NULL OR status NOT IN ('pending', 'inflight'))
);
CREATE INDEX outbox_open ON outbox (ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_open_thread ON outbox (thread_id, ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_open_owner ON outbox (owner, ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_inflight_owner ON outbox (owner, lease_until) WHERE status = 'inflight';
CREATE INDEX outbox_open_cancel ON outbox (ord)
    WHERE kind = 'cancel' AND status IN ('pending', 'inflight');

-- Which thread a correlation key (`ci:<repo-key>@<sha>`) belongs to (ADR 0016).
CREATE TABLE watches (
    key        TEXT PRIMARY KEY,
    thread_id  TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
CREATE INDEX watches_thread ON watches (thread_id);

-- Unsolicited machine input and timers (ADR 0016): deduplicated by `(source, idempotency_key)`,
-- claimed under a lease, parked until its watch exists, expired after a time-to-live.
CREATE TABLE inbox (
    id              TEXT PRIMARY KEY,
    source          TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('timer', 'ci_report')),
    payload         TEXT NOT NULL,
    correlation     TEXT,
    status          TEXT NOT NULL CHECK (status IN (
        'pending', 'inflight', 'parked', 'applied', 'expired', 'dead')),
    available_at    INTEGER NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    refunded        INTEGER NOT NULL DEFAULT 0,
    lease_owner     TEXT,
    lease_until     INTEGER,
    parked_at       INTEGER,
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE (source, idempotency_key)
);
CREATE INDEX inbox_pending ON inbox (available_at, id) WHERE status = 'pending';
CREATE INDEX inbox_inflight ON inbox (lease_until) WHERE status = 'inflight';
CREATE INDEX inbox_parked_correlation ON inbox (correlation) WHERE status = 'parked';
CREATE INDEX inbox_parked_at ON inbox (parked_at) WHERE status = 'parked';

-- The promise that a deleted thread's files will be erased (ADR 0043). No foreign key on purpose:
-- the thread it names is gone.
CREATE TABLE thread_purges (
    thread_id   TEXT PRIMARY KEY,
    deleted_at  INTEGER NOT NULL,
    attempts    INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until INTEGER
);
CREATE INDEX thread_purges_due ON thread_purges (deleted_at, thread_id);
