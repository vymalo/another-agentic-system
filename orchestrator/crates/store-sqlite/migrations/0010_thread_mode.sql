-- A thread's default mode (ADR 0109), in SQLite: the counterpart of Postgres's
-- `0041_thread_mode.sql`, the mode's own name in a TEXT column, NULL while the thread has none.
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN mode TEXT;
