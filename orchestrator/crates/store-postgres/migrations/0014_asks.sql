-- The agent the job runs on can ask an agent the person mentioned to do part of the work and wait for
-- its answer (ADR 0026, `ask_agent` of docs/api/thread-tools-v1.md). The asked agent runs as a child
-- task of the thread. Each ask is logged twice, as an `ask_started` and, once, as an `ask_finished`
-- (data: `{"ask": 1, "agent": "...", "by": "main", ...}` and `{"ask": 1, "state": "completed", ...}`),
-- and its request is an outbox row of the new kind `ask`, whose payload is
-- `{"ask": {"job": 1, "ask": 1, "agent": "...", "depth": 1, "text": "..."}}`. The ledger of the
-- job's asks lives inside threads.job, like the steps' and the mentions', so it needs no column of its
-- own, and the deadline of an ask is an inbox row (a timer) like the gate's, so it needs no kind.
--
-- Two schema changes:
--   events.kind   takes `ask_started` and `ask_finished`
--   outbox.kind   takes `ask`
-- Both CHECKs are rebuilt with every kind there is, as the earlier ones were. A row of the new kinds
-- is the only thing an older build cannot read (an event fails the decode of the log, an outbox row
-- fails the decode and dead-letters), so roll out the build that understands them first, on every
-- replica, before anything can ask: nothing does until the tools endpoint offers `ask_agent`.
-- The claim query of this build treats `ask` rows as it does `verify` rows: they wait for nothing,
-- neither for the thread's delegation (the asking agent is running inside it) nor for each other.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through, instead of blocking the log.
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('delegate', 'cancel', 'verify', 'title', 'description', 'steer', 'ask')) NOT VALID;
ALTER TABLE outbox VALIDATE CONSTRAINT outbox_kind_check;
