# orch-pg-connect

Connects to Postgres on `tokio-postgres` **as the orchestrator's crates connected through sqlx 0.9**, so moving a
crate off sqlx (ADR 0076, sys#382 E1) changes no deployment's connection string: the same URL means the same server,
the same TLS and the same certificate checks.

```rust
let options: orch_pg_connect::PgOptions = database_url.parse()?;
let pool = options.pool(4, Duration::from_secs(10))?;   // deadpool-postgres
let client = options.connect().await?;                   // one connection, e.g. for the migrations
```

## The connection string

Read as sqlx's `PgConnectOptions::from_str` read it, in three layers:

1. **The environment**: `PGHOSTADDR`, else `PGHOST`, else the first of `/var/run/postgresql`, `/private/tmp` and
   `/tmp` that holds the server's socket, else `localhost`; `PGPORT` (5432); `PGUSER`, else the operating system's
   user, else `unknown`; `PGPASSWORD`, `PGDATABASE`, `PGAPPNAME`, `PGOPTIONS`, `PGSSLMODE` (`prefer`) and
   `PGSSLROOTCERT`.
2. **The URL**, `postgres://` or `postgresql://`: its host (one that decodes to a `/...` path is a socket's
   directory), port, user, password and database, percent-decoded; then its query, form-decoded (a `+` is a space)
   key by key, the last one winning: `sslmode` (`ssl-mode`, any case), `sslrootcert` (`ssl-root-cert`, `ssl-ca`),
   `host` (a `/...` value is a socket, which wins over any host), `hostaddr` (an IP address), `port`, `dbname`,
   `user`, `password`, `application_name`, `options` (each one appended), `options[name]=value` (appended as
   `-c name=value`, a space or a backslash escaped) and `statement-cache-capacity` (checked, unused). Every other key
   is ignored, as sqlx ignored it: `connect_timeout` and the other parameters of libpq do nothing.
3. **The password file**, for a password neither gave: `PGPASSFILE`, then `~/.pgpass`, the first line of either that
   matches `host:port:database:user` (`*` matching anything, `\` escaping the next character, `#` a comment). On
   Linux a file the group or others can read is skipped.

**Refused**, where sqlx would have done something no deployment asks for: a client certificate (`sslcert`,
`ssl-cert`, `sslkey`, `ssl-key`, `sslpassword`, `PGSSLCERT`, `PGSSLKEY`); a `PGSSLMODE` that is not a mode (sqlx fell
back to `prefer`); an `sslrootcert` that holds no certificate (sqlx trusted the Mozilla roots alone). An error never
holds the URL: it may carry a password.

## TLS

rustls on aws-lc-rs, the Mozilla roots (`webpki-roots`) and `sslrootcert` (a PEM file, or the PEM itself), as sqlx's
`tls-rustls-aws-lc-rs`. Every connection is made with the mode below, whatever `config_mut` did to the
configuration's own `sslmode`.

| `sslmode` | TLS | The server's certificate |
|---|---|---|
| `disable`, `allow` | none | |
| `prefer` (the default) | when the server offers it | not checked (the handshake's signature is) |
| `require` | always | not checked |
| `verify-ca` | always | its chain to a root, not its name |
| `verify-full` | always | its chain and its name |

## The pool

`PgOptions::pool(max_size, wait)`: `deadpool-postgres`, at most `max_size` connections (at least one) made on demand,
each checked with an empty query before it is handed out again (sqlx's `test_before_acquire`), `wait` bounding a
wait for a free connection, the making of a new one and that check (so a socket that is half open cannot hang a call),
as sqlx's `acquire_timeout` did. `connect()` makes one
connection; an error that ends it is logged (`tracing`, warn).

**A connection the pool drops ends as a client ends one.** `deadpool-postgres`' own connector aborts the task that drives
a connection when the pool drops it, so the server reads an end of stream with no `Terminate` and, under TLS, no
`close_notify`, and logs `could not receive data from client: Connection reset by peer` for it (*verified 2026-11-21*
against PostgreSQL 16.16 with `ssl=on`). This crate's connector drives each connection in a task of its own, and the task
`deadpool-postgres` aborts only tells that one the client is gone: the connection reads the answers it still owes and
sends `Terminate` (the TLS stream its `close_notify`), within `LINGER` (1 s), or is cut. `pool_tracked(max_size, wait,
&tasks)` is `pool` with those tasks in a `ConnectionTasks`, which a caller that closes the pool waits for
(`ended_within(LINGER)`): a task the runtime drops when the process exits says nothing to the server.
`ConnectionTasks::spawn` counts another task among them (a `LISTEN` connection the caller drives), `running()` says how many
run. `wire_config()` is the configuration a connection is made with (its `sslmode` the one TLS needs), for a connection the
caller drives itself with `tls_connector()`.

## What differs from sqlx

| | sqlx 0.9 | here |
|---|---|---|
| Session parameters at startup | `TimeZone=UTC`, `DateStyle=ISO, MDY`, `extra_float_digits=2` | the server's defaults (`tokio-postgres` decodes typed values in binary; text a query formats follows the server) |
| A connection's life | closed after 10 idle minutes, and after 30 in all | kept until it fails its check |
| `sslrootcert` | read for every connection | read once per pool (a rotated CA needs a restart) |
| A connection the pool closes | `Terminate`, then the stream shut down (`PgConnection::close`, read in `sqlx-postgres` 0.9.0) | the same, within `LINGER` |
| The refusals above | done | refused |

## Errors

`ParseError` (`NotAUrl`, `Scheme`, `SslMode`, `ClientCertificate`, `Parameter(name)`) and `ConnectError` (`RootCert`,
`Tls`, `Pool`, `Database`).

## Tests

* Unit: the environment, its defaults and the OS user (a fake environment and file system), the URL's parts decoded,
  the query key by key (aliases, the last one winning, `options` growing, `options[...]` escaped, a socket kept apart
  from the host), the password file (`PGPASSFILE` before `~/.pgpass`, wildcards, escapes, comments, malformed lines),
  `sslrootcert` as a path or PEM; each `sslmode` with the mode `tokio-postgres` is given, and `config_mut` unable to
  change it; every refusal, none showing the password; an `sslrootcert` that cannot be read or holds no certificate.
* `tests/parse.rs` (sqlx as a dev-dependency until it leaves the workspace): 21 URLs name the same host (a socket
  included), port, user, password, database, `application_name`, `options` and `sslmode` to both; and in seven child
  processes, each with its own `PG*` variables and password files, six more URLs do.
* `tests/tls.rs`, against a server with TLS on and a certificate for `localhost` from a test CA
  (`ORCH_TEST_TLS_DATABASE_URL`, `ORCH_TEST_TLS_ROOT_CERT`; skipped without them, and **CI does not set them**: they
  are run by hand, as the pull request that changes this crate says): `disable` is plain; `prefer` and `require` take
  the untrusted certificate; without `sslrootcert` neither verifying mode does; `verify-ca` takes the server under an
  address the certificate does not name, `verify-full` does not; a pool connects with the same TLS. With
  `ORCH_TEST_DATABASE_URL` (a plain server), `prefer` falls back and `require` does not.
* `tests/pool.rs` (`ORCH_TEST_DATABASE_URL`, a plain server; skipped without it): a pool used on a runtime of its own,
  closed, its tasks waited for with `ended_within(LINGER)` and the runtime dropped at once: every connection ended with
  `Terminate`, as a forwarder between the pool and the server records (`deadpool-postgres`' own connector fails it).
