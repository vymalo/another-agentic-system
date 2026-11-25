-- The definitions of a person's own MCP servers (ADR 0046), the SQLite form of Postgres migration
-- 0021. One table beside the log: no event records a definition, a fork does not inherit it, an
-- export does not carry it and a shared thread does not show it, and deleting a thread erases
-- nothing of it. See `store-postgres/migrations/0021_user_tool_servers.sql` for what each column
-- means; only what SQLite states differently is said here.
--
-- A definition holds NO SECRET: `credential` is the opaque reference the broker gave, never the
-- token, the key or the tokens of an authorization.
--
--   owner       the person (`UserId`), NOT EMPTY. No length cap and no check of case, as Postgres
--   id          `u-` and ten lowercase letters or digits (`UserToolServerId`), the person's own key:
--               two people may each hold the same text. The default BINARY collation orders a
--               listing by id bytewise, as the port's in-memory store does
--   name        what the person called it, 1 to 80 characters
--   url         the normal form of a URL the egress guard accepted; it may carry a secret in its
--               path, so it is never logged and no error holds it
--   auth        the kind of authentication, the JSON text the port's `UserAuth` writes
--               (`{"kind":"none"}`, `{"kind":"api_key","header":"X-Key"}`, ...), validated again
--               when it is read back
--   state       added | connected | needs_reauth | revoked (`UserToolServerState`)
--   credential  the broker's reference, NULL until it has stored one
--   revision    from the one counter below, drawn by the statement that writes the row: never given
--               twice, a delete and a new insert of the same id included
--   times       microseconds since the Unix epoch, like every time of this store
--
-- The limit of eight per person is the store's, not a constraint: one writer and one `BEGIN
-- IMMEDIATE` transaction read the count and insert the row with nobody between them (the Postgres
-- adapter's advisory lock of the owner has no work to do in one writer).
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_tool_server_counter (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    n   INTEGER NOT NULL CHECK (n >= 0)
);
INSERT INTO user_tool_server_counter (one, n) VALUES (1, 0);

CREATE TABLE user_tool_servers (
    owner      TEXT    NOT NULL CHECK (owner <> ''),
    id         TEXT    NOT NULL CHECK (
                           length(id) = 12
                           AND id GLOB 'u-*'
                           AND substr(id, 3) NOT GLOB '*[^a-z0-9]*'),
    name       TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    url        TEXT    NOT NULL CHECK (length(CAST(url AS BLOB)) BETWEEN 1 AND 2048),
    auth       TEXT    NOT NULL CHECK (json_valid(auth) AND json_type(auth) = 'object'),
    state      TEXT    NOT NULL CHECK (state IN ('added', 'connected', 'needs_reauth', 'revoked')),
    credential TEXT    CHECK (credential IS NULL
                              OR length(CAST(credential AS BLOB)) BETWEEN 1 AND 128),
    revision   INTEGER NOT NULL CHECK (revision > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CONSTRAINT user_tool_servers_pk PRIMARY KEY (owner, id)
) WITHOUT ROWID;
