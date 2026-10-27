-- The usage ledger of the people (ADR 0065): what each person used, counted per task and per UTC month,
-- priced later and outliving the threads it came from.
--
-- It is written by the store in the transaction that appends a `model_usage` or `model_usage_total`
-- event (orch_core::usage_change says what the event changes, UsageRow::fold what the row becomes), so
-- the log and the ledger cannot disagree. Counts and labels only: no message, no thread content.
--
-- Four changes:
--   threads.usage_through   the seq of the thread's log the ledger has been through, every event up to it
--                           folded in (or, for a fork, the cut: the events a fork copied were counted in
--                           its parent). 0 for a thread that exists today: its log is not in the ledger
--                           yet, the next commit of the thread or `orchestrator usage backfill` folds it
--                           in. A build without the ledger leaves it behind `last_seq` and the next
--                           commit by a build with it catches up, so no event is missed whichever builds
--                           wrote when.
--   usage_tasks             one row per task of an agent in a thread: the person, the month it is counted
--                           in, whether it holds the task's latest total or the sum of its calls, the
--                           seq of the last event folded in, and where its commands ran
--   usage_lines             the counts of a task per provider and model (the parts the totals carry:
--                           reasoning, cached and cache-write tokens, NULL when no entry said them)
--   usage_sandbox           the time a task's commands used, per shape of the sandbox
--
-- No foreign key to threads, on purpose: a row outlives its thread. Erasing a thread (ADR 0043) sets
-- `thread_id` to NULL in the transaction that deletes it, so the usage stays and the link to the
-- erased thread goes. `usage_lines` and `usage_sandbox` hang on `usage_tasks` and go with it (the
-- retention sweep deletes old months from `usage_tasks`).
--
--   usage_tasks.owner                the thread's owner (`UserId`, the trimmed lower-cased e-mail)
--   usage_tasks.thread_id            NULL once the thread was erased
--   usage_tasks.agent, task          the agent that made the calls (the thread's, or an asked agent's)
--                                    and its A2A task; with the thread they are the task's key
--   usage_tasks.month                'YYYY-MM', the UTC month of the latest event that changed the row
--   usage_tasks.source               'calls' (the sum of the calls logged: the task has no total yet) or
--                                    'total' (the latest `model_usage_total`, which replaced them)
--   usage_tasks.seq                  the seq of the last event of the thread folded into the row
--   usage_tasks.sandbox_environment  NULL: the total said no sandbox; else where the commands ran
--   usage_tasks.sandbox_measured     whether the environment measured them: `usage_sandbox` holds the
--                                    entries (none at all is a measured zero); false is not measured,
--                                    and not measured is never zero
--   usage_lines.provider             '' when the agent named none
--
-- Counts are capped at the largest bigint by the code that writes them, so an agent that lies on
-- purpose fills a counter and never fails a commit.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN usage_through bigint NOT NULL DEFAULT 0 CHECK (usage_through >= 0);

CREATE TABLE usage_tasks (
    id                  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner               text   NOT NULL,
    thread_id           uuid,
    agent               text   NOT NULL,
    task                text   NOT NULL,
    month               text   COLLATE "C" NOT NULL CHECK (month ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'),
    source              text   NOT NULL CHECK (source IN ('calls', 'total')),
    seq                 bigint NOT NULL CHECK (seq >= 1),
    sandbox_environment text   CHECK (sandbox_environment IN ('kubernetes', 'local', 'devcontainer')),
    sandbox_measured    boolean NOT NULL DEFAULT false,
    CHECK (sandbox_environment IS NOT NULL OR NOT sandbox_measured)
);
-- A task is one row while its thread exists; a row whose thread was erased names none and is not unique.
CREATE UNIQUE INDEX usage_tasks_task ON usage_tasks (thread_id, agent, task) WHERE thread_id IS NOT NULL;
-- A person's month: the read of GET /api/me/usage.
CREATE INDEX usage_tasks_owner_month ON usage_tasks (owner, month);
-- The retention sweep removes the months before a limit.
CREATE INDEX usage_tasks_month ON usage_tasks (month);

CREATE TABLE usage_lines (
    task_id                  bigint NOT NULL REFERENCES usage_tasks (id) ON DELETE CASCADE,
    provider                 text   NOT NULL,
    model                    text   NOT NULL,
    input_tokens             bigint NOT NULL CHECK (input_tokens >= 0),
    output_tokens            bigint NOT NULL CHECK (output_tokens >= 0),
    total_tokens             bigint NOT NULL CHECK (total_tokens >= 0),
    reasoning_tokens         bigint CHECK (reasoning_tokens >= 0),
    cached_input_tokens      bigint CHECK (cached_input_tokens >= 0),
    cache_write_input_tokens bigint CHECK (cache_write_input_tokens >= 0),
    PRIMARY KEY (task_id, provider, model)
);

CREATE TABLE usage_sandbox (
    task_id      bigint   NOT NULL REFERENCES usage_tasks (id) ON DELETE CASCADE,
    ord          smallint NOT NULL,
    cpu_millis   bigint   CHECK (cpu_millis >= 0),
    memory_bytes bigint   CHECK (memory_bytes >= 0),
    commands     bigint   NOT NULL CHECK (commands >= 0),
    milliseconds bigint   NOT NULL CHECK (milliseconds >= 0),
    PRIMARY KEY (task_id, ord)
);
