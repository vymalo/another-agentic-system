-- A message sent while an agent works can be sent into the agent's running task (ADR 0036,
-- docs/api/steer-v1.md): the application writes an outbox row of the new kind `steer` for it, and the
-- dispatcher sends it with the `steer/v1` extension when the agent's live card lists it. A steer is
-- claimed beside the delegation that is in flight (its own lane: an older open `steer` row of the
-- thread is the only thing it waits for), and a steer the agent cannot take is rewritten in place as a
-- `delegate` row, which keeps its position in the thread's order. The payload of the new kind is
-- `{"steer": {"text": "...", "release": null}}` (and `ui_catalog` when the thread has one).
--
-- One schema change:
--   outbox.kind   takes `steer`
-- The CHECK is rebuilt with every kind there is, as the earlier ones were. A row of the new kind is
-- the only thing an older build cannot read (it fails the decode and dead-letters the row), so roll
-- out the build that understands it first, on every replica, before a person can send while an agent
-- works.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE outbox DROP CONSTRAINT outbox_kind_check;
ALTER TABLE outbox ADD CONSTRAINT outbox_kind_check
    CHECK (kind IN ('delegate', 'cancel', 'verify', 'title', 'description', 'steer')) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through.
ALTER TABLE outbox VALIDATE CONSTRAINT outbox_kind_check;
