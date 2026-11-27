-- The claims of an outbox row that only waited (ANR-1078, ADR 0103). `attempts` counts every claim
-- and only goes up: it is the fencing token of the lease. The dispatcher's caps (the row's send
-- attempts, the agent's listing attempts, the cancel budget) and its backoff count the claims
-- that tried something, so a claim that ended in `wait_outbox` (a stop waiting for a delegation
-- still in flight) adds one here and `attempts - refunded` is what the caps read. The inbox has
-- had the same column since 0004.
--
--   refunded  claims given back; never more than `attempts`
--
-- A replica of an older build ignores the column, counts the waits and may dead-letter such a row
-- earlier than this build would: benign, the row ends as it ended before.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE outbox
    ADD COLUMN refunded integer NOT NULL DEFAULT 0;
