-- ADR 0107 slice P2: admission (ADR 0081). One function, and nothing else.
--
-- `orch_admit` names no column in a SQL-standard body (it is PL/pgSQL, which resolves its tables when
-- it runs), so no later migration is blocked from altering one (ADR 0107 decision 6).

-- May a person start one more job? (ADR 0081, decisions 1 to 4; ADR 0107 row 11.) One row comes back:
--
--   outcome  'admitted', or 'no_room' when the person's unfinished jobs are at or above `p_limit`.
--            A person at the limit is an answer and not an error (ADR 0107 decision 2).
--   running  the person's unfinished jobs, in both cases: `delegate` rows of the outbox that are
--            `pending` or `inflight`.
--
-- The lock comes first and the count follows it. The count is a statement of its own in a VOLATILE
-- function, so under READ COMMITTED it takes a snapshot of its own, after the lock was granted: it
-- sees the job of whoever held the lock before and committed. A count taken before the lock, or in
-- the same statement as it, could miss that job and admit two people's worth into one place.
--
-- The lock is transaction-scoped: it lasts until the caller's transaction ends. The caller runs its
-- whole commit in that same transaction, so the admission and the commit that adds the job are one
-- decision, and the next admission of this person counts that job.
--
-- Its first key is 1633971572, `0x61646d74` ("admt"): `ADMISSION_LOCK_CLASS` of `src/store.rs`, apart
-- from the prompt library's and the others'. It must stay the key an older build takes in its own
-- statement, or its replicas and these would not exclude each other during a rollout.
CREATE FUNCTION orch_admit(p_owner text, p_limit integer)
    RETURNS TABLE (outcome text, running integer)
    LANGUAGE plpgsql
    SET search_path FROM CURRENT
AS $$
DECLARE
    v_running integer;
BEGIN
    IF p_owner IS NULL OR p_owner = '' OR p_limit IS NULL OR p_limit < 0 THEN
        RAISE EXCEPTION 'orch_admit: an owner and a limit of zero or more are required'
            USING ERRCODE = 'OR002';
    END IF;

    PERFORM pg_advisory_xact_lock(1633971572, hashtext(p_owner));

    SELECT least(count(*), 2147483647)::integer INTO v_running
        FROM outbox
        WHERE owner = p_owner AND kind = 'delegate' AND status IN ('pending', 'inflight');

    RETURN QUERY SELECT
        CASE WHEN v_running >= p_limit THEN 'no_room' ELSE 'admitted' END,
        v_running;
END;
$$;
