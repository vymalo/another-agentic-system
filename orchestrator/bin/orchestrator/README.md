# orchestrator (binary)

The orchestrator service: the composition root that wires the Postgres store,
the A2A adapter, the dispatcher and the chat API into one stateless process.

## Where it sits

The only crate that depends on every adapter, and the only place that chooses
implementations
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
[`orch-store-postgres`](../../crates/store-postgres/README.md) for
`ThreadStore` and `Wakeup`, [`orch-agent-a2a`](../../crates/agent-a2a/README.md)
for `AgentClient`, the system clock and UUIDv7 ids. It has no logic of its own:
what the service does lives in [`orch-app`](../../crates/app/README.md) and
[`orch-api`](../../crates/api/README.md). Processes are stateless; the only
persistence is Postgres
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
Running it, the container image, configuration and shutdown are documented in
[`orchestrator/README.md`](../../README.md); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## What is in it

| File | What |
|---|---|
| `src/main.rs` | tracing setup, signals (SIGTERM, SIGINT), sysexits-style exit codes (`78` configuration, `69` database unavailable, `71` listen address, `70` half of the service stopped, `1` otherwise) |
| `src/config.rs` | `Config`, `LogFormat`, `ConfigError`: the environment and `AGENTS_FILE`, parsed through closures so tests never touch the process environment; every problem names the variable, file or agent at fault and carries no secret |
| `src/boot.rs` | `run(cfg, shutdown)`: builds the adapters, migrates, serves HTTP and the dispatcher until told to stop, drains gracefully |

## Environment

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string, never logged |
| `AGENTS_FILE` | required | YAML list of `{id, name, cardUrl, tokenEnv?}` ([`agents.example.yaml`](../../agents.example.yaml)) |
| `LISTEN_ADDR` | `0.0.0.0:8080` | |
| `AUTH_DEV_USER` | unset | e-mail served for requests without `X-Auth-Request-Email`; development only, logs a warning |
| `DATABASE_MAX_CONNECTIONS` | `10` | at least 2 |
| `DISPATCHER_CONCURRENCY` | `32` | |
| `OUTBOX_LEASE_SECS` | `30` | |
| `SHUTDOWN_GRACE_SECS` | `15` | |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info`, `json` | `LOG_FORMAT=text` for humans |

The authoritative table, with the meaning of each variable, is in
[`orchestrator/README.md`](../../README.md#configuration). Keep the two in step.

*Unverified:* the sysexits.h numbers come from memory of the BSD header, as
noted in `src/main.rs`.

## Features

None. The binary is `orchestrator` (`cargo run -p orchestrator`).

## Tests

* Unit tests in `src/config.rs`: no database, no environment.
* `tests/smoke.rs`: the built executable as a process. Configuration-error
  tests always run (the unreachable-database one waits out sqlx's 30 s
  connect timeout). With a database: `/healthz`, `/readyz`, 401 without
  identity, a thread completed through a fake agent with the bearer from
  `tokenEnv`, JSON logs, a clean exit on SIGTERM, and two processes on one
  database with a SIGKILL mid-task.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | enables the database tests; without it they print a notice and pass without running |

CI runs them against a Postgres service and against the built image
([`orchestrator.yml`](../../../.github/workflows/orchestrator.yml)).

## See also

[`orch-app`](../../crates/app/README.md),
[`orch-api`](../../crates/api/README.md),
[`orch-e2e`](../../crates/e2e/README.md).
