-- The authorizations of people's own MCP servers that have begun and whose OAuth callback has not
-- come (ADR 0046, slice 4c), the SQLite form of Postgres migration 0022. One table beside the log:
-- no event records it, a fork does not inherit it and deleting a thread erases nothing of it, and
-- it holds no secret. See `store-postgres/migrations/0022_user_tool_oauth_pending.sql` for what
-- each column means; only what SQLite states differently is said here.
--
--   state_hash  the SHA-256 of the `state` (32 bytes), never the `state`: a state is a bearer value
--               that completes a person's authorization, so a copy of this table does not hold
--               working ones. A state is 96 random bits or more, so the hash cannot be turned back
--               into one
--   owner       the person (`UserId`), NOT EMPTY, as `user_tool_servers.owner`
--   server      `u-` and ten lowercase letters or digits. No foreign key, as in Postgres: the
--               definition may be deleted while a flow is pending (the callback then finds no
--               server and the row expires)
--   expires_at  when the broker's pending flow ends (`OAuthStart::expires_at`), microseconds since
--               the Unix epoch like every time of this store
--   created_at  the database's clock at the write, as Postgres's `clock_timestamp()`; the port
--               never reads it
--
-- UNIQUE (owner, server) is the rule of one pending authorization per server: a later one replaces
-- the earlier (the broker keeps one pending flow per server too), which also bounds the table at one
-- row per server of each person however often they authorize. The rows of flows that were abandoned
-- are swept by the next put, which is what the index on `expires_at` serves.
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_tool_oauth_pending (
    state_hash BLOB    NOT NULL PRIMARY KEY CHECK (length(state_hash) = 32),
    owner      TEXT    NOT NULL CHECK (owner <> ''),
    server     TEXT    NOT NULL CHECK (
                           length(server) = 12
                           AND server GLOB 'u-*'
                           AND substr(server, 3) NOT GLOB '*[^a-z0-9]*'),
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (owner, server)
) WITHOUT ROWID;

CREATE INDEX user_tool_oauth_pending_expires_at ON user_tool_oauth_pending (expires_at);
