-- ADR 0107 slice P3: claims, the inbox, the outbox and leases. Three functions, and nothing else
-- (the claims stay single statements of the store, which now also order what they return).
--
-- No function here names a column of a table in a SQL-standard body, so no later migration is
-- blocked from altering or dropping one (ADR 0107 decision 6): `orch_outbox_due_at` reads only its
-- arguments, and the other two are PL/pgSQL, which resolves its tables when it runs.

-- Is an outbox row due at `p_now`? (ADR 0066; ADR 0107 row 34.) A `pending` row is due once its
-- `next_attempt_at` has come, an `inflight` row once its lease has ended; a finished row never is.
-- The one definition: both claims and the statistics call it. `p_now` is the caller's: the function
-- never reads the database's clock (decision 3).
--
-- A SQL-standard `RETURN` body of one expression, so the planner puts the expression in place of the
-- call. The FIFO claim names no status in its due rows' `WHERE` beyond this call, so the partial
-- indexes on `status IN ('pending', 'inflight')` serve that scan only if the planner proves the
-- condition from the expression itself (the fair claim's candidate and cancel scans and the
-- statistics write it themselves): the function must stay inlinable (not STRICT, no `SET`, not
-- SECURITY DEFINER) and its body in a form the planner can prove it from (a `CASE` on the status is
-- not one). `tests/claims.rs` asks `EXPLAIN` for both.
CREATE FUNCTION orch_outbox_due_at(
    p_status text,
    p_next_attempt_at timestamptz,
    p_lease_until timestamptz,
    p_now timestamptz
)
    RETURNS boolean
    LANGUAGE sql
    IMMUTABLE PARALLEL SAFE
    RETURN (p_status = 'pending' AND p_next_attempt_at <= p_now)
        OR (p_status = 'inflight' AND p_lease_until <= p_now);

-- Adds the watches of a commit and re-arms the parked inbox rows they match (ADR 0016; ADR 0107
-- row 8), in the caller's transaction. One row comes back:
--
--   rearmed    whether a parked row became claimable;
--   held_keys  the keys that another thread watches already (a key stays with its first thread), in
--   held_by    the order they were locked in, and the thread that watches each. The caller logs them.
--
-- Each key is locked (a transaction-scoped advisory lock) before it is inserted, and
-- `orch_inbox_park` takes the same lock before it looks for the watch: of a commit adding a watch
-- and a worker parking a row for it, one goes first, and the second sees the first's committed
-- result. The keys are locked once each, in bytewise order (`COLLATE "C"`), whatever the database
-- collates by: two commits that add several watches then take them in the same order, and so does a
-- replica of an earlier release, which sorted them itself and takes the same lock. The lock's text
-- must stay what that release takes (a unit test of `src/store.rs` holds the two functions to it).
--
-- The re-arm locks the parked rows it changes in id order and skips a row that another transaction
-- holds, so it never waits for a row lock. A parked report was waiting, not failing: its claims so
-- far are refunded.
--
-- **The function relies on its caller's transaction being READ COMMITTED**, and cannot set that
-- itself: under REPEATABLE READ the snapshot is taken before the wait for a key, so the re-arm would
-- miss the row a park set aside meanwhile and leave it parked behind its watch. The store begins
-- every transaction that calls it with `BEGIN ISOLATION LEVEL READ COMMITTED`.
CREATE FUNCTION orch_watch_arm(p_thread uuid, p_keys text[], p_now timestamptz)
    RETURNS TABLE (rearmed boolean, held_keys text[], held_by uuid[])
    LANGUAGE plpgsql
    SET search_path FROM CURRENT
AS $$
DECLARE
    v_key text;
    v_holder uuid;
    v_held_keys text[] := '{}';
    v_held_by uuid[] := '{}';
    v_rearmed boolean;
