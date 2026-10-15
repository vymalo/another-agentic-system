-- The authorizations of people's own MCP servers that have begun and whose callback has not come
-- (ADR 0046, slice 4c). One table of its own, like the definitions' (migration 0021): no event of the
-- log records it, and it holds no secret.
--
-- The browser that comes back to the OAuth callback is not the app's session (ADR 0047): on the desktop
-- and on mobile it carries neither the app's bearer token nor the web's cookie. The callback finds
-- the person by the `state` alone, which is kept here as `state -> (owner, server, until)` and TAKEN
-- ONCE (`DELETE ... RETURNING`, so of any number of callbacks for one state exactly one gets the row).
-- The PKCE verifier is not here: it stays sealed in the credential broker (`user_secrets`).
--
--   state_hash  the SHA-256 of the `state` (32 bytes), never the `state`: a state is a bearer value
--               that completes a person's authorization, so a copy of this table does not hold working
--               ones. A state is 96 random bits or more, so the hash cannot be turned back into one
--   owner       the person (`UserId`, the trimmed lower-cased e-mail), as `user_tool_servers.owner`
--   server      the person's server (`u-` and ten lowercase letters or digits). No foreign key: the
--               definition may be deleted while a flow is pending (the callback then finds no server
--               and the row expires), and the key of the definitions is (owner, id)
--   expires_at  when the broker's pending flow ends (`OAuthStart::expires_at`): a state is valid while
--               the clock the caller reads is before it
--
-- UNIQUE (owner, server) is the rule of one pending authorization per server: a later one replaces the
-- earlier (the broker keeps one pending flow per server too), which also bounds the table at one row
-- per server of each person however often they authorize. The rows of flows that were abandoned are
-- swept by the next put (`expires_at`'s index).
--
-- Never edit an applied migration; add a new one.

CREATE TABLE user_tool_oauth_pending (
    state_hash bytea       NOT NULL PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    owner      text        NOT NULL CHECK (owner <> ''),
    server     text        COLLATE "C" NOT NULL CHECK (server ~ '^u-[a-z0-9]{10}$'),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (owner, server)
);

CREATE INDEX user_tool_oauth_pending_expires_at ON user_tool_oauth_pending (expires_at);
