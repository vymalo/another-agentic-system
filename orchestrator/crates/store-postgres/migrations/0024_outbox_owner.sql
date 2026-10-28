-- A fair share of the dispatcher for each person (ADR 0066), the storage half.
--
-- The fair claim ranks the open rows of each person. A row's person is the owner of its thread, and
-- reading it through `threads` meant ranking every open row of the table on each claim (measured on
-- 10 000 open rows: a median of 168 ms against 3 ms for the oldest-first claim). With the owner on
-- the row, an index serves "the first rows of this person, oldest first" and a claim reads a few
-- rows of each person whatever the length of the queues.
--
--  * outbox.owner     the person (`UserId`, as `threads.owner`, which is fixed at creation), kept on
--                     the rows that can still be claimed. NULL on the rows that are over (delivered,
--                     dead, skipped), which nothing reads by owner and which are not rewritten here.
--  * a BEFORE INSERT trigger fills it from the thread when the insert gives none, so a replica of an
--    older build that is still rolling out writes rows the new claim finds, and no statement of
--    this crate has to remember it.
--  * outbox_open_has_owner   an open row has an owner (a row without one would never be found).
--  * outbox_open_owner       the open rows of a person, oldest first: the fair claim.
--  * outbox_inflight_owner   the rows a person has running, by lease: their load.
--  * outbox_open_cancel      the open cancel rows, oldest first: they are served before the load.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE outbox ADD COLUMN owner text;

UPDATE outbox o SET owner = t.owner
FROM threads t
WHERE t.id = o.thread_id AND o.status IN ('pending', 'inflight');

CREATE FUNCTION outbox_set_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.owner IS NULL THEN
        NEW.owner := (SELECT t.owner FROM threads t WHERE t.id = NEW.thread_id);
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER outbox_set_owner BEFORE INSERT ON outbox
    FOR EACH ROW EXECUTE FUNCTION outbox_set_owner();

-- NOT VALID: the UPDATE above gave every open row its owner, and the rows that are over may have
-- none, so there is nothing to check in what is there; the check holds for every row written from
-- now on. Validating would read the whole outbox, which keeps every row it ever had, under the lock
-- ADD COLUMN took.
ALTER TABLE outbox ADD CONSTRAINT outbox_open_has_owner
    CHECK (owner IS NOT NULL OR status NOT IN ('pending', 'inflight')) NOT VALID;

CREATE INDEX outbox_open_owner ON outbox (owner, ord) WHERE status IN ('pending', 'inflight');
CREATE INDEX outbox_inflight_owner ON outbox (owner, lease_until) WHERE status = 'inflight';
CREATE INDEX outbox_open_cancel ON outbox (ord)
    WHERE kind = 'cancel' AND status IN ('pending', 'inflight');
