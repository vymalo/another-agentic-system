-- The thread list is its owner's (ADR 0042): a person pins, archives, orders and nests their threads.
-- What the row keeps, and no event records (the log is the conversation: it is copied into forks,
-- carried by an export and read by the people a thread is shared with, none of whom may see or
-- inherit the owner's list):
--   pinned_at     when the owner pinned the thread, NULL while it is not
--   archived_at   when the owner archived it, NULL while it is not
--   rail_parent   the thread this one is nested under, one level deep (a fork under the thread it was
--                 made from, ADR 0042 decision 3); NULL for a thread of the list's own. The nesting is
--                 display grouping: the lineage stays in `forked_from`. ON DELETE SET NULL, as
--                 `forked_from` is: a fork stands alone when its parent goes
--   rail_rank     a fractional key (orch_core::rank: `0-9a-z`, no key ends in `0`, at most 128
--                 characters), compared bytewise, hence COLLATE "C": the place of a top-level thread
--                 among its owner's. One key space per owner; ties are broken by id (newest first)
-- A thread table rebuilt from the log alone would have nothing pinned, archived or nested and every
-- thread in the order of its id, newest first: the list fails safe.
--
-- The backfill keeps today's list: ranks by id, newest first (the order `GET /api/threads` has always
-- had), twelve hex digits and an `h` so that no key ends in `0` and every key has room before and
-- after it; and the nesting a fork would have been made with: under the thread it was forked from,
-- one level deep (a fork of a nested fork under the same root; an edit branch's fork under the root
-- of its family of edits, which the list shows), only when that thread is the same owner's.
--
-- No event kind changes, so no rollout order is needed. Never edit an applied migration; add a new
-- one.

ALTER TABLE threads ADD COLUMN pinned_at timestamptz;
ALTER TABLE threads ADD COLUMN archived_at timestamptz;
ALTER TABLE threads ADD COLUMN rail_parent uuid REFERENCES threads (id) ON DELETE SET NULL;
ALTER TABLE threads ADD COLUMN rail_rank text COLLATE "C";

UPDATE threads t SET rail_rank = r.rank
FROM (
    SELECT id, lpad(to_hex(row_number() OVER (PARTITION BY owner ORDER BY id DESC)), 12, '0') || 'h' AS rank
    FROM threads) r
WHERE t.id = r.id;

ALTER TABLE threads ALTER COLUMN rail_rank SET NOT NULL;

-- Oldest first, so that the thread a fork was made from has its own place by the time the fork is
-- read: a fork of a fork lands under the root, not under the fork.
DO $$
DECLARE
    fork record;
    seen record;
BEGIN
    FOR fork IN
        SELECT id, owner, forked_from FROM threads
        WHERE fork_kind = 'fork' AND forked_from IS NOT NULL
        ORDER BY created_at, id
    LOOP
        -- the thread the person sees: the parent, or for an edit branch the root of its family
        WITH RECURSIVE up AS (
            SELECT id, owner, forked_from, fork_kind, rail_parent, 0 AS depth
            FROM threads WHERE id = fork.forked_from
            UNION ALL
            SELECT p.id, p.owner, p.forked_from, p.fork_kind, p.rail_parent, up.depth + 1
            FROM threads p JOIN up ON p.id = up.forked_from AND up.fork_kind = 'edit'
            WHERE up.depth < 1000)
        SELECT * INTO seen FROM up ORDER BY depth DESC LIMIT 1;
        -- (an edit branch that is the root of its family, its own parent gone, is not shown either)
        IF seen.id IS NOT NULL AND seen.owner = fork.owner AND seen.fork_kind IS DISTINCT FROM 'edit' THEN
            UPDATE threads SET rail_parent = COALESCE(seen.rail_parent, seen.id) WHERE id = fork.id;
        END IF;
    END LOOP;
END $$;

ALTER TABLE threads ADD CONSTRAINT threads_rail_shape CHECK (rail_parent IS DISTINCT FROM id) NOT VALID;
-- NOT VALID takes the table lock only for the catalogue change; the scan that proves the old rows
-- fit runs under a lock that lets reads and writes through.
ALTER TABLE threads VALIDATE CONSTRAINT threads_rail_shape;

-- The list: an owner's top-level threads by rank, and the children of a thread.
CREATE INDEX threads_rail ON threads (owner, rail_rank) WHERE rail_parent IS NULL;
CREATE INDEX threads_rail_parent ON threads (rail_parent) WHERE rail_parent IS NOT NULL;
