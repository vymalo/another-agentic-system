-- The agent's work has a shape (ADR 0025): an agent, a sub-agent it delegated to, the tools and
-- commands they ran. A step is logged as an `agent_step` event that carries its path, and the log
-- keeps a step's start, its end and a bounded number of updates. The one schema change is the
-- CHECK on events.kind. The ledger of open steps lives inside threads.job (a ledger without it
-- has none open), so it needs no column.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through, instead of blocking the log.
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
