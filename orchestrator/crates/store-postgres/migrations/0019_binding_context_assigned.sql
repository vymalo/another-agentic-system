-- The A2A context of a thread is the one its agent assigned (ADR 0055). The first message to an agent
-- carries no context; the binding adopts the one the agent answers in, once.
--
-- One schema change:
--   a2a_bindings.context_id   may be NULL (no answer read yet)
-- Rows that exist keep what they have: their own thread id, the context their first message was sent
-- in, which is the one every later message of that thread goes on using. Only a thread created by a
-- build that has this migration starts with NULL.
--
-- Roll out the build that understands NULL on every replica before any of them creates a thread: an
-- older build fails to read a binding whose context is NULL (its decode wants a string).
--
-- Never edit an applied migration; add a new one.

ALTER TABLE a2a_bindings ALTER COLUMN context_id DROP NOT NULL;
