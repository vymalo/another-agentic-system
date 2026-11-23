-- A thread's default model and reasoning effort (ADR 0086), in SQLite: the counterpart of
-- Postgres's `0031_thread_pick.sql`, the same JSON in a TEXT column, NULL while the thread has none.
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN pick TEXT;
