-- A person's team presets (ADR 0074, decision 13), the SQLite form of Postgres migration 0032. One
-- table beside the log: no event records a preset, deleting a thread erases nothing of it, and there
-- is no foreign key to `threads` on purpose. See `store-postgres/migrations/0032_user_team_presets.sql`
-- for what each column means; only what SQLite states differently is said here.
--
--   id          global, the whole primary key. BINARY collation, so `ORDER BY id` is bytewise, as
--               the port's in-memory store orders
--   name, lead  `^[a-z0-9][a-z0-9-]{0,31}$` as GLOB; `name` unique per owner
--   title       1 to 120 characters (`length` counts code points of a TEXT)
--   members     JSON array text of 2 to 8 members; validated in Rust (a CHECK cannot read a member)
--   charter     at most 4096 BYTES
--   max_handoffs 6 to 20; push_tokens 4000 to 100000
--   handoff_approval + handoff_after_seconds
--               `ask` | `after` | `auto`, the seconds present exactly when it is `after`
--   checkpoint_after, checkpoint_before_pull_request
--               the members whose turn pauses the team, and the rule before a merge or a pull request
--   summary     whether the older entries are summarised for the members
--   revision    from the one counter below, drawn by the statement that writes the row; never given
--               twice, a delete and a new insert of the same id included
--   times       microseconds since the Unix epoch, like every time of this store
--
-- The limit of 50 per person is the store's (one writer, one transaction: count, then insert).
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_team_preset_counter (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    n   INTEGER NOT NULL CHECK (n >= 0)
);
INSERT INTO user_team_preset_counter (one, n) VALUES (1, 0);

CREATE TABLE user_team_presets (
    id                           TEXT NOT NULL,
    owner                        TEXT NOT NULL CHECK (owner <> ''),
    name                         TEXT NOT NULL CHECK (
                                     length(name) BETWEEN 1 AND 32
                                     AND name NOT GLOB '*[^a-z0-9-]*'
                                     AND name GLOB '[a-z0-9]*'),
    title                        TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 120),
    lead                         TEXT NOT NULL CHECK (
                                     length(lead) BETWEEN 1 AND 32
                                     AND lead NOT GLOB '*[^a-z0-9-]*'
                                     AND lead GLOB '[a-z0-9]*'),
    members                      TEXT NOT NULL CHECK (
                                     json_valid(members) AND json_type(members) = 'array'
                                     AND json_array_length(members) BETWEEN 2 AND 8),
    charter                      TEXT NOT NULL DEFAULT '' CHECK (
                                     length(CAST(charter AS BLOB)) <= 4096),
    max_handoffs                 INTEGER NOT NULL CHECK (max_handoffs BETWEEN 6 AND 20),
    handoff_approval             TEXT NOT NULL CHECK (handoff_approval IN ('ask', 'after', 'auto')),
    handoff_after_seconds        INTEGER CHECK (handoff_after_seconds BETWEEN 5 AND 600),
    checkpoint_after             TEXT NOT NULL DEFAULT '[]' CHECK (
                                     json_valid(checkpoint_after)
                                     AND json_type(checkpoint_after) = 'array'),
    checkpoint_before_pull_request INTEGER NOT NULL DEFAULT 0 CHECK (
                                     checkpoint_before_pull_request IN (0, 1)),
    push_tokens                  INTEGER NOT NULL CHECK (push_tokens BETWEEN 4000 AND 100000),
    summary                      INTEGER NOT NULL DEFAULT 0 CHECK (summary IN (0, 1)),
    revision                     INTEGER NOT NULL CHECK (revision > 0),
    created_at                   INTEGER NOT NULL,
    updated_at                   INTEGER NOT NULL,
    CONSTRAINT user_team_presets_pk PRIMARY KEY (id),
    CONSTRAINT user_team_presets_id_shape CHECK (
        length(id) = 14 AND id GLOB 't-*' AND substr(id, 3) NOT GLOB '*[^a-z0-9]*'),
    CONSTRAINT user_team_presets_name_per_owner UNIQUE (owner, name),
    CONSTRAINT user_team_presets_after_seconds_match_mode CHECK (
        (handoff_approval = 'after') = (handoff_after_seconds IS NOT NULL))
) WITHOUT ROWID;

CREATE INDEX user_team_presets_owner_id ON user_team_presets (owner, id);
