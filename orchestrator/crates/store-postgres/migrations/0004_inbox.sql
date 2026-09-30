-- The inbox, the watches and the timers (ADR 0016), the storage half.
--
--  * inbox: unsolicited machine input. A webhook report is stored as it arrives (deduplicated by
--    its sender's delivery id) and applied to a thread by a worker; a timer is a row with
--    source 'timer' whose available_at is its due time, so the core never reads a clock.
--  * watches: which thread a correlation key (`ci:<repo-key>@<sha>`) belongs to. A commit that
--    inserts a watch also re-arms the parked inbox rows with that correlation, in the same
--    transaction.
--
-- Never edit an applied migration; add a new one.

CREATE TABLE watches (
    key        text PRIMARY KEY,
    thread_id  uuid NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL
);
CREATE INDEX watches_thread ON watches (thread_id);

CREATE TABLE inbox (
    id              uuid PRIMARY KEY,
    source          text NOT NULL,
    -- The sender's id of the delivery (or, for a timer, thread + timer + attempt +
    -- verification): a repeat of it is one row.
    idempotency_key text NOT NULL,
    kind            text NOT NULL CHECK (kind IN ('timer', 'ci_report')),
    payload         jsonb NOT NULL,
    -- The watch key that says which thread the row is about; null when the payload names it.
    correlation     text,
    status          text NOT NULL CHECK (status IN ('pending', 'inflight', 'parked', 'applied', 'expired', 'dead')),
    -- Earliest claim: a timer's due time, a re-armed row's re-arm time, a retry's backoff.
    available_at    timestamptz NOT NULL,
    -- Claims so far: the fencing token of a claim, so it only goes up.
    attempts        integer NOT NULL DEFAULT 0,
    -- Claims that do not count against the attempt limit (released at shutdown; up to the one
    -- that parked the row). attempts - refunded is what the worker limits.
    refunded        integer NOT NULL DEFAULT 0,
    lease_owner     text,
    lease_until     timestamptz,
    -- When the row was parked; its time-to-live counts from here.
    parked_at       timestamptz,
    last_error      text,
    created_at      timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL,
    UNIQUE (source, idempotency_key)
);
-- The claim query: pending rows in due order, and inflight rows whose lease lapsed.
CREATE INDEX inbox_pending ON inbox (available_at, id) WHERE status = 'pending';
CREATE INDEX inbox_inflight ON inbox (lease_until) WHERE status = 'inflight';
-- Re-arming (by correlation) and expiry (by age) look only at parked rows.
CREATE INDEX inbox_parked_correlation ON inbox (correlation) WHERE status = 'parked';
CREATE INDEX inbox_parked_at ON inbox (parked_at) WHERE status = 'parked';
