-- A thread's default mode (ADR 0109, `agent-mode/v1`): the mode a message that names none is sent
-- with, when a new task starts. The mode's own name (a non-blank string of at most 64 bytes with no
-- control character), NULL while the thread has no default. It is the owner's setting on the thread
-- row, like the pin and the archive of `0016_thread_rail.sql`: no event records it, no fork or
-- export copies it, and it is erased with the thread. What reaches an agent is always the one mode a
-- message recorded on its `user_message`.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN mode text;
