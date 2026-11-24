-- A person's team presets (ADR 0074, decision 13). One table of its own, beside the log, as a
-- person's prompt library (ADR 0070, migration 0026) and their own MCP servers (ADR 0046,
-- migration 0021): what the row keeps is the person's, NO EVENT of the log records it, and deleting
-- a thread erases nothing of it. A thread that a preset started keeps a COPY in `team_formed`, never
-- a reference, so editing or deleting a preset changes no conversation.
--
-- Invariant 3 is amended by ADR 0074: presets are durable state beside the log, the person's own,
-- read and changed only through the API.
--
-- A preset is its owner's alone in this version: sharing it by role comes later, on ADR 0070's model,
-- so there is no `audience` column. No type of the port carries the owner.
--
--   id            `t-` and 12 lowercase letters or digits (`TeamPresetId`), the whole primary key as
--                 a prompt's id is (ids come from entropy, so two people never collide); COLLATE "C"
--                 so that a listing ordered by id is ordered bytewise, as the in-memory store's is
--   owner         the person (`UserId`, the trimmed lower-cased e-mail). NOT EMPTY, for the reason of
--                 migration 0021
--   name          `^[a-z0-9][a-z0-9-]{0,31}$`, unique per owner
--   title         1 to 120 characters
--   lead          the name of a member (`^[a-z0-9][a-z0-9-]{0,31}$`). A name, not an agent id: two
--                 members may be the same agent (ADR 0074 decision 1), so an agent id would not say
--                 which member leads. That it names a member is checked in Rust: a CHECK cannot read
--                 the members' array
--   members       a JSON array of 2 to 8 members (`{"name", "target": {"agentId", "release"},
--                 "role", "history"}`), validated in Rust as a prompt's variables are
--   charter       0 to 4096 BYTES of text (`octet_length`); no U+0000 (Postgres cannot store it: the
--                 rules of `orch_core::team` refuse it before it gets here)
--   max_handoffs  6 to 20 (ADR 0074 decision 8)
--   handoff_approval
--                 `ask` | `after` | `auto`, and `handoff_after_seconds` present exactly when it is
--                 `after`, 5 to 600 (decision 10)
--   checkpoint_after
--                 the members whose turn pauses the team; `checkpoint_before_pull_request` the rule
--                 that pauses before a merge or a pull request (decision 11). That each names a member
--                 is checked in Rust
--   push_tokens   the budget of a push, 4000 to 100000 estimated tokens (amendment A1)
--   summary       whether the orchestrator's model summarises the older entries (amendment A2); the
--                 summary itself is the thread's, never the preset's
--   revision      what the store gives to every write of the row, from one sequence for the whole
--                 table: a write based on a read passes it back in the WHERE of its UPDATE (a compare
--                 and swap), and a revision is never given twice, a delete and a new insert of the
--                 same id included
--   created_at, updated_at
--                 written by the application from its clock, so the in-memory and the Postgres stores
--                 agree
--
-- The limit of 50 presets per person is the store's, kept by a transaction-scoped advisory lock of the
-- owner that every insert takes before it counts (`PgStore`, `teams.rs`): the two-`int4` form with the
-- class 0x7470_7273 ("tprs"), a key space of its own. No constraint can say it.
--
-- Never edit an applied migration; add a new one.

CREATE SEQUENCE user_team_preset_revisions AS bigint NO CYCLE;

CREATE TABLE user_team_presets (
    id                           text COLLATE "C" NOT NULL,
    owner                        text NOT NULL CHECK (owner <> ''),
    name                         text COLLATE "C" NOT NULL CHECK (name ~ '^[a-z0-9][a-z0-9-]{0,31}$'),
    title                        text NOT NULL CHECK (char_length(title) BETWEEN 1 AND 120),
    lead                         text NOT NULL CHECK (lead ~ '^[a-z0-9][a-z0-9-]{0,31}$'),
    members                      jsonb NOT NULL CHECK (
                                     jsonb_typeof(members) = 'array'
                                     AND jsonb_array_length(members) BETWEEN 2 AND 8),
    charter                      text NOT NULL DEFAULT '' CHECK (octet_length(charter) <= 4096),
    max_handoffs                 integer NOT NULL CHECK (max_handoffs BETWEEN 6 AND 20),
    handoff_approval             text NOT NULL CHECK (handoff_approval IN ('ask', 'after', 'auto')),
    handoff_after_seconds        smallint CHECK (handoff_after_seconds BETWEEN 5 AND 600),
    checkpoint_after             text[] NOT NULL DEFAULT '{}' CHECK (
                                     array_position(checkpoint_after, NULL) IS NULL),
    checkpoint_before_pull_request boolean NOT NULL DEFAULT false,
    push_tokens                  integer NOT NULL CHECK (push_tokens BETWEEN 4000 AND 100000),
    summary                      boolean NOT NULL DEFAULT false,
    revision                     bigint NOT NULL CHECK (revision > 0),
    created_at                   timestamptz NOT NULL,
    updated_at                   timestamptz NOT NULL,
    CONSTRAINT user_team_presets_pk PRIMARY KEY (id),
    CONSTRAINT user_team_presets_id_shape CHECK (id ~ '^t-[a-z0-9]{12}$'),
    CONSTRAINT user_team_presets_name_per_owner UNIQUE (owner, name),
    -- the seconds are present exactly when the mode is `after`: a preset whose mode and number
    -- disagree is no preset, and no reader has to guess which one was meant
    CONSTRAINT user_team_presets_after_seconds_match_mode CHECK (
        (handoff_approval = 'after') = (handoff_after_seconds IS NOT NULL))
);

-- a person's own presets, in id order
CREATE INDEX user_team_presets_owner_id ON user_team_presets (owner, id);

-- issue 294 (account erasure) must delete `user_team_presets WHERE owner = $1`, as it does the
-- person's prompts, tool servers and thread rail.
