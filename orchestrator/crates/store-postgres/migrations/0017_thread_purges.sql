-- Deleting a thread erases it (ADR 0043): the promise that its files will go.
--
-- A delete is one transaction that removes the thread's row (which cascades its events, outbox rows,
-- binding and watches, and takes the share nonce with it) and the timer rows of the inbox that name
-- it, and writes one row here per thread. The files of the thread live in the artifact store, outside
-- Postgres, so they cannot go in that transaction: the API erases them right after it (the inline
-- purge) and removes the row; when that fails, or the process dies between the two, the row stays and
-- the purge sweep of a worker finishes it. So there is never a thread gone with no row to finish it,
-- and the log goes before the files: nothing can reference a file that is missing.
--
-- A row is deleted when it is finished (the table has no finished column, and the gauge
-- `thread_purges_pending` is its row count). No foreign key on purpose: the thread it names is gone.
--
--   thread_id    the deleted thread; its files are `threads/<thread_id>/` in the artifact store
--   deleted_at   when the thread was deleted, the order the sweep works in
--   attempts     claims by the sweep so far (the inline purge is not a claim)
--   lease_owner  the worker that holds the row, while its lease lasts
--   lease_until  the lease's end; a lapsed lease is claimed again
--
-- Never edit an applied migration; add a new one.

CREATE TABLE thread_purges (
    thread_id   uuid PRIMARY KEY,
    deleted_at  timestamptz NOT NULL,
    attempts    integer NOT NULL DEFAULT 0,
    lease_owner text,
    lease_until timestamptz
);
-- The claim: rows nobody holds, oldest deletion first.
CREATE INDEX thread_purges_due ON thread_purges (deleted_at, thread_id);
