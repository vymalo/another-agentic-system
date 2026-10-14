-- The definitions of a person's own MCP servers (ADR 0046, slice 4a). One table of its own, like the
-- thread's organisation of the list (ADR 0042, migration 0016): what the row keeps is the person's,
-- and no event of the log records it (the log is the conversation: it is copied into forks, carried by
-- an export and read by the people a thread is shared with, none of whom may see or inherit the
-- owner's servers).
--
-- A definition holds NO SECRET. The credential (a token, a key, the tokens of an authorization) is
-- the credential broker's, sealed in `user_secrets` (the `orch-secrets-pg` crate, with its own
-- history of migrations): `credential` here is only the opaque reference the broker gave.
--
--   owner       the person (`UserId`, the trimmed lower-cased e-mail), as `threads.owner` is
--   id          `u-` and ten lowercase letters or digits (`UserToolServerId`), the person's own key:
--               two people may each hold the same text. COLLATE "C" so that a listing ordered by id
--               is ordered bytewise, as the port's in-memory store orders it
--   name        what the person called it, 1 to 80 characters
--   url         the NORMAL FORM of a URL the egress guard accepted (the OAuth `resource`, which the
--               broker keeps its credential bound to); may carry a secret in its path: never logged
--   auth        the kind of authentication with its header, scopes and client id, JSON as the port's
--               `UserAuth` writes it (`{"kind":"none"}`, `{"kind":"api_key","header":"X-Key"}`, ...)
--   state       added | connected | needs_reauth | revoked (`UserToolServerState`); `unavailable` is a
--               reason shown while a cause lasts, not a state
--   credential  the broker's reference to the credential, NULL until it has stored one
--   revision    what the store gives to every write of the row, from one sequence for the whole table:
--               a write based on a read passes it back, in the WHERE of its UPDATE (a compare and
--               swap), and a revision is never given twice for a key, a delete and a new insert of the
--               same id included
--
-- The limit of eight servers per person is the store's, kept by a transaction-scoped advisory lock of
-- the owner that every insert takes before it counts (`PgUserToolServers`); no constraint can say it.
--
-- Never edit an applied migration; add a new one.

CREATE SEQUENCE user_tool_server_revisions AS bigint NO CYCLE;

CREATE TABLE user_tool_servers (
    owner      text        NOT NULL,
    id         text        COLLATE "C" NOT NULL CHECK (id ~ '^u-[a-z0-9]{10}$'),
    name       text        NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    url        text        NOT NULL CHECK (octet_length(url) BETWEEN 1 AND 2048),
    auth       jsonb       NOT NULL CHECK (jsonb_typeof(auth) = 'object'),
    state      text        NOT NULL CHECK (state IN ('added', 'connected', 'needs_reauth', 'revoked')),
    credential text        CHECK (credential IS NULL OR octet_length(credential) BETWEEN 1 AND 128),
    revision   bigint      NOT NULL CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (owner, id)
);
