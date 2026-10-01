-- A person can rename a thread: the rename is logged as a `thread_titled` event, which says who
-- wrote the title and what it is, and `threads.title` takes the new value in the same transaction.
-- The one schema change is the CHECK on events.kind. Whose title the thread has (the first
-- message's words, or a person's) lives inside threads.job (a ledger without it has the first
-- message's words), so it needs no column.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through, instead of blocking the log.
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
