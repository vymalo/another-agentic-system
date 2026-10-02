-- A thread can have a description: a sentence or two on what the conversation is about now (ADR 0035).
-- The model writes one when a job ends or pauses for the person, and a person can write or clear it;
-- either is logged as a `thread_described` event, which says who wrote it and what it is, and
-- `threads.description` takes the value in the same transaction, so a listing need not read the log.
-- Whose description the thread has (nobody's, the model's, a person's) lives inside threads.job, like
-- the title's, so it needs no column of its own.
--
-- Three schema changes:
--   threads.description   the thread's description, NULL when it has none (never written, or a person
--                         cleared it); at most 500 characters, the limit of a person's edit
--   events.kind           takes `thread_described`
--   outbox.kind           takes `description`, the model's request: like a `title`, a `cancel` or a
--                         `verify` it is claimable whatever the thread's older delegations, and its
--                         payload is `{"description": {"job": n}}`
-- A row of the new kind is the only thing an older build cannot read, so roll out the build that
-- understands it first; a build that finds one it does not know fails its decode and dead-letters it.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN description text;
ALTER TABLE threads ADD CONSTRAINT threads_description_len
    CHECK (description IS NULL OR (description <> '' AND char_length(description) <= 500)) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit (there are none with a description) runs under a lock that lets reads and writes through.
ALTER TABLE threads VALIDATE CONSTRAINT threads_description_len;

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('delegate', 'cancel', 'verify', 'title', 'description')) NOT VALID;
ALTER TABLE outbox VALIDATE CONSTRAINT outbox_kind_check;