BEGIN
    IF p_thread IS NULL OR p_keys IS NULL OR p_now IS NULL
        OR array_position(p_keys, NULL) IS NOT NULL
    THEN
        RAISE EXCEPTION 'orch_watch_arm: a thread, keys with no NULL among them and a time are required'
            USING ERRCODE = 'OR002';
    END IF;

    FOR v_key IN SELECT DISTINCT u.k COLLATE "C" FROM unnest(p_keys) AS u(k) ORDER BY 1 LOOP
        PERFORM pg_advisory_xact_lock(hashtextextended('orch:watch:' || v_key, 0));
        INSERT INTO watches (key, thread_id, created_at) VALUES (v_key, p_thread, p_now)
            ON CONFLICT (key) DO NOTHING;
        IF NOT FOUND THEN
            SELECT w.thread_id INTO v_holder FROM watches w WHERE w.key = v_key;
            IF v_holder <> p_thread THEN
                v_held_keys := v_held_keys || v_key;
                v_held_by := v_held_by || v_holder;
            END IF;
        END IF;
    END LOOP;

    WITH c AS (
        SELECT i.id FROM inbox i WHERE i.status = 'parked' AND i.correlation = ANY (p_keys)
        ORDER BY i.id FOR UPDATE SKIP LOCKED)
    UPDATE inbox SET status = 'pending', available_at = p_now, parked_at = NULL,
        refunded = attempts, updated_at = p_now
        WHERE id IN (SELECT id FROM c);
    v_rearmed := FOUND;

    RETURN QUERY SELECT v_rearmed, v_held_keys, v_held_by;
END;
$$;

-- Parks a claimed inbox row until its watch exists (ADR 0016; ADR 0107 row 42). One row comes back:
--
--   outcome  'parked', 'rearmed' (the watch is there by now: the row is `pending` again, due at
--            `p_now`) or 'lost' (the claim named is not the row's any more; nothing was written).
--
-- The correlation never changes, so it is read before the row is locked: the lock order is the watch
-- key, then the row. The watch is then looked for in a statement of its own, which in a VOLATILE
-- function under READ COMMITTED takes a snapshot after the lock was granted: a watch committed
-- before is seen, and one that commits after finds this row parked and re-arms it
-- (`orch_watch_arm`, whose key lock this is). Either way the claim ends without having failed: the
-- row's claims so far are refunded.
--
-- This does not make deadlock impossible: a stale park holds the key and may wait for the row, which
-- the commit of the worker that took the row over holds while it wants the key. PostgreSQL detects
-- the cycle (`40P01`), aborts one side, and the store reports a transient error the caller retries.
--
-- **The function relies on its caller's transaction being READ COMMITTED**, and cannot set that
-- itself: under REPEATABLE READ the snapshot is taken by the first read, before the wait for the
-- key, so the watch committed meanwhile would not be seen and the row would stay parked behind it.
-- The store begins that transaction with `BEGIN ISOLATION LEVEL READ COMMITTED`.
CREATE FUNCTION orch_inbox_park(p_inbox uuid, p_owner text, p_attempts integer, p_now timestamptz)
    RETURNS TABLE (outcome text)
    LANGUAGE plpgsql
    SET search_path FROM CURRENT
AS $$
DECLARE
    v_correlation text;
    v_watched boolean := false;
BEGIN
    IF p_inbox IS NULL OR p_owner IS NULL OR p_attempts IS NULL OR p_now IS NULL THEN
        RAISE EXCEPTION 'orch_inbox_park: a row, its claim''s owner and attempts, and a time are required'
            USING ERRCODE = 'OR002';
    END IF;

    SELECT i.correlation INTO v_correlation FROM inbox i
        WHERE i.id = p_inbox AND i.lease_owner = p_owner AND i.attempts = p_attempts
          AND i.status = 'inflight';
    IF NOT FOUND THEN
        RETURN QUERY SELECT 'lost'::text;
        RETURN;
    END IF;

    IF v_correlation IS NOT NULL THEN
        PERFORM pg_advisory_xact_lock(hashtextextended('orch:watch:' || v_correlation, 0));
        v_watched := EXISTS (SELECT 1 FROM watches w WHERE w.key = v_correlation);
    END IF;

    UPDATE inbox SET status = CASE WHEN v_watched THEN 'pending' ELSE 'parked' END,
        available_at = CASE WHEN v_watched THEN p_now ELSE available_at END,
        parked_at = CASE WHEN v_watched THEN NULL ELSE p_now END,
        refunded = attempts, lease_owner = NULL, lease_until = NULL, updated_at = p_now
        WHERE id = p_inbox AND lease_owner = p_owner AND attempts = p_attempts
          AND status = 'inflight';
    IF NOT FOUND THEN
        RETURN QUERY SELECT 'lost'::text;
        RETURN;
    END IF;

    RETURN QUERY SELECT CASE WHEN v_watched THEN 'rearmed' ELSE 'parked' END;
END;
$$;
