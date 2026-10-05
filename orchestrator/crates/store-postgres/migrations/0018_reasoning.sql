-- An agent's model can think before it answers, and the log keeps what it thought (ADR 0044): one
-- `agent_reasoning` event per stream of reasoning, bounded (32 KiB of text) and marked when it was cut.
--
-- One schema change:
--   events.kind   takes `agent_reasoning`
-- The CHECK is rebuilt with every kind there is, as the earlier ones were. An `agent_reasoning` event is
-- the only thing an older build cannot read (it fails the decode of the log), so roll out the build that
-- understands it first, on every replica, before an agent that sends reasoning (adam-rs ADR 0020) is
-- deployed: nothing writes one until such an agent does.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
