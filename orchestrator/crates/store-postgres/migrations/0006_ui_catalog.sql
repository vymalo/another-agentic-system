-- The UI's component catalog is part of the conversation (ADR 0023): the web sends a version of it
-- with the first run of a thread, and again when its own version is newer than the thread's, and
-- the log records each digest once as a `ui_catalog` event. The one schema change is the CHECK on
-- events.kind. The thread's catalog ledger lives inside threads.job (a ledger without it has
-- seen no catalog), and the delivery to the agent inside the outbox payload (a row without one
-- tells the agent nothing of the screen), so neither needs a column.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through, instead of blocking the log.
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
