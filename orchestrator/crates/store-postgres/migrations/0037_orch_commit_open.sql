-- ADR 0107 slice P1: the event log's append. Two functions, and nothing else (the append itself is
-- one statement of the store, under the lock the first function takes).
--
-- No function here names a column of a table in a SQL-standard body, so no later migration is
-- blocked from altering or dropping one (ADR 0107 decision 6): `orch_commit_open` is PL/pgSQL, which
-- resolves its tables when it runs, and `orch_usage_counted_through` reads only its arguments.

-- How far the usage ledger has counted a thread's log (ADR 0065): the thread's watermark, or the cut
-- of a fork when that is later (a fork's copied events were counted in its parent). The one
-- definition: the commit reads it from `orch_commit_open`, the backfill's due-selection and its
-- per-thread judgement call it. `orch_core::usage_counted_through` stays for the in-memory and SQLite
-- stores. A SQL-standard `RETURN` body: plain SQL, no control flow.
CREATE FUNCTION orch_usage_counted_through(p_usage_through bigint, p_forked_at bigint)
    RETURNS bigint
    LANGUAGE sql
    IMMUTABLE PARALLEL SAFE
    RETURN greatest(p_usage_through, coalesce(p_forked_at, 0));

-- The opening of a commit (ADR 0107 rows 1 and 13): locks, in the order the store has always taken
-- them (the thread `FOR UPDATE`, then the claimed outbox row `FOR SHARE`, then the claimed inbox row
-- `FOR UPDATE`; nothing takes a thread lock while it holds an outbox row lock), and decides, before
-- anything is written, whether the commit may go on. One row comes back:
--
--   outcome  'applied' (go on, the locks are held until the caller's transaction ends), 'not_found',
--            'fenced' (a claim the commit names is not held by it any more), 'version_conflict'
--            (the thread's version is not `p_expected_version`) or 'duplicate' (one of `p_keys` is
--            already in the thread's log). The first that applies wins, in that order.
--   cur_*    the locked thread's values the commit goes on from (NULL for 'not_found');
--            `cur_counted_through` is `orch_usage_counted_through` of the row.
--
-- A lease is present when its id is not NULL; its owner and attempts are then required. The caller
-- rolls back on any outcome but 'applied'. PL/pgSQL, for the lock order, the early return and the
-- `RAISE` of an `OR` SQLSTATE: `OR002` when the arguments contradict each other (a caller's bug,
-- raised before a lock is taken).
CREATE FUNCTION orch_commit_open(
    p_thread uuid,
    p_expected_version bigint,
    p_outbox_id uuid,
    p_outbox_owner text,
    p_outbox_attempts integer,
    p_inbox_id uuid,
    p_inbox_owner text,
    p_inbox_attempts integer,
    p_keys text[]
)
    RETURNS TABLE (
        outcome text,
        cur_version bigint,
        cur_last_seq bigint,
        cur_state text,
        cur_owner text,
        cur_counted_through bigint,
        cur_search_through bigint,
        cur_search_fold integer,
        cur_title text,
        cur_description text
    )
    LANGUAGE plpgsql
AS $$
DECLARE
    t threads%ROWTYPE;
    v_outcome text := 'applied';
BEGIN
    IF (p_outbox_id IS NULL) <> (p_outbox_owner IS NULL)
        OR (p_outbox_id IS NULL) <> (p_outbox_attempts IS NULL)
        OR (p_inbox_id IS NULL) <> (p_inbox_owner IS NULL)
        OR (p_inbox_id IS NULL) <> (p_inbox_attempts IS NULL)
        OR p_thread IS NULL OR p_expected_version IS NULL
    THEN
        RAISE EXCEPTION 'orch_commit_open: a lease needs its id, owner and attempts together'
            USING ERRCODE = 'OR002';
    END IF;

    SELECT * INTO t FROM threads WHERE id = p_thread FOR UPDATE;
    IF NOT FOUND THEN
        RETURN QUERY SELECT 'not_found'::text, NULL::bigint, NULL::bigint, NULL::text, NULL::text,
            NULL::bigint, NULL::bigint, NULL::integer, NULL::text, NULL::text;
        RETURN;
    END IF;

    IF p_outbox_id IS NOT NULL THEN
        -- FOR SHARE keeps the claim in place until the transaction ends: a claimer's UPDATE waits
        -- (or, with SKIP LOCKED, passes the row by), so the check cannot go stale before the write.
        PERFORM 1 FROM outbox
            WHERE id = p_outbox_id AND thread_id = p_thread AND lease_owner = p_outbox_owner
              AND attempts = p_outbox_attempts AND status = 'inflight'
            FOR SHARE;
        IF NOT FOUND THEN
            v_outcome := 'fenced';
        END IF;
    END IF;
    IF v_outcome = 'applied' AND p_inbox_id IS NOT NULL THEN
        -- FOR UPDATE: the row is marked applied by the commit, so its claim must not move before
        -- the transaction ends.
        PERFORM 1 FROM inbox
            WHERE id = p_inbox_id AND lease_owner = p_inbox_owner
              AND attempts = p_inbox_attempts AND status = 'inflight'
            FOR UPDATE;
        IF NOT FOUND THEN
            v_outcome := 'fenced';
        END IF;
    END IF;
    IF v_outcome = 'applied' AND t.version <> p_expected_version THEN
        v_outcome := 'version_conflict';
    END IF;
    IF v_outcome = 'applied' AND coalesce(cardinality(p_keys), 0) > 0 THEN
        PERFORM 1 FROM events
            WHERE thread_id = p_thread AND idempotency_key = ANY (p_keys);
        IF FOUND THEN
            v_outcome := 'duplicate';
        END IF;
    END IF;

    RETURN QUERY SELECT v_outcome, t.version, t.last_seq, t.state, t.owner,
        orch_usage_counted_through(t.usage_through, t.forked_at), t.search_through, t.search_fold,
        t.title, t.description;
END;
$$;
