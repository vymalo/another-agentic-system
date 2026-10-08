-- The tokens of each model call and a task's totals (ADR 0056, `docs/api/usage-v1.md`): one `model_usage`
-- event per call an agent reports, one `model_usage_total` when a task ends or pauses. Labels and numbers
-- only; the data is JSON like every event's, so the only schema change is the kinds.
--
-- One schema change:
--   events.kind   takes `model_usage` and `model_usage_total`
-- The CHECK is rebuilt with every kind there is, as the earlier ones were. An older build cannot decode a
-- log that has either, so roll out the build that understands them first, on every replica, before an
-- agent that lists `usage/v1` is deployed: nothing writes one until such an agent does.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared', 'model_usage', 'model_usage_total')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
