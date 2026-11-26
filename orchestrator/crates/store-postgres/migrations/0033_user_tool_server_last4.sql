-- The last four characters of the key a person typed for their own MCP server (ADR 0083, rule 3 of
-- the trusted components: the agent's receipt names the id, the scopes and the last four characters,
-- never the key). Kept on the definition, beside the broker's reference and not beside the sealed
-- secret, and written only by the save that had the key in hand: nothing derives it from a stored
-- secret afterwards.
--
--   last4  exactly four characters, or NULL: for a key of fewer than 12 characters, for a server
--          that holds no typed key (an OAuth authorization, no authentication) and for a definition
--          written before this column
--
-- Never edit an applied migration; add a new one.

ALTER TABLE user_tool_servers
    ADD COLUMN last4 text CHECK (last4 IS NULL OR char_length(last4) = 4);
