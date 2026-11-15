-- The search index of a person's threads (ADR 0068): the folded words (keys) of their titles, descriptions
-- and messages. It is a function of the log, written by the store in the transaction that appends the
-- event, erased with the thread, and rebuilt from the log by `orchestrator search backfill`.
--
-- The keys are made in Rust (`orch_core::search`: the rules of docs/api/thread-search-v1.md) and stored
-- with `array_to_tsvector`: lexemes exactly as given, no parser, no dictionary, no stop words, no locale.
-- A query is a `tsquery` made of the keys of the query with `:*` (a prefix), so the candidates Postgres
-- finds are the contract's whatever the database's collation or text-search configuration. No extension.
--
--   message_terms           one row per searched message of a thread: a person's `user_message` and an agent's
--                           final `agent_message` that is not working text, the last of each `message_id`
--                           (a later final one deletes the earlier row of its id when it is indexed).
--                           `owner` is the thread's owner: every read filters on it.
--   message_terms.terms     the distinct keys of the message's first 3000 words
--   threads.title_terms     the keys of the title; NULL until the thread has been indexed
--   threads.description_terms  the keys of the description; NULL when the thread has none or has not been indexed
--   threads.search_through the seq of the thread's log the index has been through, with no gap: every searched
--                           message up to it is indexed. 0 for a thread that exists today: its log is not indexed
--                           yet. A commit catches an old log up by at most `COMMIT_CATCH_UP_EVENTS` (500) events
--                           and always indexes its own events (rows may exist above the watermark: the
--                           upsert makes the catch-up that reaches them harmless), so a long thread is caught up
--                           over several commits or by `orchestrator search backfill`. Unlike the usage
--                           ledger's, a fork's starts at its last seq: the events it copied are indexed from its
--                           own log in the transaction that creates it (they are searched only once its parent is
--                           gone), because the parent's rows may no longer hold a message the parent replaced
--   threads.search_fold     the `orch_core::search::FOLD_VERSION` the thread's keys were made under; 0 for a thread
--                           that exists today. A thread whose value is not the current version is unindexed: it is
--                           not found, a commit to it or `search backfill` indexes it again from the start (its rows
--                           replaced), and `search backfill --rebuild` does it for every thread. A change to the
--                           folding rules, or a bump of a dependency whose Unicode tables they read, raises the
--                           version and so needs no migration
--
-- Erasing a thread (ADR 0043) deletes its events, and `message_terms` goes with them (`ON DELETE CASCADE`), and
-- the title columns are on the row that is deleted: no word of the thread is left when the transaction commits.
--
-- No CHECK on the new columns (a constraint on `ADD COLUMN ... DEFAULT` makes Postgres scan `threads` under an
-- exclusive lock), and the table is new and empty, so a plain CREATE INDEX is instant: nothing is rewritten and
-- no large table is locked. The heavy work, the words of the logs that exist, is `search backfill`.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE threads ADD COLUMN title_terms tsvector;
ALTER TABLE threads ADD COLUMN description_terms tsvector;
ALTER TABLE threads ADD COLUMN search_through bigint NOT NULL DEFAULT 0;
ALTER TABLE threads ADD COLUMN search_fold integer NOT NULL DEFAULT 0;

CREATE TABLE message_terms (
    thread_id  uuid   NOT NULL,
    seq        bigint NOT NULL,
    owner      text   NOT NULL,
    message_id text,
    terms      tsvector NOT NULL,
    PRIMARY KEY (thread_id, seq),
    FOREIGN KEY (thread_id, seq) REFERENCES events (thread_id, seq) ON DELETE CASCADE
);
-- The words: a GIN index answers `terms @@ query`, and the owner's btree lets the planner combine the two.
CREATE INDEX message_terms_terms ON message_terms USING gin (terms);
CREATE INDEX message_terms_owner ON message_terms (owner);
-- A later final message deletes the earlier ones of its id.
CREATE INDEX message_terms_message ON message_terms (thread_id, message_id) WHERE message_id IS NOT NULL;
