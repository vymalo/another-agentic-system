-- ADR 0083: "a version is never reissued" made true by construction.
--
-- Until now the promise held because `purge` never removes a name's current row, so
-- `max(version) + 1` could not restart. One row per (owner, name) keeps the greatest version the
-- name has ever had, in the same transaction as the write that took it, and survives anything done
-- to `vault_secrets`: `put` numbers the next version from it, so the next version is past every
-- earlier one whether or not their rows are still there.
CREATE TABLE vault_names (
    owner              text   NOT NULL,
    name               text   NOT NULL CHECK (name <> '' AND length(name) <= 80 AND strpos(name, '#') = 0),
    high_water_version bigint NOT NULL CHECK (high_water_version > 0),
    PRIMARY KEY (owner, name)
);

-- Names written before this table existed start at the greatest version they hold.
INSERT INTO vault_names (owner, name, high_water_version)
SELECT owner, name, max(version) FROM vault_secrets GROUP BY owner, name;
