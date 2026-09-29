# Orchestrator

A stateless Rust service that implements the chat API
([`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml)) and delegates each
thread to one configured A2A agent. Every thread is an event log in Postgres;
the process itself keeps nothing, so a restart mid-task loses nothing. Design:
[`docs/orchestrator.md`](../docs/orchestrator.md); decisions: ADRs 0001, 0004,
0007, 0008, 0009 in [`docs/decisions/`](../docs/decisions/).

> **Status:** MVP steps 1–2 of issue #9 are implemented: the chat API, the
> durable dispatcher, the A2A adapter, the Postgres store and the runnable
> binary and image. The planner, verify/rework, reviewers and the MCP/webhook
> inputs come in later steps ([`docs/mvp.md`](../docs/mvp.md)).

## Run it locally

You need a Postgres (16 is what CI uses) and at least one A2A agent.

```sh
cd orchestrator
cp agents.example.yaml agents.yaml         # then edit: id, name, cardUrl, tokenEnv
export CODER_A2A_TOKEN=...                 # the variable named by tokenEnv
DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch \
AGENTS_FILE=agents.yaml \
AUTH_DEV_USER=me@example.com \
LOG_FORMAT=text \
  cargo run -p orchestrator
```

Migrations are applied at boot. Then, for example:

```sh
curl -s localhost:8080/api/agents
curl -s -X POST localhost:8080/api/threads -H 'content-type: application/json' \
  -d '{"target":{"agentId":"coder"},"text":"say hello"}'
```

### Configuration

All configuration is environment variables; the process refuses to start, with
a message naming the culprit, when it is wrong.

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string. Never logged. |
| `AGENTS_FILE` | required | YAML list of `{id, name, cardUrl, tokenEnv?}`, see [`agents.example.yaml`](agents.example.yaml). Ids are unique slugs; a `tokenEnv` that names an unset or empty variable is a startup error, not an unauthenticated agent. |
| `LISTEN_ADDR` | `0.0.0.0:8080` | |
| `AUTH_DEV_USER` | unset | An e-mail served for requests **without** `X-Auth-Request-Email`. Development only: the orchestrator logs a warning at boot. Unset, such requests get 401. |
| `DATABASE_MAX_CONNECTIONS` | `10` | At least 2: the wakeup listener holds one connection. |
| `DISPATCHER_CONCURRENCY` | `32` | Delegations processed at the same time by this replica. |
| `OUTBOX_LEASE_SECS` | `30` | How long after a crash another replica waits before taking a delegation over (a graceful shutdown hands over at once). |
| `SHUTDOWN_GRACE_SECS` | `15` | Bound of each graceful-shutdown step. |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | Names this replica in leases. |
| `RUST_LOG` / `LOG_FORMAT` | `info` / `json` | `LOG_FORMAT=text` for humans. |

The identity header is only trustworthy behind a proxy (oauth2-proxy) that
strips client-supplied copies; run the orchestrator only behind one.

### Container image

```sh
docker build -t orchestrator orchestrator     # context: this directory
docker run --rm -p 8080:8080 -e DATABASE_URL=... -e AGENTS_FILE=/agents.yaml \
  -v "$PWD/orchestrator/agents.yaml:/agents.yaml:ro" orchestrator
```

Multi-stage (cargo-chef for the dependency layer), running as uid 65532 on
`gcr.io/distroless/cc-debian12:nonroot` with the same Debian 12 glibc as the
builder. CI builds it on every pull request, runs it against a Postgres service
(numeric non-root user, `/healthz` and `/readyz` answer, the API refuses requests
without identity, SIGTERM exits 0) and, on `main`, pushes
`ghcr.io/vymalo/another-agentic-system/orchestrator:sha-<7 chars>` and
`:latest` (see [the workflow](../.github/workflows/orchestrator.yml)).
Probes: `/healthz` (503 while shutting down) and `/readyz` (also needs the
database to answer).

### Shutdown

On SIGTERM or SIGINT the process drains instead of dying, so a rolling update
neither drops requests nor waits for leases to expire.

```mermaid
sequenceDiagram
  participant K as Kubernetes
  participant O as orchestrator
  participant S as HTTP server
  participant D as Dispatcher
  participant DB as Postgres
  K->>O: SIGTERM
  O->>O: /readyz and /healthz answer 503
  O->>S: stop accepting, and open SSE streams end once caught up
  O->>D: stop
  D->>DB: release this replica's leases
  S-->>O: drained (bounded by SHUTDOWN_GRACE_SECS)
  O->>DB: close the pool
  O-->>K: exit 0
```

```mermaid
stateDiagram-v2
  [*] --> Starting
  Starting --> Ready: migrated, listener attached, server bound
  Starting --> [*]: bad config (exit 78), unreachable database (exit 69), listen address taken (exit 71)
  Ready --> Draining: SIGTERM / SIGINT
  Ready --> [*]: server or dispatcher died or panicked (exit 70)
  Draining --> [*]: drained (exit 0)
```

## Crates

The workspace follows the ports-and-adapters split of
[ADR 0009](../docs/decisions/0009-swappable-implementations-at-build-time.md):
the service is written against traits, implementations are separate crates, and
the binary is only a composition. Swapping an implementation is a build-time
change of the composition root, never a runtime plugin.

| Directory | Package | Role |
|---|---|---|
| `crates/core` | `orch-core` | Pure: contract types (`ThreadState`, `Event`, `EventKind`, `Actor`, …) and `transition`. No async, no I/O. |
| `crates/ports` | `orch-ports` | Traits `ThreadStore`, `Wakeup`, `AgentClient`, `Clock`, `IdGen`; feature `testkit` adds in-memory implementations, a scripted fake agent and the conformance testkit. |
| `crates/app` | `orch-app` | Thread service (`transition` + optimistic commit loop, live event streams) and the durable outbox `Dispatcher`, written against the ports. |
| `crates/api` | `orch-api` | axum 0.8 routes for every operation of the contract, proxy-identity auth (fail closed), RFC 9457 problems, SSE. |
| `crates/store-postgres` | `orch-store-postgres` | `ThreadStore` + `Wakeup` on Postgres (sqlx): per-thread `seq` from a counter row in the writing transaction, outbox claims with `FOR UPDATE SKIP LOCKED` leases, `LISTEN/NOTIFY`, embedded idempotent migrations. |
| `crates/agent-a2a` | `orch-agent-a2a` | `AgentClient` over `a2a-client-lf` (A2A 1.0): live card and release-channels discovery, streaming delegation, resubscribe, polling, cancel. |
| `crates/testsupport` | `orch-testsupport` | Test-only: an in-process fake A2A agent (`a2a-server-lf`), a running orchestrator on a TCP port, chat and SSE clients; the executable `orch-fake-agent` serves two scripted agents for the browser tests (`web/e2e-system`) and is never part of the image. |
| `crates/e2e` | `orch-e2e` | Tests only: chat API + dispatcher + A2A adapter + fake agent over real HTTP, on either store. |
| `bin/orchestrator` | `orchestrator` | The composition root: environment and `AGENTS_FILE` parsing (`config.rs`, unit-tested) and the wiring, startup and graceful shutdown (`boot.rs`). No logic of its own. |

Dependency direction: `core` ← `ports` ← `app` ← `api`; adapters
(`store-postgres`, `agent-a2a`) implement the ports; only `bin/orchestrator`
depends on all of them.

## Behaviour worth knowing

- **Identity.** The API trusts `X-Auth-Request-Email` and answers 401 without it
  on every path except `/healthz` and `/readyz`. `AUTH_DEV_USER` (an e-mail)
  supplies an identity only when it is set. The header is only trustworthy
  behind a proxy such as oauth2-proxy that strips client-supplied copies.
- **`thread_state` events** are appended only when a thread *enters* `blocked`,
  `done`, `failed` or `cancelled`; entering `queued`/`working` is implied by
  `user_message` / `agent_status`.
- **No inbox table.** The chat API runs the transition inside the request and
  writes the events and the outbox row in one transaction, so redeliveries
  cannot happen on this path. The inbox of `docs/orchestrator.md` arrives with
  the webhook/MCP inputs.
- **Durability.** A delegation is an outbox row claimed under a lease. A worker
  that dies leaves the row to be re-claimed; `sent_at` tells the next worker to
  resume the agent's task (resubscribe, then poll) rather than send again, and
  every stored agent update carries an idempotency key.

## A2A adapter

- **Protocol.** A2A 1.0 only (the pinned `a2a-*-lf` crates do not speak 0.3):
  `SendStreamingMessage`, `SubscribeToTask` (the port's `resubscribe`),
  `GetTask`, `CancelTask`, `ListTasks`.
- **Live cards.** Every operation reads the agent card fresh; nothing about
  releases is cached (ADR 0008). `releases` is offered only when the card
  declares the release-channels extension with well-formed parameters, and a
  selected release is refused (never run as the default) when the live card no
  longer offers the extension.
- **Errors.** The card request keeps its HTTP status: 401/403 are
  `Unauthenticated`, 429 is `RateLimited` (with its `Retry-After`, capped at
  an hour), 408 and 5xx are `Unreachable`, other 4xx are `Rejected`. The SDK
  drops the status of a JSON-RPC call, so those failures are classified by
  JSON-RPC code and by the SDK's message prefixes: connection failures are
  retryable `Unreachable`, an answer that is not JSON-RPC (a proxy's 401 page)
  is a retryable-but-bounded `Protocol`, invalid requests and missing
  extensions are permanent `Rejected`. The transport error is kept as the
  error's `source` and never reaches the chat.
- **Limits worth knowing.** `SubscribeToTask` only works for tasks executing in
  the answering process, hence the dispatcher's `GetTask` polling fallback.
  `Task.history` is not replayed. An artifact that is not marked `lastChunk` is
  emitted when the agent's next event arrives. `find_task_by_message` only
  finds messages the agent records in the task history.

## Errors

Every error enum implements `orch_core::Classify`: a variant says what
happened, its `ErrorClass` says what to do. The dispatcher's retry decision,
the HTTP status and the exit code all match on the class, never on a variant.
A message describes its own layer only; `orch_core::report` prints the whole
chain (`a: b: c`) and is used where an error is flattened: logs and the outbox
row's `last_error`. What the chat's users see is fixed text per class
(`AgentError::public_detail`), plus the agent's own message where it gave one
about the request; transport text (URLs, proxy bodies) stays in the log.

| Error | Class | HTTP (RFC 9457) |
|---|---|---|
| thread not found | `NotFound` | 404 |
| invalid request | `Invalid` | 400 |
| thread finished, input invalid in the state | `Rejected` | 409 |
| the optimistic commit loop lost every race | `Conflict` | 503, `Retry-After: 1` |
| store unreachable | `Transient` | 503, `Retry-After: 5` |
| an agent is rate limiting (card check) | `RateLimited` | 503, the agent's `Retry-After` |
| an agent failed (card unreachable, refused) | any | 502 |
| corrupt data, a rejected statement, a bug | `Corrupt`, `Internal` | 500 |

Exit codes (sysexits.h) are found by walking the error chain: 78 configuration,
69 Postgres unreachable at boot, 71 listen address unavailable, 70 a half of the
service stopped or panicked, 1 anything else.

## MVP simplifications

- **Static agents.** The agent directory is the `AGENTS_FILE`, read once at
  boot; changing it means a restart. Agent cards themselves are read live.
- **One user identity source.** The proxy header, or the dev user. No sessions,
  no roles: a user sees exactly their own threads.
- **Every replica migrates.** Boot runs the embedded migrations; sqlx's
  advisory lock serialises replicas, so a rolling update is safe. There is no
  separate migration job.
- **Polling backs up every wakeup.** `LISTEN/NOTIFY` makes things prompt, but
  the dispatcher and the SSE streams also poll, so a lost notification only
  costs latency.
- **One agent per thread, no planner.** A thread delegates to the agent chosen
  at creation; verify/rework, reviewers and other inputs are later MVP steps.
- **Follow-ups on a `done` thread are refused (409)** rather than starting a new
  task; start a new thread.

## Contract issues found while implementing

`docs/api/chat-api.yaml` is binding and was not edited; these are for the next
version of the contract.

1. `agent_status.status` uses A2A spelling (`input_required`, `canceled`) while
   `ThreadState` says `cancelled`, and `status` is an untyped string.
2. `EventData` is an untyped bag; the per-kind shapes live only in comments (the
   `agent_message` comment omits `messageId`). A `oneOf` with a discriminator on
   `kind` would fix it.
3. SSE without `Last-Event-ID` is unspecified: implemented as a replay from seq 1.
4. `cancelThread` documents no 409/400: cancelling a finished thread is a 202 no-op.
5. `postMessage`, `listThreads` and `listEvents` document no 400 although their
   inputs are constrained; a malformed `threadId` is a 404.
6. The 409 response combines `$ref` with a sibling `description` (valid in 3.1,
   breaks 3.0 tooling); `Problem` lacks `instance`.
7. `/healthz` and `/readyz` define no body (text/plain is returned).
8. `Agent.releases.revisions` is `string[]` while the release-channels v1 card
   has `[{name, createdAt}]` (mapped to the names).
9. `listAgents` cannot say "card unreachable", which ADR 0008 wants shown.
10. When `thread_state` events are emitted, and what "newest first" means for
    `listThreads`, are unspecified (here: on entering blocked/done/failed/
    cancelled; creation order).

The issue text names the A2A 0.3 methods; the pinned `-lf` crates speak A2A 1.0
(`SendStreamingMessage`, `SubscribeToTask`, `GetTask`, `CancelTask`).

## Test

```sh
cd orchestrator
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # no database: Postgres variants skip
ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test \
  cargo test --workspace                                 # everything, against a real Postgres
```

Without `ORCH_TEST_DATABASE_URL` the Postgres variants print a skip notice and
pass; CI always sets it (a `postgres:16` service). CI also refuses a pull request
that edits, renames or deletes an already applied migration. With it set:

- **Store conformance.** The `ThreadStore`/`Wakeup` testkit runs against both
  the in-memory and the Postgres store.
- **End to end, on both stores.** Every scenario of `crates/e2e` runs twice, as
  `<scenario>::memory` and `<scenario>::postgres`: the acceptance sequence over
  SSE and `GET /events`, restart safety (a killed instance and a new one on the
  *same* database, no gap and no duplicate), SSE resume, blocked/follow-up,
  cancel, releases, agent auth, what the agent says (`agent_message`, once,
  also across a crash). The tests that need separate connections (a stream on
  one replica fed by another, a cancel through a replica that runs no
  dispatcher) are Postgres-only. Each Postgres test gets its own schema
  (`search_path`), so they run in parallel and never see each other; every
  simulated instance opens its own pool and listener, like separate processes.
  Schemas older than an hour are dropped by later runs.
- **The binary.** `bin/orchestrator/tests/smoke.rs` starts the built executable
  against the test database and a fake agent: `/healthz`, `/readyz`, 401 without
  identity, a thread that completes with the bearer from `tokenEnv`, JSON logs,
  and a clean exit on SIGTERM; and, as two real processes on one database, a
  SIGKILL mid-task that the second process finishes (the message reaches the
  agent once), threads served by either process, and a SIGTERM with a running
  task that exits within `SHUTDOWN_GRACE_SECS`. Its configuration-error tests (and the unit tests
  in `config.rs`) need no database; the unreachable-database one waits out sqlx's
  30 s connect timeout.

- **Golden transcripts.** `crates/e2e/tests/golden.rs` writes what the orchestrator emits for
  each scripted agent behaviour to [`docs/api/examples`](../docs/api/examples/README.md) and
  fails when they differ (`UPDATE_GOLDEN=1` rewrites them). The web replays them.

The contract conformance test (`crates/api/tests/conformance.rs`) starts the
real router on a TCP port over the in-memory stack, drives every operation and
validates each response body against the schemas of the contract.
