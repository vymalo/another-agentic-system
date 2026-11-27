-- ANR-1175: the fair claim ranks a person's due rows by next_attempt_at, then ord (it was ord
-- alone, `outbox_open_owner`), so that a routed row that only waited, and was put back due later,
-- does not outrank the person's newer work. The index has the columns of that order, so the claim
-- reads the first `4 * limit` open rows of a person and stops, as it did. `outbox_open_owner` stays:
-- the FIFO claim and the open-row listings read it.
CREATE INDEX outbox_open_owner_due ON outbox (owner, next_attempt_at, ord)
    WHERE status IN ('pending', 'inflight');
