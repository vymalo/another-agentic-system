-- A person drives a child session (ADR 0072, S3 of #332): they start one from the parent, send to
-- it, steer, stop and close it. Four more event kinds, none a row of their own:
--
--   child_session_start_asked   the parent's log: what was asked of whom (never the words)
--   child_session_start_ended   the parent's log: the session opened, or the agent's words for why not
--   child_session_close_asked   the child's log: the person asked to close the session
--   child_session_state         the child's log: the session's state changed
--
-- The ledgers (`Job.session_starts`, `ChildLedger.close_asked`) are in the job's JSON, which reads a
-- ledger written before them as having none; the outbox rows for a start and a close are `steer`-lane
-- rows (`outbox.kind` is unchanged). So the only change to the schema is the check on `events.kind`,
-- rebuilt as 0027 did: NOT VALID first so it takes no long lock, then VALIDATE.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared', 'model_usage', 'model_usage_total',
    'child_session_opened', 'child_session_reported', 'child_session_closed',
    'child_session_start_asked', 'child_session_start_ended', 'child_session_close_asked',
    'child_session_state')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
