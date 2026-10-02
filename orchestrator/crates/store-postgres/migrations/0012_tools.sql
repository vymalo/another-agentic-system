-- MCP servers can be attached to a thread (ADR 0024, docs/api/thread-tools-v1.md): a person picks
-- servers from the deployment's list for a conversation and detaches them again. Each change is
-- logged as a `tools_attached` or a `tools_detached` event, whose data is `{"servers": [ids]}`:
-- ids only, never a URL or a credential.
--
-- One schema change:
--   events.kind   takes `tools_attached` and `tools_detached`
-- The set itself (the ids, sorted, carried from job to job) lives inside threads.job, like the
-- title's and the description's ledgers, so it needs no column of its own; and no outbox kind is
-- added, because attaching writes no delegation (the next message carries the set to the agent).
-- An event of the new kinds is the only thing an older build cannot read, so roll out the build
-- that understands them first.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through, instead of blocking the log.
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
