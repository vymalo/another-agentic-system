-- A thread's default model and reasoning effort (ADR 0086, `model-pick/v1`): what a message that
-- names none is sent with, in the shape of `orch_core::ModelPick`
-- (`{"model": "<alias>", "reasoningEffort": "<level>"}`, either member optional). NULL while the
-- thread has no default. It is the owner's setting on the thread row, like the pin and the archive
-- of `0016_thread_rail.sql`: no event records it, no fork or export copies it, and it is erased with
-- the thread. What reaches an agent is always the one pick a message recorded on its `user_message`.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN pick jsonb;
