-- The orchestrator can ask a language model for the title of a thread: the request is an outbox
-- row of the new kind `title`, written in the commit of the agent's first reply (ADR 0005, MVP
-- slice 6). Like a `cancel` or a `verify` it is claimable whatever the thread's older delegations,
-- and its payload is `{"title": {"ask": n}}`. The one schema change is the CHECK on outbox.kind.
-- A row of this kind is the only thing an older build cannot read, so roll out the build that
-- understands it first; a build that finds one it does not know fails its decode and dead-letters it.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('delegate', 'cancel', 'verify', 'title')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through.
ALTER TABLE outbox VALIDATE CONSTRAINT outbox_kind_check;
