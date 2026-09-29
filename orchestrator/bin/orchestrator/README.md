# orchestrator (binary)

The orchestrator service: the composition root that wires the Postgres store,
the A2A adapter, the dispatcher, the resource API and the interaction surfaces
chosen by `ORCH_SURFACES` into one stateless process.

## Where it sits

The only crate that depends on every adapter, and the only place that chooses
implementations
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
[`orch-store-postgres`](../../crates/store-postgres/README.md) for
`ThreadStore` and `Wakeup`, [`orch-agent-a2a`](../../crates/agent-a2a/README.md)
for `AgentClient`, the system clock and UUIDv7 ids. It has no logic of its own:
what the service does lives in [`orch-app`](../../crates/app/README.md) and
[`orch-api`](../../crates/api/README.md) and the surface crates
([`orch-surface-chat-api`](../../crates/surface-chat-api/README.md)). Processes are stateless; the only
persistence is Postgres
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
Running it, the container image, configuration and shutdown are documented in
[`orchestrator/README.md`](../../README.md); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## What is in it

| File | What |
|---|---|
| `src/main.rs` | tracing setup, signals (SIGTERM, SIGINT), sysexits-style exit codes (`78` configuration, `69` database unavailable, `71` listen address, `70` half of the service stopped, `1` otherwise) |
| `src/config.rs` | `Args` (clap derive: a flag per setting, falling back to its environment variable), `Config`, `Surface`, `LogFormat`, `ConfigError`. Clap only collects raw strings; `Config::load` validates them, reading the agent file and the `tokenEnv` variables through closures, so tests build `Args` by hand and never touch the process environment. Every problem names the variable, file or agent at fault, carries no secret and exits 78 |
| `src/boot.rs` | `run(cfg, shutdown)`, in three parts: shared `setup` (pool, migrations, wakeup, A2A client, agent directory, `App`); `control_plane` (the HTTP server: health, the resource API, the configured surfaces; a future that ends when its `CancellationToken` is cancelled); `worker` (the dispatcher, same contract). `supervise` drives any list of such halves: the first to end on its own is fatal (exit `70`), then readiness flips, every half is cancelled and they are awaited in order, each for `SHUTDOWN_GRACE_SECS` and aborted after that (the dispatcher releases its leases as it stops). `run` starts both; there is no role flag yet, so a later change can run one half by building only its future (a worker-only process can serve probes with [`orch_api::health_router`](../../crates/api/README.md)) |

## Environment

Each is also a flag (`--database-url`, `--listen-addr`, `--surfaces`, and so on; `orchestrator --help`), and a flag wins over its variable.

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
| `ORCH_SURFACES` | `chat-api` | comma-separated surfaces to mount (`--surfaces`); unknown, empty, repeated or not compiled in is a startup error |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info`, `json` | `LOG_FORMAT=text` for humans |

The authoritative table, with the meaning of each variable, is in
[`orchestrator/README.md`](../../README.md#configuration). Keep the two in step.

*Unverified:* the sysexits.h numbers come from memory of the BSD header, as
noted in `src/main.rs`.

## Features

| Feature | Default | Compiles in |
|---|---|---|
| `surface-chat-api` | yes | [`orch-surface-chat-api`](../../crates/surface-chat-api/README.md), the surface name `chat-api` |

The feature decides what *can* be mounted, `ORCH_SURFACES` what *is*: a surface
named but not compiled in stops startup with an error naming its feature. The
binary is `orchestrator` (`cargo run -p orchestrator`).

## Tests

* Unit tests in `src/config.rs`: no database, no environment (defaults, the
  environment/flag mapping, unknown, empty and repeated surfaces, a surface not
  compiled in, `--help` naming every variable).
* `tests/smoke.rs`: the built executable as a process. Configuration-error
  tests always run (the unreachable-database one waits out sqlx's 30 s
  connect timeout). The CLI tests spawn the executable: `--help`, each variable
  read from the environment alone, a flag over its variable, a usage error, and
  `--surfaces chat-api` serving the legacy routes. With a database: `/healthz`, `/readyz`, 401 without
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
