-- ADR 0046, slice 3: the sealed secrets of a person's MCP servers (`orch-secrets-pg`).
--
-- This crate's own history table (`_orch_secrets_pg_migrations`) records this file; it does not
-- share `_sqlx_migrations` with `orch-store-postgres`.

-- One counter for the whole store. A version is taken from it inside the key's lock, so it is
-- never given twice for a key (not across a delete either) and a key holds the greatest it was
-- given. NO CYCLE: running out would be an error, not a repeat.
CREATE SEQUENCE user_secret_versions AS bigint NO CYCLE;

-- One row per (owner, server). `server` is the text form of a SecretServer: `u-abcdefghij`, or
-- `u-abcdefghij#flow` for the pending flow of that server.
--
-- The envelope is the four columns `v`, `kid`, `nonce`, `ciphertext`: the format version, the
-- id of the key that sealed it, the 96-bit nonce, and the ciphertext with its 128-bit tag.
-- owner, server, version and kid are the associated data of the seal, so a row moved to
-- another owner, server or version does not open.
CREATE TABLE user_secrets (
    owner      text        NOT NULL,
    server     text        NOT NULL,
    version    bigint      NOT NULL CHECK (version > 0),
    v          smallint    NOT NULL CHECK (v > 0),
    kid        text        NOT NULL CHECK (kid <> ''),
    nonce      bytea       NOT NULL CHECK (octet_length(nonce) = 12),
    ciphertext bytea       NOT NULL CHECK (octet_length(ciphertext) >= 16),
    updated    timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (owner, server)
);

-- `rewrap` and an operator's `SELECT kid, count(*) ... GROUP BY kid` look rows up by key id.
CREATE INDEX user_secrets_kid ON user_secrets (kid);
