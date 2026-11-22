-- The usage ledger of the people (ADR 0065) and the search index of their threads (ADR 0068), in
-- SQLite (ADR 0076 decision 3, sys#382 E2 milestone 2a). The Postgres counterparts are migrations
-- `0023_usage_ledger.sql` and `0025_thread_search.sql`; this file states the same tables in the
-- types `0001_init.sql` fixes (uuid TEXT, time INTEGER microseconds, JSON TEXT, bytes BLOB), and
-- the same columns the queries of `src/usage.rs` and `src/search.rs` read. It is not a translation
-- by a tool: what Postgres gave for free is this crate's job (see below).
--
-- Both are **a function of the log**, written by `ThreadStore::commit` and by the creation of a
-- thread in the transaction that appends the event, so the log and they cannot disagree:
--
--   threads.usage_through  the seq of the thread's log the ledger has been through, every usage
--                          event up to it folded in (or, for a fork, the cut: what a fork copied
--                          was counted in its parent). 0 for a thread a build without the ledger
--                          wrote: the next commit of the thread or `backfill` folds it in.
--   threads.search_through the seq of the thread's log the index has been through, with no gap.
--                          Rows may exist above it (a commit indexes its own events while it caught
--                          an old log up only in part): the writes are upserts, so the catch-up
--                          that reaches them changes nothing.
--   threads.search_fold    the `orch_core::search::FOLD_VERSION` the thread's keys were made under;
--                          0 for a thread this build did not index. A thread under another version
--                          is **unindexed**: not found, and indexed again from the start (its rows
--                          replaced) by its next commit or the backfill.
--
-- The ledger, as in Postgres:
--   usage_tasks     one row per task of an agent in a thread: the person, the month, whether it
--                   holds the task's latest total or the sum of its calls, the seq of the last
--                   event folded in, and where its commands ran. No foreign key to `threads`, on
--                   purpose: a row outlives its thread. Erasing a thread sets `thread_id` to NULL
--                   in the transaction that deletes it.
--   usage_lines     the counts of a task per provider and model (the parts the totals carry:
--                   reasoning, cached and cache-write tokens, NULL when no entry said them).
--   usage_sandbox   the time a task's commands used, per shape of the sandbox, in `ord` order.
--
-- `usage_tasks.id` is `INTEGER PRIMARY KEY`: SQLite's rowid, one counter, no sequence object. An id
-- may be reused after the row that had the largest one was swept; nothing refers to a task id
-- across transactions (`usage_lines` and `usage_sandbox` cascade from it), so reuse names nothing
-- stale.
--
-- The index, and **not** FTS5. FTS5 is not compiled into every bundled SQLite, and its folding is
-- not `orch_core::search::fold`'s: a store's job (core/src/search.rs, module doc) is only to hold
-- the keys `document_keys` makes and to answer the keys of a query, so the candidates are the
-- contract's whatever the database's collation or text-search configuration. The keys are exact,
-- already folded, and matched by prefix:
--   message_terms   one row per distinct key of a searched message (a person's `user_message`, an
--                   agent's final `agent_message` that is not working text), the last of each
--                   `messageId`. `owner` is the thread's owner: every read filters on it. It hangs
--                   on `events` with `ON DELETE CASCADE`, so erasing a thread leaves no word.
--   thread_terms    one row per distinct key of a thread's title (`kind = 'title'`) or description
--                   (`kind = 'description'`), cascading from `threads`. Postgres keeps these as
--                   `tsvector` columns because that is a value there; SQLite has no array type, and
--                   a JSON column would put `json_each` in every query and make "replace the keys"
--                   a rewrite of a document. A table of key rows gives one predicate for every
--                   document (title, description, message: every term has a key row that starts
--                   with it), one DELETE for the erase and one for the conformance hook.
--
-- No FTS index and no `collate`: the primary key of `message_terms` starts with `(thread_id, seq)`,
-- which is the access the queries make (the messages of one thread, newest first), and `key` has
-- its own index for the prefix probe.
--
-- Never edit an applied migration; add a new one. The runner refuses a file whose hash changed
-- after it was applied (`src/schema.rs`).

ALTER TABLE threads ADD COLUMN usage_through INTEGER NOT NULL DEFAULT 0;
ALTER TABLE threads ADD COLUMN search_through INTEGER NOT NULL DEFAULT 0;
ALTER TABLE threads ADD COLUMN search_fold INTEGER NOT NULL DEFAULT 0;

CREATE TABLE usage_tasks (
    id                  INTEGER PRIMARY KEY,
    owner               TEXT    NOT NULL,
    -- NULL once the thread was erased (ADR 0043): the usage stays, the link goes.
    thread_id           TEXT,
    agent               TEXT    NOT NULL,
    task                TEXT    NOT NULL,
    -- 'YYYY-MM', the UTC month of the latest event that changed the row.
    month               TEXT    NOT NULL CHECK (month GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]'),
    source              TEXT    NOT NULL CHECK (source IN ('calls', 'total')),
    seq                 INTEGER NOT NULL CHECK (seq >= 1),
    -- NULL: the total said no sandbox; else a word of the contract, checked for its shape only, so
    -- a new environment needs no migration.
    sandbox_environment TEXT    CHECK (sandbox_environment GLOB '[a-z][a-z0-9_-]*'),
    -- Whether the environment measured the commands: `usage_sandbox` holds the entries (none at all
    -- is a measured zero); false is not measured, and not measured is never zero.
    sandbox_measured    INTEGER NOT NULL DEFAULT 0 CHECK (sandbox_measured IN (0, 1)),
    CHECK (sandbox_environment IS NOT NULL OR sandbox_measured = 0)
);
-- A task is one row while its thread exists; a row whose thread was erased names none and is not
-- unique.
CREATE UNIQUE INDEX usage_tasks_task ON usage_tasks (thread_id, agent, task)
    WHERE thread_id IS NOT NULL;
-- A person's month (the read of GET /api/me/usage) and the retention sweep.
CREATE INDEX usage_tasks_owner_month ON usage_tasks (owner, month);
CREATE INDEX usage_tasks_month ON usage_tasks (month);

CREATE TABLE usage_lines (
    task_id                  INTEGER NOT NULL REFERENCES usage_tasks (id) ON DELETE CASCADE,
    -- '' when the agent named no provider.
    provider                 TEXT    NOT NULL,
    model                    TEXT    NOT NULL,
    input_tokens             INTEGER NOT NULL CHECK (input_tokens >= 0),
    output_tokens            INTEGER NOT NULL CHECK (output_tokens >= 0),
    total_tokens             INTEGER NOT NULL CHECK (total_tokens >= 0),
    reasoning_tokens         INTEGER CHECK (reasoning_tokens >= 0),
    cached_input_tokens      INTEGER CHECK (cached_input_tokens >= 0),
    cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens >= 0),
    PRIMARY KEY (task_id, provider, model)
);

CREATE TABLE usage_sandbox (
    task_id      INTEGER NOT NULL REFERENCES usage_tasks (id) ON DELETE CASCADE,
    ord          INTEGER NOT NULL,
    cpu_millis   INTEGER CHECK (cpu_millis >= 0),
    memory_bytes INTEGER CHECK (memory_bytes >= 0),
    commands     INTEGER NOT NULL CHECK (commands >= 0),
    milliseconds INTEGER NOT NULL CHECK (milliseconds >= 0),
    PRIMARY KEY (task_id, ord)
);

CREATE TABLE message_terms (
    thread_id  TEXT    NOT NULL,
    seq        INTEGER NOT NULL,
    owner      TEXT    NOT NULL,
    message_id TEXT,
    key        TEXT    NOT NULL,
    PRIMARY KEY (thread_id, seq, key),
    FOREIGN KEY (thread_id, seq) REFERENCES events (thread_id, seq) ON DELETE CASCADE
);
-- The prefix probe of one key, and the owner filter of every read.
CREATE INDEX message_terms_key ON message_terms (key);
CREATE INDEX message_terms_owner ON message_terms (owner);
-- A later final message deletes the earlier rows of its id.
CREATE INDEX message_terms_message ON message_terms (thread_id, message_id)
    WHERE message_id IS NOT NULL;

CREATE TABLE thread_terms (
    thread_id TEXT NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    kind      TEXT NOT NULL CHECK (kind IN ('title', 'description')),
    key       TEXT NOT NULL,
    PRIMARY KEY (thread_id, kind, key)
);
