-- The suggestions offered under a job's finished answer (ADR 0035, amended 2026-11-28): one more
-- event kind and one more outbox kind, no row of its own.
--
--   thread_suggested   the thread's log: the job whose end asked, and at most three suggestions
--                      of at most 80 characters each. They fill the message box and never send
--                      (ADR 0013, rule 7). Job-scoped: nothing of the thread's row changes.
--   suggestions        the outbox: an ask of the utility model, worked like `title` and
--                      `description`.
--
-- No table and no column change; the only changes to the schema are the two CHECKs, rebuilt as
-- 0027, 0028 and 0029 did the events one: NOT VALID first so it takes no long lock, then VALIDATE.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'thread_suggested', 'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared', 'model_usage', 'model_usage_total',
    'child_session_opened', 'child_session_reported', 'child_session_closed',
    'child_session_start_asked', 'child_session_start_ended', 'child_session_close_asked',
    'child_session_state', 'trusted_request')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('delegate', 'cancel', 'verify', 'title', 'description', 'suggestions', 'steer', 'ask')) NOT VALID;
ALTER TABLE outbox VALIDATE CONSTRAINT outbox_kind_check;
