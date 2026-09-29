# orchestrator (binary)

The orchestrator service: the composition root that wires the Postgres store,
the A2A adapter, the dispatcher, the resource API and the interaction surfaces
chosen by `ORCH_SURFACES` into one stateless process. `ORCH_ROLE` says which
halves the process runs: the control plane, a worker, or both
([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)).

## Where it sits

The only crate that depends on every adapter, and the only place that chooses
implementations
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
[`orch-store-postgres`](../../crates/store-postgres/README.md) for
`ThreadStore` and `Wakeup`, [`orch-agent-a2a`](../../crates/agent-a2a/README.md)
for `AgentClient`, the system clock and UUIDv7 ids. It has no logic of its own:
what the service does lives in [`orch-app`](../../crates/app/README.md) and
[`orch-api`](../../crates/api/README.md) and the surface crates
([`orch-surface-agui`](../../crates/surface-agui/README.md),
[`orch-surface-chat-api`](../../crates/surface-chat-api/README.md)). Processes are stateless; the only
persistence is Postgres
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
The role enum (`Role`) and the supervisor (`Host`) are not ours: they come from
the `adam-host` crate of [adam-rs](https://github.com/vymalo/another-adam-rs),
a git dependency pinned to a full commit sha in `orchestrator/Cargo.toml`
(ADR 0015, decision 4); a PR bumps it. It brings two crates into the lock file,
`adam-host` and `adam-error`, and nothing else new.
Running it, the container image, configuration and shutdown are documented in
[`orchestrator/README.md`](../../README.md); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## What is in it

| File | What |
|---|---|
| `src/main.rs` | tracing setup, signals (SIGTERM, SIGINT), sysexits-style exit codes (`78` configuration, `69` database unavailable, `71` listen address, `70` a component of the service stopped, ended early or panicked (`adam_host::HostError`), `1` otherwise) |
| `src/config.rs` | `Args` (clap derive: a flag per setting, falling back to its environment variable), `Config`, `Surface`, `LocalAgentKind` (the closed set of in-process agent kinds `transport: local` may name; `Echo` so far, none compiled in yet), `LogFormat`, `ConfigError`. Clap only collects raw strings; `Config::load` validates them, reading the agent file and the `tokenEnv` variables through closures, so tests build `Args` by hand and never touch the process environment. Every problem names the variable, file or agent at fault, carries no secret and exits 78 |
| `src/boot.rs` | `run(cfg, shutdown)`: shared `setup` (pool, migrations, wakeup, A2A client, agent directory, `App`), then the components of `cfg.role`, registered with `adam_host::Host`: the HTTP server (`control_plane_router` plus `serve`: health, the resource API, the configured surfaces) as a control-plane component, the dispatcher as a worker component, and for a worker-only process the health-only router ([`orch_api::health_router`](../../crates/api/README.md)) on `LISTEN_ADDR`. `Host` starts only what the role asks for, treats the first component to end on its own as fatal (exit `70`), and stops the control plane before the workers, each bounded by `SHUTDOWN_GRACE_SECS` and aborted after that (the dispatcher releases its leases as it stops). Readiness flips first, so probes answer 503 for the whole drain |

## Environment

Each is also a flag (`--database-url`, `--listen-addr`, `--surfaces`, and so on; `orchestrator --help`), and a flag wins over its variable.

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string, never logged |
| `AGENTS_FILE` | required | YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?}` ([`agents.example.yaml`](../../agents.example.yaml)); `transport` is `a2a` (the default when absent; `cardUrl` required) or `local` (`agent` required, names a `LocalAgentKind`; `cardUrl` and `tokenEnv` refused). `local` is refused with `LocalAgentsNotCompiled` (78) until a build has local agents (Cargo feature `agent-local`, not available yet); any other `transport` is a startup error |
| `LISTEN_ADDR` | `0.0.0.0:8080` | control plane: the API; worker: the probes only |
| `ORCH_ROLE` | `all` | `all`, `control-plane` or `worker` (`--role`); see [Roles](#roles). Unknown is a startup error (78) |
| `AUTH_DEV_USER` | unset | e-mail served for requests without `X-Auth-Request-Email`; development only, logs a warning |
| `DATABASE_MAX_CONNECTIONS` | `10` | at least 2 |
| `DISPATCHER_CONCURRENCY` | `32` | |
| `OUTBOX_LEASE_SECS` | `30` | |
| `SHUTDOWN_GRACE_SECS` | `15` | |
| `ORCH_SURFACES` | `agui,chat-api` | comma-separated surfaces to mount (`--surfaces`), as far as the build has them; unknown, empty, repeated or not compiled in is a startup error |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info`, `json` | `LOG_FORMAT=text` for humans |

### Logs and metrics

