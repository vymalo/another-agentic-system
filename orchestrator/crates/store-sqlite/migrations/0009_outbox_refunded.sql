-- The claims of an outbox row that only waited, the SQLite form of Postgres migration 0034 (see it
-- for what the column means): `attempts - refunded` is what the dispatcher's caps read, and
-- `attempts`, the fencing token, only goes up.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE outbox
    ADD COLUMN refunded INTEGER NOT NULL DEFAULT 0;
