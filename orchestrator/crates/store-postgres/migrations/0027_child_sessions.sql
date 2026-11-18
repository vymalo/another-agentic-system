-- Child sessions (ADR 0072, `docs/orchestrator.md` "Child sessions"): a helper an agent opens as a session
-- is a thread of its own, a child of the thread that asked. What the schema gains:
--   threads.child_of        the parent thread, NULL for every thread that is not a child. REFERENCES with
--                           NO ACTION on purpose: a parent is never deleted from under its children
--                           (`delete_threads` erases them in the same statement and refuses a set that
--                           leaves one behind), unlike `rail_parent`, which only groups the list
--   threads.child_session   the session's id, opaque text the agent chose
--   child_session is 1 to 512 characters (`threads_child_session_len`)
--   the two come together (a row with one and not the other is corrupt), and a thread is not its own child
--   threads_child           at most one child of a parent for a session: the one thing that makes two
--                           replicas that learn of the same session at once make one thread
--   events.kind             takes `child_session_opened`, `child_session_reported` and
--                           `child_session_closed`
-- No new table: what the parent knows of its sessions and what a child knows of its own are in the `job`
-- column, which is JSON. The kinds are rebuilt in the CHECK with every kind there is, as the earlier
-- migrations did (NOT VALID, then VALIDATE).
--
-- Locks: sqlx runs the whole file in one transaction, so the ACCESS EXCLUSIVE locks that DROP CONSTRAINT
-- and ALTER TABLE take on `events` and `threads` are held until it commits, through VALIDATE's scan of
-- `events` and the build of the unique index on `threads`. Reads and writes of both tables wait for the
-- duration, which grows with the size of the log. That is acceptable at this stage; on a large database
-- run the migration in a maintenance window (stop the replicas, then start one).
--
-- Rollout order: roll out every replica of the build that has this migration before any agent that lists
-- `child-sessions/v1` is deployed. A build that does not know the three kinds cannot decode a log that
-- has one, and its `delete_threads` does not know to take the children with a parent (the foreign key
-- refuses the delete instead). Nothing writes a child, or one of the kinds, until such an agent does.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN child_of uuid REFERENCES threads (id);
ALTER TABLE threads ADD COLUMN child_session text;
ALTER TABLE threads ADD CONSTRAINT threads_child_pair CHECK ((child_of IS NULL) = (child_session IS NULL));
ALTER TABLE threads ADD CONSTRAINT threads_child_session_len CHECK (length(child_session) BETWEEN 1 AND 512);
ALTER TABLE threads ADD CONSTRAINT threads_child_not_self CHECK (child_of IS NULL OR child_of <> id);
CREATE UNIQUE INDEX threads_child ON threads (child_of, child_session) WHERE child_of IS NOT NULL;

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_reasoning', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared', 'model_usage', 'model_usage_total',
    'child_session_opened', 'child_session_reported', 'child_session_closed')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