Every log line carries the process's `role` and `instance` (the lease owner), so lines of several
replicas can be told apart in a collector. In JSON they are the first two keys of the object
(`{"role":"worker","instance":"w1","timestamp":...}`); in text the line starts
`role=worker instance=w1 `. They are added by the event formatter (`src/logging.rs`), not by a
span, so the dispatcher's spawned tasks have them too. The configuration is read before logging
starts: a line about an invalid configuration has neither (there is no role yet). While a
dispatcher processes an outbox row, its lines also sit in an `outbox` span (`id`, `thread`,
`kind`, `attempt`; JSON keys `span` and `spans`).

Every role serves `GET /metrics` on `LISTEN_ADDR` without an identity: the outbox queue as
Prometheus text, for dashboards and for scaling the workers (see
[`orch-api`](../../crates/api/README.md#get-metrics) for the samples, and
[`docs/orchestrator.md`](../../../docs/orchestrator.md#observability-and-scaling) for the KEDA
recipe). The edge proxy of the compose stack does not route it.

### Roles

`ORCH_ROLE` is `adam_host::Role`, a closed enum owned by adam-rs, so the names are the same for every
host (adam-coder reads its own `ROLE`). Every role runs the migrations, needs `DATABASE_URL` and
`AGENTS_FILE`, and binds `LISTEN_ADDR`.

| Role | Starts | Serves on `LISTEN_ADDR` | Ready when |
|---|---|---|---|
| `all` (default) | HTTP server and dispatcher, as before the role existed | health, resource API, surfaces | the database is migrated and answers |
| `control-plane` | HTTP server; **no dispatcher**, so nothing is delivered to an agent from this process | health, resource API, surfaces | the database is migrated and answers |
| `worker` | dispatcher, and a router with only `/healthz` and `/readyz` | health only: every other path is 404, with or without an identity | the database answers and the dispatcher has started |

The two halves share nothing but Postgres (outbox, thread version compare-and-swap, `LISTEN/NOTIFY`;
[ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)), so a control plane
and its workers may be any number of processes on any machines. A thread created through a control
plane stays `queued` until some worker runs; a worker that dies mid-task hands it over through the
outbox lease, as before. On shutdown the server drains before the dispatcher stops; a worker's probe
router outlives its dispatcher, so a draining worker answers 503, not connection refused.

The authoritative table, with the meaning of each variable, is in
[`orchestrator/README.md`](../../README.md#configuration). Keep the two in step.

*Unverified:* the sysexits.h numbers come from memory of the BSD header, as
noted in `src/main.rs`.

## Features

| Feature | Default | Compiles in |
|---|---|---|
| `surface-agui` | yes | [`orch-surface-agui`](../../crates/surface-agui/README.md), the surface name `agui` |
| `surface-chat-api` | yes | [`orch-surface-chat-api`](../../crates/surface-chat-api/README.md), the surface name `chat-api` |

The feature decides what *can* be mounted, `ORCH_SURFACES` what *is*: a surface
named but not compiled in stops startup with an error naming its feature. The
binary is `orchestrator` (`cargo run -p orchestrator`).

## Tests

* Unit tests in `src/config.rs`: no database, no environment (defaults, the
  environment/flag mapping, unknown, empty and repeated surfaces, a surface not
  compiled in, `--help` naming every variable, the role: default `all`, each
  value, blank, unknown, the flag collected raw). `src/main.rs` maps every
  `HostError` to exit 70. `src/logging.rs`: role and instance first on every JSON and text
  line (an instance with a quote stays valid JSON, no fields means the stock line).
* `tests/smoke.rs`: the built executable as a process. Configuration-error
  tests always run (the unreachable-database one waits out sqlx's 30 s
  connect timeout). The CLI tests spawn the executable: `--help`, each variable
  read from the environment alone, a flag over its variable, a usage error, and
  `--surfaces chat-api` serving the legacy routes and not the AG-UI one, and the default mounting both
  (the AG-UI route answers 400 to `{}` and 401 without identity). With a database: `/healthz`, `/readyz`, 401 without
  identity, a thread completed through a fake agent with the bearer from
  `tokenEnv`, JSON logs, a clean exit on SIGTERM, and two processes on one
  database with a SIGKILL mid-task. The roles: `--role worker` (over a
  nonsense `ORCH_ROLE`) serves `/healthz` and `/readyz` answers 404 on
  `/api/...` and `/metrics` with role and instance on every log line; a `control-plane` process serves the API but the thread stays
  `queued` and the agent is never called until a worker process starts (its `/metrics` shows one
  due row meanwhile), then it completes and the backlog reads zero; one control plane and two workers, where the worker that holds
  the delegation is SIGKILLed and the other finishes it (message delivered once,
  events once each); a worker stopped on SIGTERM within its grace hands a running
  task over at once.

| Variable | Meaning |
|---|---|
| `ORCH_TEST_DATABASE_URL` | enables the database tests; without it they print a notice and pass without running |

CI runs them against a Postgres service and against the built image
([`orchestrator.yml`](../../../.github/workflows/orchestrator.yml)).

## See also

[`orch-app`](../../crates/app/README.md),
[`orch-api`](../../crates/api/README.md),
[`orch-e2e`](../../crates/e2e/README.md).
