-- A thread is a conversation (ADR 0020): a message on a finished thread starts the thread's next
-- job, and the log marks it with a `job_started` event. The one schema change is the CHECK on
-- events.kind. `Job.number` lives inside threads.job (a ledger without it is job 1), and the
-- outbox's `cancel` payload gains a `job` inside its JSON (a row without one means the current
-- job), so neither needs a column.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started'));
