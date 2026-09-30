-- The job ledger and the verification gate (ADR 0016, ADR 0018), the storage half.
--
--  * threads.job: the ledger, written in the same commit as `state`, under the same `version`
--    compare-and-swap. `{}` is a job with no gate (the core reads it as its defaults), so every
--    existing thread keeps behaving as before.
--  * threads.state gains 'verifying'; events.kind gains 'ci_result', 'check_result' and
--    'rework'. Every CHECK the gate needs is widened here, once.
--  * outbox.kind gains 'verify' (a request to the verifier agent) and outbox.task_id (the
--    verifier's A2A task, nullable: delegate and cancel rows keep the task on the binding).
--
-- Never edit an applied migration; add a new one. Migration 0004 belongs to the inbox.

ALTER TABLE threads ADD COLUMN job jsonb NOT NULL DEFAULT '{}';

ALTER TABLE threads DROP CONSTRAINT threads_state_check;
ALTER TABLE threads ADD CONSTRAINT threads_state_check CHECK (state IN (
    'queued', 'working', 'verifying', 'blocked', 'done', 'failed', 'cancelled'));

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework'));

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check CHECK (kind IN ('delegate', 'cancel', 'verify'));
ALTER TABLE outbox ADD COLUMN task_id text;
