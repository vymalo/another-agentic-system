-- A person's prompt library (ADR 0070, `docs/api/prompts-v1.md`). One table of its own, like the
-- definitions of a person's own MCP servers (ADR 0046, migration 0021) and the thread's organisation of
-- the list (ADR 0042, migration 0016): what the row keeps is the person's, and NO EVENT of the log records
-- it (the log is the conversation: it is copied into forks, carried by an export and read by the people a
-- thread is shared with, none of whom may see or inherit the owner's prompts). Deleting a thread erases
-- nothing of it, and there is no foreign key to `threads` on purpose.
--
-- A prompt is its owner's. The owner may share it with roles (`audience`); the people whose roles claim
-- holds one of those values (or `*`) read and render it, and never learn who wrote it: no type of the port
-- carries the owner. It goes with the person's account (issue 294 deletes `user_prompts WHERE owner = $1`;
-- nothing deletes a person today, so no hook exists to extend).
--
--   id          `p-` and 12 lowercase letters or digits (`PromptId`). GLOBAL, the whole primary key: a
--               person in the audience fetches a shared prompt by its id without knowing the author, so
--               (owner, id) as in `user_tool_servers` would not do. COLLATE "C" so that a listing ordered
--               by id is ordered bytewise, as the port's in-memory store orders it
--   owner       the person (`UserId`, the trimmed lower-cased e-mail), as `threads.owner` is. NOT EMPTY:
--               the authenticators refuse an identity that is empty after the trim. No cap and no check of
--               case, for the reasons of migration 0021
--   name        `^[a-z0-9][a-z0-9-]{0,31}$`, unique per owner (the `/name` of a command menu)
--   title       1 to 120 characters (`char_length` counts code points, as Rust's `chars().count()`)
--   description 0 to 500 characters
--   body        1 to 16384 BYTES of text with `{{id}}` references (`octet_length`); no U+0000 (Postgres
--               cannot store it: the rules of `orch_core::prompt` refuse it before it gets here)
--   variables   a JSON array of 0 to 8 questions of the UI catalog (version 8, without `show` and
--               `images`); validated in Rust: a CHECK cannot read a question. `jsonb` cannot hold U+0000 in a
--               string, so the rules refuse a control character in every text of a variable (a store that
--               accepted it would fail on the insert, and what fails every time is no 503 a client retries)
--   audience    0 to 32 role values, each 1 to 64 bytes of 0x21 to 0x7E (checked in Rust: a CHECK cannot
--               walk the elements of an array without a function); `*` only alone. Empty is private
--   revision    what the store gives to every write of the row, from one sequence for the whole table: a
--               write based on a read passes it back, in the WHERE of its UPDATE (a compare and swap), and a
--               revision is never given twice, a delete and a new insert of the same id included
--   created_at, updated_at
--               written by the application from its clock (not `clock_timestamp()`), so the in-memory and
--               the Postgres stores agree. `created_at` is never read by anybody but the owner's repairs
--               (a shared view has no created time)
--
-- The limit of 100 prompts per person is the store's, kept by a transaction-scoped advisory lock of the
-- owner that every insert takes before it counts (`PgStore`, `prompts.rs`): the two-`int4` form with the
-- class 0x7072_6d70 ("prmp"), a key space of its own (not the `usrv` of tool servers). No constraint can say it.
--
-- Never edit an applied migration; add a new one.

CREATE SEQUENCE user_prompt_revisions AS bigint NO CYCLE;

CREATE TABLE user_prompts (
    id          text COLLATE "C" NOT NULL,
    owner       text NOT NULL CHECK (owner <> ''),
    name        text COLLATE "C" NOT NULL CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,31}$'),
    title       text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    description text NOT NULL DEFAULT '' CHECK (char_length(description) <= 500),
    body        text NOT NULL CHECK (octet_length(body) BETWEEN 1 AND 16384),
    variables   jsonb NOT NULL CHECK (jsonb_typeof(variables) = 'array' AND jsonb_array_length(variables) <= 8),
    audience    text[] NOT NULL DEFAULT '{}' CHECK (
                    cardinality(audience) <= 32
                    AND array_position(audience, NULL) IS NULL
                    AND (NOT ('*' = ANY (audience)) OR cardinality(audience) = 1)),
    revision    bigint NOT NULL CHECK (revision > 0),
    created_at  timestamptz NOT NULL,
    updated_at  timestamptz NOT NULL,
    CONSTRAINT user_prompts_pk PRIMARY KEY (id),
    CONSTRAINT user_prompts_id_shape CHECK (id ~ '^p-[a-z0-9]{12}$'),
    CONSTRAINT user_prompts_name_per_owner UNIQUE (owner, name)
);

-- a person's own prompts, in id order
CREATE INDEX user_prompts_owner_id ON user_prompts (owner, id);

-- the prompts shared with a role: `cardinality(audience) > 0 AND audience && roles_plus_star`. A query uses a
-- partial index only when its WHERE implies the index's predicate, and the overlap alone does not (the
-- planner does not know an empty audience overlaps nothing), so every listing states both; `tests/prompts.rs`
-- asks EXPLAIN. The owner's own prompts are the `(owner, id)` index above; `all` is a BitmapOr of the two
CREATE INDEX user_prompts_audience ON user_prompts USING gin (audience) WHERE cardinality(audience) > 0;
