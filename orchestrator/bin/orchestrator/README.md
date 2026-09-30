# orchestrator (binary)

The orchestrator service: the composition root that wires the Postgres store,
the A2A adapter (and, with the feature `agent-local`, the local agents), the dispatcher, the inbox worker, the resource API and the interaction surfaces
chosen by `ORCH_SURFACES` into one stateless process. `ORCH_ROLE` says which
halves the process runs: the control plane, a worker, or both
([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)).

## Where it sits

The only crate that depends on every adapter, and the only place that chooses
implementations
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)):
[`orch-store-postgres`](../../crates/store-postgres/README.md) for
`ThreadStore` and `Wakeup`, [`orch-agent-a2a`](../../crates/agent-a2a/README.md)
for `AgentClient` (and, with the feature `agent-local`, [`orch-agent-adam`](../../crates/agent-adam/README.md) for
in-process agents, routed by transport with `ByTransport`), the system clock and UUIDv7 ids. It has no logic of its own:
what the service does lives in [`orch-app`](../../crates/app/README.md) and
[`orch-api`](../../crates/api/README.md) and the surface crates
([`orch-surface-agui`](../../crates/surface-agui/README.md)). Processes are stateless; the only
persistence is Postgres
([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)).
The role enum (`Role`) and the supervisor (`Host`) are not ours: they come from
the `adam-host` crate of [adam-rs](https://github.com/vymalo/another-adam-rs),
a git dependency pinned to a full commit sha in `orchestrator/Cargo.toml`
(ADR 0015, decision 4); a PR bumps it. Without the feature `agent-local` it brings two crates into the
dependency tree, `adam-host` and `adam-error`, and nothing else from adam-rs: `cargo tree -p orchestrator -i adam-runtime`
finds nothing.
Running it, the container image, configuration and shutdown are documented in
[`orchestrator/README.md`](../../README.md); the design is in
[`docs/orchestrator.md`](../../../docs/orchestrator.md).

## What is in it

| File | What |
|---|---|
| `src/main.rs` | tracing setup, signals (SIGTERM, SIGINT), sysexits-style exit codes (`78` configuration, `69` database unavailable, `71` listen address, `70` a component of the service stopped, ended early or panicked (`adam_host::HostError`), `1` otherwise) |
| `src/config.rs` | `Args` (clap derive: a flag per setting, falling back to its environment variable), `Config`, `Surface`, `LocalAgentKind` (the closed set of in-process agent kinds `transport: local` may name; `Echo` so far; compiled in with the feature `agent-local`; `needs_model()` says whether a kind calls a model), `LogFormat`, `ConfigError`. Clap only collects raw strings; `Config::load` validates them, reading the agent file and the `tokenEnv` variables through closures, so tests build `Args` by hand and never touch the process environment. Every problem names the variable, file or agent at fault, carries no secret and exits 78 |
| `src/local.rs` | the one place that knows `orch-agent-adam`, in two variants of one surface. With the feature `agent-local`: `Local::start` builds the local agents' own pool on `DATABASE_URL` and migrates their journal (only when `AGENTS_FILE` lists a local agent), `compose` builds `ByTransport<A2aAgentClient, LocalAgentClient>`, `Local::register` adds the agents' worker as a worker component in the roles that run workers, `is_unavailable` maps a transient failure to exit 69. Without it: `Agents` is the A2A client alone and `Local` cannot be built |
| `src/boot.rs` | `run(cfg, shutdown)`: shared `setup` (pool, migrations, wakeup, A2A client, agent directory, `App`), then the components of `cfg.role`, registered with `adam_host::Host`: the HTTP server (`control_plane_router` plus `serve`: health, the resource API, the configured surfaces) as a control-plane component, the dispatcher and the inbox worker (timers and stored reports, `orch_app::InboxWorker`) as worker components, and for a worker-only process the health-only router ([`orch_api::health_router`](../../crates/api/README.md)) on `LISTEN_ADDR`. `Host` starts only what the role asks for, treats the first component to end on its own as fatal (exit `70`), and stops the control plane before the workers, each bounded by `SHUTDOWN_GRACE_SECS` and aborted after that (the dispatcher and the inbox worker release their leases as they stop). Readiness flips first, so probes answer 503 for the whole drain |

## Environment

Each is also a flag (`--database-url`, `--listen-addr`, `--surfaces`, and so on; `orchestrator --help`), and a flag wins over its variable.

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string, never logged |
| `AGENTS_FILE` | required | YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}` ([`agents.example.yaml`](../../agents.example.yaml)); `gate` is the entry's verification gate, `{require?, maxAttempts?, verifier?, ci?: {required?, timeoutSecs?}}` with `deny_unknown_fields` (see [The gate](#the-verification-gate)); `transport` is `a2a` (the default when absent; `cardUrl` required) or `local` (`agent` required, names a `LocalAgentKind`; `cardUrl` and `tokenEnv` refused). `local` is refused with `LocalAgentsNotCompiled` (78) unless the build has the Cargo feature `agent-local`; any other `transport` is a startup error |
| `LISTEN_ADDR` | `0.0.0.0:8080` | control plane: the API; worker: the probes only |
| `ORCH_ROLE` | `all` | `all`, `control-plane` or `worker` (`--role`); see [Roles](#roles). Unknown is a startup error (78) |
| `AUTH_DEV_USER` | unset | e-mail served for requests without `X-Auth-Request-Email`; development only, logs a warning |
| `DATABASE_MAX_CONNECTIONS` | `10` | at least 2 |
| `DISPATCHER_CONCURRENCY` | `32` | |
| `OUTBOX_LEASE_SECS` | `30` | also the lease of a local agent's run |
| `INBOX_LEASE_SECS` | `30` | at least 3; how long a crashed replica's claim on an inbox row blocks others |
| `INBOX_POLL_SECS` | `2` | at least 1; the inbox worker's safety poll, and so the latest a timer fires after its time |
| `INBOX_PARKED_TTL_SECS` | `86400` | at least 1; how long a report that no thread watches yet waits before it expires |
| `INBOX_MAX_ATTEMPTS` | `10` | at least 1; claims of one inbox row before it is dead-lettered (a row claimed more often without being finished is dead-lettered undelivered; claims handed back at shutdown or ended by a park are not counted) |
| `AGENT_LOCAL_CONCURRENCY` | `4` | only with the feature `agent-local`: runs of local agents stepped at once (at least 1); the local agents' pool is this plus 4 connections |
| `SHUTDOWN_GRACE_SECS` | `15` | |
| `ORCH_SURFACES` | `agui` | comma-separated surfaces to mount (`--surfaces`), as far as the build has them; unknown, empty, repeated or not compiled in is a startup error, and so is the removed `chat-api` (see [Surfaces](#surfaces)) |
| `ORCH_GATE` | none | sources every job must pass before it is `done`, a comma list of `ci`, `agent-checks`, `verifier` (`--gate`). Empty is no gate: an agent that completes is done. **Only `agent-checks` is accepted by this build**; `ci` and `verifier` are a startup error (78) naming the slice that enables them |
| `ORCH_MAX_ATTEMPTS` | `3` | attempts a gated job's agent gets, the first included (`--max-attempts`); at least 1 and at most the cap |
| `ORCH_MAX_ATTEMPTS_CAP` | `10` | the most an `AGENTS_FILE` entry or a run may set the attempts to (`--max-attempts-cap`); at most `100`. When only the cap is set below `3`, the default attempts are lowered to it; an explicit `ORCH_MAX_ATTEMPTS` above the cap is a startup error |
| `ORCH_VERIFIER` | none | the verifier agent's id (`--verifier`). Refused (78) until the verifier dispatch is built |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info`, `json` | `LOG_FORMAT=text` for humans |

