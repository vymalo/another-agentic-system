-- A2UI (ADR 0013): two additive event kinds. `ui_surface` is what an agent sent to render,
-- `ui_action` is what the user did on it. Widening a CHECK is the whole migration: the data is
-- jsonb, and the outbox needs nothing (an action is a `delegate` row with another payload).
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action'));
