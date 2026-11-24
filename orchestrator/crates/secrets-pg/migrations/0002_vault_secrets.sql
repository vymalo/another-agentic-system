-- ADR 0083 (amended 2026-11-21; decision row 114 of 2026-11-22): a person's **named** secrets, with
-- versions, named by a configuration as `vault:<name>#v<n>` (`orch-secrets-pg`).
--
-- One row per (owner, name, version). The current version of a name is the one row with no
-- `superseded`; a write marks that row superseded and adds the next version in the same
-- transaction, so a name has exactly one current version at every commit. A superseded version
-- stays readable for 24 hours from the write that superseded it (the port's SUPERSEDED_GRACE) and
-- is refused after that, whether or not the row is still here; `purge` removes the ones whose 24
-- hours are over.
--
-- The envelope is the same as `user_secrets`: the format `v`, the id of the key that sealed it
-- (`kid`), the 96-bit `nonce`, and the ciphertext with its 128-bit tag. owner, name, version and
-- kid are the associated data of the seal (with a `vault` domain in front of it, so a row moved
-- between this table and `user_secrets` does not open), so a row moved to another owner, name or
-- version does not open either.
CREATE TABLE vault_secrets (
    owner      text        NOT NULL,
    -- The person's own name for the secret: 1 to 80 characters, no control character, and no `#`
    -- (the reference form needs it for the version). Refused by VaultName::parse in Rust; these
    -- checks are the backstop for a row somebody wrote by hand.
    name       text        NOT NULL CHECK (name <> '' AND length(name) <= 80 AND strpos(name, '#') = 0),
    version    bigint      NOT NULL CHECK (version > 0),
    v          smallint    NOT NULL CHECK (v > 0),
    kid        text        NOT NULL CHECK (kid <> ''),
    nonce      bytea       NOT NULL CHECK (octet_length(nonce) = 12),
    ciphertext bytea       NOT NULL CHECK (octet_length(ciphertext) >= 16),
    created    timestamptz NOT NULL,
    -- When a later write of the name superseded this version; NULL while it is the current one.
    superseded timestamptz,
    PRIMARY KEY (owner, name, version),
    CHECK (superseded IS NULL OR superseded >= created)
);

-- At most one current version per name, enforced by the database and not only by the lock.
CREATE UNIQUE INDEX vault_secrets_current ON vault_secrets (owner, name) WHERE superseded IS NULL;

-- `purge` looks rows up by the instant their grace started.
CREATE INDEX vault_secrets_superseded ON vault_secrets (superseded) WHERE superseded IS NOT NULL;

-- A rewrapped row is found by key id, as in `user_secrets`.
CREATE INDEX vault_secrets_kid ON vault_secrets (kid);