### The verification gate

[ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md). Three layers (sources can only be added, attempts set anywhere
within the cap): the deployment (`ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP`, `ORCH_VERIFIER`), an agent's
`gate:` in `AGENTS_FILE`, and the run that creates a thread (`forwardedProps["vymalo.gate"]`, see
[`docs/api/agui.md`](../../../docs/api/agui.md#verification-the-gate)). The result is copied into the thread's job when
it is created, so a change of configuration never reaches a running job.

```yaml
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  gate:
    require: [agent-checks]   # the agent's own `checks` artifact must pass, or it is sent back
    maxAttempts: 2            # 1..=ORCH_MAX_ATTEMPTS_CAP
```

Startup validates all of it and exits **78** with a message naming the variable or the agent: an unknown source; a
`maxAttempts` outside `1..=cap`; an entry whose `require` leaves out a source the deployment requires; a `verifier`
that is not another configured agent; an unknown member of `gate`. **`ci` and `verifier`, as sources or as settings
(`ci:`, `verifier:`), are refused in every layer**: the application drops `RequestVerification` until the verifier dispatch (slice 10) exists, and no surface writes CI reports until the CI webhook
(slice 6) exists (the inbox and timers, MVP slice 5, are built), so a gate that required
them could never pass. The message says which slice enables them. A run that asks for the same is a 400.

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
| `all` (default) | HTTP server, dispatcher and inbox worker, as before the role existed | health, resource API, surfaces | the database is migrated and answers |
| `control-plane` | HTTP server; **no dispatcher and no inbox worker**, so nothing is delivered to an agent and no timer fires from this process (a report received here is stored, and applied by a worker) | health, resource API, surfaces | the database is migrated and answers |
| `worker` | dispatcher, inbox worker, and a router with only `/healthz` and `/readyz` | health only: every other path is 404, with or without an identity | the database answers and the dispatcher has started |

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
| `agent-local` | **no** | [`orch-agent-adam`](../../crates/agent-adam/README.md) and the adam-rs runtime: `transport: local` agents run in this process ([ADR 0015](../../../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)); the variable `AGENT_LOCAL_CONCURRENCY` |

The feature decides what *can* be mounted, `ORCH_SURFACES` what *is*: a surface
named but not compiled in stops startup with an error naming its feature. The
binary is `orchestrator` (`cargo run -p orchestrator`).

**Local agents (`agent-local`).** An `AGENTS_FILE` entry with `transport: local` needs a build with
`cargo run -p orchestrator --features agent-local`; without it the entry is refused at startup (78). With it and at
least one such entry, every role opens a pool of its own on `DATABASE_URL` (`AGENT_LOCAL_CONCURRENCY + 4`
connections, on top of `DATABASE_MAX_CONNECTIONS`) and creates the journal's tables, `orch_agent_runs`,
`orch_agent_journal` and `orch_agent_meta` (prefix `orch_agent_`), after the orchestrator's own migrations; both are
idempotent. The `worker` and `all` roles register the agents' worker (and its `NOTIFY` listener on the channels
`orch_agent_events` and `orch_agent_signals`) as a worker component next to the dispatcher; the `control-plane` role
runs neither and steps nothing. A transient failure of the local agents' database at boot exits `69`.

## Surfaces

The resource API (`GET /api/agents`, `GET /api/threads`, `GET /api/threads/{id}`,
`POST /api/threads/{id}/cancel`) and health are `orch-api`'s and are mounted whatever
`ORCH_SURFACES` says. The interaction surfaces are chosen by it:

| `ORCH_SURFACES` | Serves |
|---|---|
| unset, or `agui` (the default) | the AG-UI routes: `POST /agui/agents/{agentId}`, `GET /agui/threads/{threadId}/connect`, `GET /agui/agents/{agentId}/capabilities` |

That is the whole list. The legacy chat API interaction routes (`POST /api/threads`,
`POST /api/threads/{id}/messages`, `GET /api/threads/{id}/events`, `GET /api/threads/{id}/stream`),
the crate `orch-surface-chat-api` and its feature `surface-chat-api` were **removed on 2026-09-30**
([ADR 0012](../../../docs/decisions/0012-ag-ui-user-facing-protocol.md)). Those routes answer 404
(`POST /api/threads` answers 405, because its path is also the thread list).

**Migrating an existing deployment.** An `ORCH_SURFACES` (or `--surfaces`) that still lists
`chat-api` fails closed: the process exits 78 (`EX_CONFIG`) before it connects to anything,
with

```text
ORCH_SURFACES is invalid: surface "chat-api" was removed on 2026-09-30: the legacy chat API
interaction routes (createThread, postMessage, listEvents, streamEvents; POST /api/threads and
/api/threads/{id}/messages, GET /api/threads/{id}/events and /api/threads/{id}/stream). Use AG-UI
instead: set ORCH_SURFACES=agui (the default) and speak POST /agui/agents/{agentId}, GET
/agui/threads/{threadId}/connect and GET /agui/agents/{agentId}/capabilities (docs/api/agui.md);
the resource API (GET /api/threads, GET /api/threads/{id}, GET /api/agents, cancel) is unchanged
```

Nothing is quietly ignored: drop `chat-api` from the list (or unset the variable) and move the
client. Create a thread and send a message with one `POST /agui/agents/{agentId}` (a UUID you
mint as `threadId`), read the log with `GET /agui/threads/{threadId}/connect`
([`docs/api/agui.md`](../../../docs/api/agui.md)). The web app has run on AG-UI only since
2026-09-29.

## Tests

* Unit tests in `src/config.rs`: no database, no environment (the gate: the defaults, the four variables, `ci` and
  `verifier` refused in `ORCH_GATE`, `ORCH_VERIFIER` and an `AGENTS_FILE` entry naming the slice, strict parsing of `gate:`, a
  target that weakens the deployment or exceeds the cap; defaults, the
  environment/flag mapping, unknown, empty and repeated surfaces, the removed
  `chat-api` refused with an error naming it and AG-UI (`RemovedSurface`, from the variable and from the
  flag), `--help` naming every variable, the role: default `all`, each
  value, blank, unknown, the flag collected raw). `src/main.rs` maps every
  `HostError` to exit 70. `src/logging.rs`: role and instance first on every JSON and text
  line (an instance with a quote stays valid JSON, no fields means the stock line).
* `tests/local.rs` (`#![cfg(feature = "agent-local")]`, run with `--features agent-local`): the executable hosting
  a local `echo` agent answers an AG-UI run and the journal holds the run (`orch_agent_runs`); a `control-plane`
  process with a local agent starts, accepts a run and leaves it `queued` with an empty journal until a `worker`
  process starts and completes it. Unit tests in `src/config.rs` cover the flavours: without the feature a local agent is
  refused naming `agent-local`, with it it is accepted, and `AGENT_LOCAL_CONCURRENCY` defaults to 4.
* `tests/smoke.rs`: the built executable as a process. Without the feature, `transport: local` exits 78 naming `agent-local`. Configuration-error
  tests always run (including a gate this build cannot honour, in the environment and in `AGENTS_FILE`: exit 78, the slice named; the unreachable-database one waits out sqlx's 30 s
  connect timeout). The CLI tests spawn the executable: `--help`, each variable
  read from the environment alone, a flag over its variable, a usage error, the removed
  `chat-api` (`the_removed_chat_api_surface_is_a_config_error_pointing_to_agui`: from the variable,
  beside `agui`, from the flag, and the flag over a valid variable: exit 78, the log says it was
  removed on 2026-09-30 and points to AG-UI, nothing connects first), and the default serving `agui`
  and the resource API only (the AG-UI route answers 400 to `{}` and 401 without identity; the
  four removed legacy routes answer 404, or 405 for `POST /api/threads`, while the thread list, an
  unknown thread and cancel answer as resources). With a database: `/healthz`, `/readyz`, 401 without
  identity, a thread run and completed over AG-UI (the default surface) through a fake agent with the bearer from
  `tokenEnv`, JSON logs, a clean exit on SIGTERM, and two processes on one
  database with a SIGKILL mid-task. The roles: `--role worker` (over a
  nonsense `ORCH_ROLE`) serves `/healthz` and `/readyz` answers 404 on
  `/api/...` and `/metrics` with role and instance on every log line; a `control-plane` process serves the API but the thread stays
  `queued` and the agent is never called until a worker process starts (its `/metrics` shows one
  due row meanwhile), then it completes and the backlog reads zero; a due timer row that a control plane alone leaves pending and a worker process applies (`INBOX_POLL_SECS=1`); one control plane and two workers, where the worker that holds
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
