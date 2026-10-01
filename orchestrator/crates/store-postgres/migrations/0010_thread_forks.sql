-- A thread can begin as a copy of another's log (ADR 0029): "fork from here" and an edited message.
-- The copy is the events 1..=forked_at of the parent, then a `thread_forked` event, then the thread's
-- own. The thread row keeps where it came from, so a list and a sidebar need not read the log:
--   forked_from  the parent; NULL once the parent is deleted (the fork is whole, its events are its own)
--   forked_at    the last event copied (the cut), 0 when none was
--   fork_kind    `fork` (a root of its own) or `edit` (a sibling of the parent, a branch)
-- A thread that was not forked has all three NULL.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN forked_from uuid REFERENCES threads (id) ON DELETE SET NULL;
ALTER TABLE threads ADD COLUMN forked_at bigint;
ALTER TABLE threads ADD COLUMN fork_kind text;

ALTER TABLE threads ADD CONSTRAINT threads_fork_shape CHECK (
    (forked_at IS NULL) = (fork_kind IS NULL)
    AND (forked_from IS NULL OR forked_at IS NOT NULL)
    AND (forked_at IS NULL OR forked_at >= 0)
    AND (fork_kind IS NULL OR fork_kind IN ('fork', 'edit'))) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through.
ALTER TABLE threads VALIDATE CONSTRAINT threads_fork_shape;

-- The children of a thread (the edits of a family, and a parent's deletion that sets the column null).
CREATE INDEX threads_forked_from ON threads (forked_from) WHERE forked_from IS NOT NULL;

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
