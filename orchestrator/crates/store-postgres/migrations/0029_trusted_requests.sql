-- An agent asks the client to draw a trusted component (ADR 0083, `trusted-components/v1`): one
-- more event kind, no row of its own.
--
--   trusted_request   the thread's log: the component asked for and the agent's bounded hints
--
-- The request is data and nothing else: the client draws the component, the agent never does, and
-- the secret is the client's own state, never in the event. No table and no column change; the only
-- change to the schema is the check on `events.kind`, rebuilt as 0027 and 0028 did: NOT VALID first
-- so it takes no long lock, then VALIDATE.
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
    'child_session_state', 'trusted_request')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
