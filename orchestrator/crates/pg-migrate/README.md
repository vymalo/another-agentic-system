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

`migrations!` names each file (its text is in the binary; a missing file fails the build), so a crate's test checks its
list against the directory, or a file added there would never run:

```rust
migrator.check_directory(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))).unwrap();
```

## What it keeps from sqlx 0.9

| | sqlx's migrator and this one |
|---|---|
| The history | `version BIGINT PRIMARY KEY, description TEXT, installed_on TIMESTAMPTZ, success BOOLEAN, checksum BYTEA, execution_time BIGINT`, in the table the crate names; created if missing |
| A migration | the file `<version>_<description>.sql`: the integer before the first `_`, the rest without `.sql` with each `_` a space; its text is in the binary (`migrations!`) |
| Its checksum | the SHA-384 of its text |
| Its transaction | one, holding the SQL and the history row; none for a file that starts with `-- no-transaction` |
| The lock | `pg_advisory_lock(0x3d32ad9e * crc32(current_database()))` (CRC-32/ISO-HDLC) for the whole run, on the run's connection; released at its end, an error included (sqlx keeps it on an error) |
| The refusals | before anything is applied: a row that did not succeed (`Dirty`), a version the history has and this build does not (`VersionMissing`); where its version comes (a lower one not yet applied is applied first): a checksum that differs (`VersionMismatch`) |
| The session | `TimeZone = UTC` and `DateStyle = ISO, MDY` for the run, as sqlx sets them on every connection |

A reversible migration (`.up.sql`, `.down.sql`) is refused: sqlx runs only the `up` half, and none exists here.

A session lock needs one session: run it on a connection to the database itself, never through a
transaction-mode pooler (PgBouncer). A run that is cancelled (its future dropped by a timeout or a shutdown) keeps
the lock until its connection closes: drop that client then, rather than reusing it.

## Errors

`MigrateError`: `FileName`, `TableName` and `DuplicateVersion` for a crate's mistake, the three refusals
above, `Execute { version, source }` when a migration's SQL fails (its transaction rolled back), and
`Database` for the connection, the lock or a bookkeeping statement.

## Tests

* Unit: a file name read as sqlx reads it (`-- no-transaction` included; bad names, `.up.sql` and `.down.sql`
  refused), the table name and versions checked, and the lock key against CRC-32's check value.
* `tests/sqlx_compat.rs`, with sqlx as a dev-dependency until it leaves the workspace at the end of E1:
  * `every_migration_of_the_workspace_reads_as_sqlx_reads_it`: every `crates/*/migrations` directory has the
    versions, descriptions, checksums and transactions sqlx gives it (no database needed);
  * `a_list_is_checked_against_its_directory`: a file left out of the list, and one the directory lacks;
  * against Postgres (`ORCH_TEST_DATABASE_URL`, skipped without it; a database of its own each):
    `each_continues_the_others_history` (sqlx migrates, this runner is a no-op and then applies only the new
    ones, a `CREATE INDEX CONCURRENTLY` outside a transaction among them, and sqlx is a no-op after it),
    `the_history_is_checked_before_anything_runs` (the three refusals, a failed migration leaves no row and none of
    its work, the lock is free after each refusal), `the_lock_is_sqlx_s_lock` (while sqlx's migrator holds its lock
    the server shows this runner's request for it, not granted; it runs once sqlx lets go). Each drops its database,
    and the databases a killed run left are dropped by the next.
