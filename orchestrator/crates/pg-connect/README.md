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

A `postgres://` or `postgresql://` URL. `tokio-postgres` reads every parameter it knows (`host`, `port`, `user`,
`password`, `dbname`, `application_name`, `options`, `connect_timeout`, ...), each segment of the query passed to it as
written. This crate reads `sslmode` and `sslrootcert` itself, because `tokio-postgres` knows three of sqlx's five
modes, and **refuses** `sslcert`, `sslkey` and `sslpassword` (a client certificate: no deployment uses one, and a
parameter that would be ignored must not be). An error never holds the URL: it may carry a password.

## TLS

rustls on aws-lc-rs, the Mozilla roots (`webpki-roots`) and the PEM file `sslrootcert`, as sqlx's
`tls-rustls-aws-lc-rs`:

| `sslmode` | TLS | The server's certificate |
|---|---|---|
| `disable`, `allow` | none | |
| `prefer` (the default) | when the server offers it | not checked (the handshake's signature is) |
| `require` | always | not checked |
| `verify-ca` | always | its chain to a root, not its name |
| `verify-full` | always | its chain and its name |

## The pool

`PgOptions::pool(max_size, wait)`: `deadpool-postgres`, at most `max_size` connections (at least one) made on demand,
`wait` bounding both a wait for a free connection and the making of a new one, as sqlx's `acquire_timeout` did.

## Errors

`ParseError` (`NotAUrl`, `Scheme`, `SslMode`, `ClientCertificate`, `Parameter`) and `ConnectError` (`RootCert`, `Tls`,
`Pool`, `Database`).

## Tests

* Unit: each `sslmode` read as sqlx reads it, with the mode `tokio-postgres` is given; every refusal, none showing the
  password; an `sslrootcert` that cannot be read.
* `tests/parse.rs` (sqlx as a dev-dependency until it leaves the workspace): seven URLs name the same host, port,
  user, database, `application_name`, `options` and `sslmode` to both.
* `tests/tls.rs`, against a server with TLS on and a certificate for `localhost` from a test CA
  (`ORCH_TEST_TLS_DATABASE_URL`, `ORCH_TEST_TLS_ROOT_CERT`; skipped without them): `disable` is plain; `prefer` and
  `require` take the untrusted certificate; without `sslrootcert` neither verifying mode does; `verify-ca` takes the
  server under an address the certificate does not name, `verify-full` does not; a pool connects with the same TLS.
  With `ORCH_TEST_DATABASE_URL` (a plain server), `prefer` falls back and `require` does not.
