-- A person's memory: short notes and the per-agent grants (ADR 0091, `docs/api/memory-v1.md`). Two tables of
-- their own, beside the log like the prompt library (migration 0026) and the team presets (migration 0032):
-- what a row keeps is the person's, and NO EVENT of the log records it (the log is copied into forks,
-- carried by an export and read by the people a thread is shared with, none of whom may see or inherit the
-- owner's memory). There is no foreign key to `threads` on purpose: deleting a thread erases no note.
--
-- THE OWNER IS ON EVERY ROW AND IN EVERY STATEMENT of the adapter (`store-postgres/src/memory_notes.rs`), and
-- it is the first column of both primary keys: a note id is unique per person, never global, so saving an id
-- that somebody else holds is neither an error nor a way to learn that they hold it.
--
-- `user_memories` has NO `status` column on purpose (ADR 0091 decision 6): a saved note is stored at once,
-- there is no approval step and so no waiting note.
--
--   owner       the person (`UserId`, the trimmed lower-cased e-mail), as `threads.owner` is. NOT EMPTY
--   id          `m-` and an opaque suffix, made by the caller; 1 to 64 bytes. COLLATE "C" so that comparisons are
--               bytewise as the in-memory store's are
--   ord         what orders the notes: one number per row from an identity of the table, so that two notes saved
--               in the same microsecond (or with the same `saved_at` from a clock that stood still) still have a
--               newest. `memory_read` is newest first, by `ord`, never by `saved_at`
--   body        1 to 1024 BYTES (`octet_length`; ADR 0091 decision 6, `MAX_NOTE_BYTES`), already trimmed by the
--               rules of `orch_core::memory`. No U+0000: Postgres cannot store it, and the adapter refuses it
--               before it gets here
--   written_by  the agent id that saved it, or `person`
--   saved_at    written by the application from its clock (not `clock_timestamp()`), so the in-memory and the
--               Postgres stores agree
--
-- The limit of 200 notes per person (`MAX_NOTES`) is the store's, kept by a transaction-scoped advisory lock of
-- the owner that every save takes before it counts: the two-`int4` form with the class 0x6d65_6d6f ("memo"), a
-- key space of its own. No constraint can say it.
--
-- `user_memory_settings` is OFF BY DEFAULT BY ABSENCE: a person with no row has no grant, and the default
-- every reader sees is both `false` (it is not a row and no route changes it). One row per agent the person
-- touched, with whether it may read and whether it may write. `agent` is at most 128 bytes: what the route
-- validates against the registry is not this table's business, but an unbounded text per request is.
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_memories (
    owner      text NOT NULL CHECK (owner <> ''),
    id         text COLLATE "C" NOT NULL CHECK (octet_length(id) BETWEEN 1 AND 64),
    ord        bigint GENERATED ALWAYS AS IDENTITY,
    body       text NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 1024),
    written_by text NOT NULL CHECK (octet_length(written_by) BETWEEN 1 AND 128),
    saved_at   timestamptz NOT NULL,
    CONSTRAINT user_memories_pk PRIMARY KEY (owner, id)
);

-- a person's notes, newest first (the read, the listing and the count all start at the owner)
CREATE INDEX user_memories_owner_ord ON user_memories (owner, ord DESC);

CREATE TABLE user_memory_settings (
    owner     text NOT NULL CHECK (owner <> ''),
    agent     text COLLATE "C" NOT NULL CHECK (octet_length(agent) BETWEEN 1 AND 128),
    can_read  boolean NOT NULL DEFAULT false,
    can_write boolean NOT NULL DEFAULT false,
    CONSTRAINT user_memory_settings_pk PRIMARY KEY (owner, agent)
);
