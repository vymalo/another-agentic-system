-- A thread can be shared by a revocable link (ADR 0040): `private` (every thread starts so, a fork too),
-- `internal` (signed-in people who have the link) or `public` (anybody who has it). What the row keeps:
--   visibility   `private` (the default), `internal` or `public`
--   share_nonce  the 16 random bytes the link is built on, NULL while the thread is private; the link
--                itself is that nonce plus a MAC under the deployment's secret, which is not stored
--   shared_at    when the share was last set (a new link and a change of level included)
-- and the constraint that says a thread is private exactly when it has no nonce. Revoking clears the
-- nonce and sets `private`; sharing again draws a new nonce, so a revoked link never comes back.
-- The index is how a link is found: the nonce is unique across threads, and only a shared thread has one.
-- The log keeps the audit trail (`thread_shared` with the SHA-256 of the nonce, `thread_unshared`) and
-- never the nonce, because an export carries the log. A thread table rebuilt from the log alone would
-- have every thread private again: sharing fails closed.
--
-- Two schema changes:
--   threads       takes `visibility`, `share_nonce` and `shared_at`
--   events.kind   takes `thread_shared` and `thread_unshared`
-- The CHECK is rebuilt with every kind there is, as the earlier ones were. A `thread_shared` event is
-- the only thing an older build cannot read (it fails the decode of the log), so roll out the build that
-- understands it first, on every replica, before any deployment sets `sharing.mode` above `disabled`:
-- nothing writes one until it does. Every thread that exists is `private` after this migration.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN visibility text NOT NULL DEFAULT 'private'
    CHECK (visibility IN ('private', 'internal', 'public'));
ALTER TABLE threads ADD COLUMN share_nonce bytea;
ALTER TABLE threads ADD COLUMN shared_at timestamptz;

ALTER TABLE threads ADD CONSTRAINT threads_share_shape CHECK (
    (visibility = 'private') = (share_nonce IS NULL)
    AND (share_nonce IS NULL) = (shared_at IS NULL)
    AND (share_nonce IS NULL OR length(share_nonce) = 16)) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through.
ALTER TABLE threads VALIDATE CONSTRAINT threads_share_shape;

CREATE UNIQUE INDEX threads_share_nonce ON threads (share_nonce) WHERE share_nonce IS NOT NULL;

ALTER TABLE events DROP CONSTRAINT events_kind_check;
ALTER TABLE events ADD CONSTRAINT events_kind_check CHECK (kind IN (
    'user_message', 'agent_message', 'agent_status', 'artifact', 'thread_state', 'error',
    'ui_surface', 'ui_action', 'ci_result', 'check_result', 'rework', 'job_started',
    'ui_catalog', 'agent_step', 'thread_titled', 'thread_forked', 'thread_described',
    'tools_attached', 'tools_detached', 'ask_started', 'ask_finished',
    'thread_shared', 'thread_unshared')) NOT VALID;
ALTER TABLE events VALIDATE CONSTRAINT events_kind_check;
