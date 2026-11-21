# orch-pg-migrate

Applies a crate's Postgres migrations on `tokio-postgres`, and **continues the history sqlx kept**, so the
orchestrator can leave `sqlx` (ADR 0076, sys#382 E1) without a migration step: a database sqlx migrated is a
no-op here, and a process of the build before, still on sqlx, and one of this build never migrate at once.

```rust
let migrator = orch_pg_migrate::Migrator::new(
    "_orch_secrets_pg_migrations",
    orch_pg_migrate::migrations!("migrations": "0001_user_secrets.sql")?,
)?;
migrator.run(&mut client).await?; // a tokio_postgres::Client to the database itself
```

## What it keeps from sqlx 0.9

| | sqlx's migrator and this one |
|---|---|
| The history | `version BIGINT PRIMARY KEY, description TEXT, installed_on TIMESTAMPTZ, success BOOLEAN, checksum BYTEA, execution_time BIGINT`, in the table the crate names; created if missing |
| A migration | the file `<version>_<description>.sql`: the integer before the first `_`, the rest without `.sql` with each `_` a space; its text is in the binary (`migrations!`) |
| Its checksum | the SHA-384 of its text |
| Its transaction | one, holding the SQL and the history row; none for a file that starts with `-- no-transaction` |
| The lock | `pg_advisory_lock(0x3d32ad9e * crc32(current_database()))` (CRC-32/ISO-HDLC) for the whole run, on the run's connection; released at its end, an error included (sqlx keeps it on an error) |
| The refusals, before anything runs | a row that did not succeed (`Dirty`), a version the history has and this build does not (`VersionMissing`), a checksum that differs (`VersionMismatch`) |

A session lock needs one session: run it on a connection to the database itself, never through a
transaction-mode pooler (PgBouncer).

## Errors

`MigrateError`: `FileName`, `TableName` and `DuplicateVersion` for a crate's mistake, the three refusals
above, `Execute { version, source }` when a migration's SQL fails (its transaction rolled back), and
`Database` for the connection, the lock or a bookkeeping statement.

## Tests

* Unit: a file name read as sqlx reads it (`-- no-transaction` included, bad names refused), the table name
  and versions checked, and the lock key against CRC-32's check value.
* `tests/sqlx_compat.rs`, with sqlx as a dev-dependency until it leaves the workspace at the end of E1:
  * `every_migration_of_the_workspace_reads_as_sqlx_reads_it`: every `crates/*/migrations` directory has the
    versions, descriptions and checksums sqlx gives it (no database needed);
  * against Postgres (`ORCH_TEST_DATABASE_URL`, skipped without it; a database of its own each):
    `each_continues_the_others_history` (sqlx migrates, this runner is a no-op and then applies only the new
    one, and sqlx is a no-op after it), `the_history_is_checked_before_anything_runs` (the three refusals, a
    failed migration leaves no row and none of its work, the lock is free after each refusal),
    `the_lock_is_sqlx_s_lock` (this runner waits while sqlx's migrator holds its lock).
