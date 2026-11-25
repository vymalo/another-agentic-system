-- A person's prompt library (ADR 0070), the SQLite form of Postgres migration 0026. One table beside
-- the log: no event records a prompt, deleting a thread erases nothing of it, and there is no foreign
-- key to `threads` on purpose. See `store-postgres/migrations/0026_user_prompts.sql` for what each
-- column means; only what SQLite states differently is said here.
--
--   id        global, the whole primary key (a person in the audience fetches by id). BINARY collation,
--             so `ORDER BY id` is bytewise, as the port's in-memory store orders
--   name      `^[a-z0-9][a-z0-9-]{0,31}$` as GLOB, unique per owner
--   title     1 to 120 characters (`length` counts code points of a TEXT)
--   body      1 to 16384 BYTES
--   variables JSON array text of 0 to 8 questions; validated in Rust
--   audience  JSON array text of 0 to 32 role values; validated in Rust. Empty is private
--   revision  from the one counter below, drawn by the statement that writes the row; never given twice,
--             a delete and a new insert of the same id included
--   times     microseconds since the Unix epoch, like every time of this store
--
-- The limit of 100 per person is the store's (one writer, one transaction: count, then insert).
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_prompt_counter (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    n   INTEGER NOT NULL CHECK (n >= 0)
);
INSERT INTO user_prompt_counter (one, n) VALUES (1, 0);

CREATE TABLE user_prompts (
    id          TEXT NOT NULL,
    owner       TEXT NOT NULL CHECK (owner <> ''),
    name        TEXT NOT NULL CHECK (
                    length(name) BETWEEN 1 AND 32
                    AND name NOT GLOB '*[^a-z0-9-]*'
                    AND name GLOB '[a-z0-9]*'),
    title       TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 120),
    description TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 500),
    body        TEXT NOT NULL CHECK (length(CAST(body AS BLOB)) BETWEEN 1 AND 16384),
    variables   TEXT NOT NULL CHECK (json_valid(variables) AND json_type(variables) = 'array'
                                     AND json_array_length(variables) <= 8),
    audience    TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(audience) AND json_type(audience) = 'array'
                                     AND json_array_length(audience) <= 32),
    revision    INTEGER NOT NULL CHECK (revision > 0),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    CONSTRAINT user_prompts_pk PRIMARY KEY (id),
    CONSTRAINT user_prompts_id_shape CHECK (
        length(id) = 14 AND id GLOB 'p-*' AND substr(id, 3) NOT GLOB '*[^a-z0-9]*'),
    CONSTRAINT user_prompts_name_per_owner UNIQUE (owner, name)
) WITHOUT ROWID;

CREATE INDEX user_prompts_owner_id ON user_prompts (owner, id);
