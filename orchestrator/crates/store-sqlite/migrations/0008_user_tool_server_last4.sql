-- The last four characters of the key a person typed for their own MCP server, the SQLite form of
-- Postgres migration 0033 (see it for what the column means): exactly four characters, or NULL.
--
-- Never edit an applied migration; add a new one.

ALTER TABLE user_tool_servers
    ADD COLUMN last4 TEXT CHECK (last4 IS NULL OR length(last4) = 4);
