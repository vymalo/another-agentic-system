# Orchestrator

A stateless Rust service that speaks AG-UI to people
([`docs/api/agui.md`](../docs/api/agui.md)), keeps a small resource API beside it
([`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml)) and delegates each
thread to one configured A2A agent. Every thread is an event log in Postgres;
the process itself keeps nothing, so a restart mid-task loses nothing. Design:
[`docs/orchestrator.md`](../docs/orchestrator.md); decisions: ADRs 0001, 0004,
0007, 0008, 0009 in [`docs/decisions/`](../docs/decisions/).

> **Status:** MVP steps 1–2 of issue #9 are implemented: the AG-UI surface (run,
> connect, capabilities; A2UI surfaces and actions), the resource API, the
> durable dispatcher, the A2A adapter, the Postgres store and the runnable
> binary and image. The legacy chat API interaction routes were removed on 2026-09-30. The planner, verify/rework, reviewers and the MCP/webhook
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

Migrations are applied at boot. Then, for example (`AUTH_DEV_USER` above supplies the identity,
so no header is needed here):

```sh
curl -s localhost:8080/api/agents
# a run: the thread id is a UUID you mint; the response streams until the run ends
ID=$(uuidgen | tr A-F a-f)
curl -sN -X POST localhost:8080/agui/agents/coder -H 'content-type: application/json' \
  -H 'accept: text/event-stream' -d '{
    "threadId": "'$ID'", "runId": "'$(uuidgen | tr A-F a-f)'", "state": {}, "tools": [], "context": [],
    "messages": [{"id": "'$(uuidgen | tr A-F a-f)'", "role": "user", "content": "say hello"}],
    "forwardedProps": {}}'
# the whole thread, replayed and followed; ends when the replay is done and no run is open
curl -sN -H 'accept: text/event-stream' "localhost:8080/agui/threads/$ID/connect?mode=run"
curl -s localhost:8080/api/threads/$ID     # the thread's state, from the resource API
```

`dev/try-thread.sh` does the same with `jq` ([`dev/README.md`](../dev/README.md)). The legacy
`POST /api/threads` (and `…/messages`, `…/events`, `…/stream`) was removed on 2026-09-30 (ADR 0012):
those routes answer 404, but `POST /api/threads` answers 405, because its path is shared with the
resource API's `GET /api/threads`.

### Configuration

Every setting is a command-line flag with an environment fallback (`orchestrator
--help` lists both); the variables below are what deployments set, and a flag
wins over its variable. The process refuses to start, with a message naming the
culprit, when a value is wrong (exit code 78; a malformed command line exits 2).
An empty value counts as unset.

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string. Never logged. |
| `AGENTS_FILE` | required | YAML list of `{id, name, transport?, cardUrl?, tokenEnv?, agent?, gate?}` (`gate` is the entry's verification gate, see the `ORCH_GATE` rows below and [`bin/orchestrator`](bin/orchestrator/README.md#the-verification-gate); `transport` is `a2a`, the default, which needs `cardUrl`; `local` is an in-process agent named by `agent`, served only by a build with the Cargo feature `agent-local` and refused at startup otherwise), see [`agents.example.yaml`](agents.example.yaml). Ids are unique slugs; a `tokenEnv` that names an unset or empty variable is a startup error, not an unauthenticated agent. The first entry is the default agent the chat UI preselects ([ADR 0014](../docs/decisions/0014-adam-coder-default-agent-over-a2a.md)). |
| `LISTEN_ADDR` | `0.0.0.0:8080` | Control plane: the API. Worker: the probes only. |
| `ORCH_ROLE` | `all` | What this process runs: `all`, `control-plane` (server, API and surfaces; no dispatcher) or `worker` (dispatcher, and a router with only `/healthz` and `/readyz`). Flag `--role`; the enum is `adam_host::Role` ([ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)). The role table is in [`bin/orchestrator`](bin/orchestrator/README.md#roles). An unknown value is a startup error. |
| `ORCH_SURFACES` | `agui` | Comma-separated interaction surfaces to mount (flag `--surfaces`). Known: `agui`, the AG-UI routes `POST /agui/agents/{agentId}`, `GET /agui/threads/{threadId}/connect` and `GET /agui/agents/{agentId}/capabilities` ([ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md), [`docs/api/agui.md`](../docs/api/agui.md)). The legacy `chat-api` surface (`createThread`, `postMessage`, `listEvents`, `streamEvents`) was **removed on 2026-09-30**: naming it fails closed (exit 78, an error that says it was removed and points to AG-UI, see [`bin/orchestrator`](bin/orchestrator/README.md#surfaces)). An unknown name, an empty list (`,`), a repeat, or a surface whose Cargo feature (`surface-agui`) is not in the build is a startup error. The resource API and health are always mounted. |
| `AUTH_DEV_USER` | unset | An e-mail served for requests **without** `X-Auth-Request-Email`. Development only: the orchestrator logs a warning at boot. Unset, such requests get 401. |
| `DATABASE_MAX_CONNECTIONS` | `10` | At least 2: the wakeup listener holds one connection. |
| `DISPATCHER_CONCURRENCY` | `32` | Delegations processed at the same time by this replica. |
| `OUTBOX_LEASE_SECS` | `30` | How long after a crash another replica waits before taking a delegation over (a graceful shutdown hands over at once). Also the lease of a local agent's run. |
| `AGENT_LOCAL_CONCURRENCY` | `4` | Only in a build with the Cargo feature `agent-local`: runs of local agents stepped at once by this replica (at least 1). The local agents open a pool of their own, this plus 4 connections, on top of `DATABASE_MAX_CONNECTIONS`. |
| `SHUTDOWN_GRACE_SECS` | `15` | Bound of each graceful-shutdown step. |
| `ORCH_GATE` | none | The sources every job must pass before it is `done`: a comma list of `ci`, `agent-checks`, `verifier` (flag `--gate`; [ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md)). None is no gate. This build honours only `agent-checks`; `ci` and `verifier` are a startup error (78) naming the slice that enables them. |
| `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP` | `3`, `10` | Attempts a gated job's agent gets (the first included), and the most an `AGENTS_FILE` entry or a run may raise them to. |
| `ORCH_VERIFIER` | none | The verifier agent's id. Refused (78) until the verifier dispatch is built. |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | Names this replica in leases. |
| `RUST_LOG` / `LOG_FORMAT` | `info` / `json` | `LOG_FORMAT=text` for humans. |

The identity header is only trustworthy behind a proxy (oauth2-proxy) that
strips client-supplied copies; run the orchestrator only behind one.

### Local agents (Cargo feature `agent-local`)

An `AGENTS_FILE` entry with `transport: local` and `agent: echo` is an agent that runs inside the orchestrator
process, on the durable adam-rs runtime ([ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md),
[`docs/orchestrator.md`](../docs/orchestrator.md#local-agents)). The feature is **off by default**, so the default
binary and image link no adam-rs runtime and refuse such an entry at startup (exit 78, naming the feature). Build one
that hosts them with `cargo run -p orchestrator --features agent-local`, or the image with
`docker build --build-arg ORCH_FEATURES=agent-local`; [`dev/agents.local-echo.yaml`](../dev/agents.local-echo.yaml) is a
ready file.

* **Where the state is.** In the orchestrator's own Postgres, in the tables `orch_agent_runs`, `orch_agent_journal` and
  `orch_agent_meta`, notified on the channels `orch_agent_events` and `orch_agent_signals`. They are created at boot in
  every role, next to the orchestrator's own tables, and nothing else uses the prefix `orch_agent_`. A worker that dies
  mid-step loses nothing: another one resumes the run when its lease (`OUTBOX_LEASE_SECS`) has expired. Nothing deletes
  finished runs yet ([open question 28](../docs/open-questions.md)).
* **Roles.** `worker` and `all` step runs; `control-plane` only starts and reads them.
* **Deployment notes.** Each replica of a process that has a local agent configured holds
  `AGENT_LOCAL_CONCURRENCY + 4` more database connections than `DATABASE_MAX_CONNECTIONS` (a pool of its own, one
  connection of which is the notifier's listener, which needs a session: a direct connection or a session-mode pooler,
  not a transaction-mode one). A step that runs tools can starve the process that hosts it (lesson 1), so give such a
  worker its own pod and limits.

### Container image

```sh
docker build -t orchestrator orchestrator     # context: this directory
docker run --rm -p 8080:8080 -e DATABASE_URL=... -e AGENTS_FILE=/agents.yaml \
  -v "$PWD/orchestrator/agents.yaml:/agents.yaml:ro" orchestrator
```

The build fetches git dependencies from `github.com/vymalo/another-adam-rs` (a public repository, pinned by one
commit sha): `adam-host` always, and with the build argument `ORCH_FEATURES=agent-local` the runtime, task backend and
Postgres crates too, so the builder needs network access to github.com as well as crates.io.
Multi-stage (cargo-chef for the dependency layer), running as uid 65532 on
`gcr.io/distroless/cc-debian12:nonroot` with the same Debian 12 glibc as the
builder. CI builds it on every pull request, runs it against a Postgres service
(numeric non-root user, `/healthz` and `/readyz` answer, the API refuses requests
without identity, SIGTERM exits 0) and, on `main`, pushes
`ghcr.io/vymalo/another-agentic-system/orchestrator:sha-<7 chars>` and
`:latest` (see [the workflow](../.github/workflows/orchestrator.yml)).
Probes: `/healthz` (503 while shutting down) and `/readyz` (also needs the
database to answer).
`/metrics` serves the outbox queue as Prometheus text on every role, without an identity
(see [`bin/orchestrator`](bin/orchestrator/README.md#logs-and-metrics)). Every log line carries
the process's `role` and `instance`.

### Shutdown

On SIGTERM or SIGINT the process drains instead of dying, so a rolling update
neither drops requests nor waits for leases to expire. The order is that of
`adam_host::Host`: the control plane first, then the workers. A process that
does not run a half skips its step (a `worker` has no server to drain, and a
`control-plane` no dispatcher).

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
  Ready --> [*]: a component (server or dispatcher) died or panicked (exit 70)
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
| [`crates/core`](crates/core/README.md) | `orch-core` | Pure: contract types (`ThreadState`, `Event`, `EventKind`, `Actor`, …) and `transition`. No async, no I/O. |
| [`crates/ports`](crates/ports/README.md) | `orch-ports` | Traits `ThreadStore`, `Wakeup`, `AgentClient`, `Clock`, `IdGen`, and `ByTransport` (routes an endpoint to the A2A or the local client); feature `testkit` adds in-memory implementations, a scripted fake agent and the conformance testkit. |
| [`crates/agui-proto`](crates/agui-proto/README.md) | `orch-agui-proto` | AG-UI 1.0 wire types as closed serde enums (all 31 events, `RunAgentInput`), the vendored official JSON Schema, and a `testkit` that validates against it. No `orch-*` dependencies. |
| [`crates/agui-projection`](crates/agui-projection/README.md) | `orch-agui-projection` | Pure: the AG-UI view of the event log (`Projector`: events to frames, audiences, resume preamble) and the translation of a `RunAgentInput` to core inputs. Depends on `orch-core` and `orch-agui-proto` only; no async, no I/O. |
| [`crates/app`](crates/app/README.md) | `orch-app` | Thread service (`transition` + optimistic commit loop, live event streams) and the durable outbox `Dispatcher`, written against the ports. |
| [`crates/api`](crates/api/README.md) | `orch-api` | The always-mounted HTTP edge: proxy-identity auth (fail closed), RFC 9457 problems, the resource API (agents, threads, cancel), health, and `SurfaceRoutes`, the mounting point of interaction surfaces. |
| [`crates/store-postgres`](crates/store-postgres/README.md) | `orch-store-postgres` | `ThreadStore` + `Wakeup` on Postgres (sqlx): per-thread `seq` from a counter row in the writing transaction, outbox claims with `FOR UPDATE SKIP LOCKED` leases, `LISTEN/NOTIFY`, embedded idempotent migrations. |
| [`crates/agent-a2a`](crates/agent-a2a/README.md) | `orch-agent-a2a` | `AgentClient` over `a2a-client-lf` (A2A 1.0): live card and release-channels discovery, streaming delegation, resubscribe, polling, cancel. |
| [`crates/agent-adam`](crates/agent-adam/README.md) | `orch-agent-adam` | `AgentClient` over adam-rs agents hosted in this process (`transport: local`), journaled in the orchestrator's Postgres under `orch_agent_`; only with the binary's feature `agent-local` (off by default). |
| [`crates/a2a-mapping`](crates/a2a-mapping/README.md) | `orch-a2a-mapping` | Pure: the mapping from A2A 1.0 stream items and tasks to `AgentEnvelope`s and idempotency keys (`StreamMapper`, `snapshot`). No I/O, no async, no HTTP client. |
| [`crates/testsupport`](crates/testsupport/README.md) | `orch-testsupport` | Test-only: an in-process fake A2A agent (`a2a-server-lf`), a running orchestrator on a TCP port, clients for the resource API and the AG-UI routes; the executable `orch-fake-agent` serves two scripted agents for the browser tests (`web/e2e-system`) and is never part of the image. |
| [`crates/e2e`](crates/e2e/README.md) | `orch-e2e` | Tests only: AG-UI surface, dispatcher and A2A adapter against a fake agent over real HTTP, on either store. |
| [`bin/orchestrator`](bin/orchestrator/README.md) | `orchestrator` | The composition root: flags, environment (clap) and `AGENTS_FILE` parsing (`config.rs`, unit-tested), the role (`ORCH_ROLE`, `adam_host::Role`), and the surfaces to mount and the wiring, startup and graceful shutdown on `adam_host::Host` (`boot.rs`). No logic of its own. |

Every crate has its own README (role, public API, environment, tests); update it
in the same change as the crate's API, environment variables or tests. The docs
check fails when one is missing.

Dependency direction: `core` ← `ports` ← `app` ← `api` ← the surface crates; adapters
(`store-postgres`, `agent-a2a`, `agent-adam`; the last two build on the pure `a2a-mapping`) implement the ports;
only `bin/orchestrator` depends on all of them.

## Behaviour worth knowing

- **Identity.** The API trusts `X-Auth-Request-Email` and answers 401 without it
  on every path except `/healthz` and `/readyz`. `AUTH_DEV_USER` (an e-mail)
  supplies an identity only when it is set. The header is only trustworthy
  behind a proxy such as oauth2-proxy that strips client-supplied copies.
- **`thread_state` events** are appended only when a thread *enters* `blocked`,
  `done`, `failed` or `cancelled`; entering `queued`/`working` is implied by
  `user_message` / `agent_status`.
- **No inbox table.** A surface (the AG-UI run route)
  runs the transition inside the request and writes the events and the outbox
  row in one transaction, so redeliveries cannot happen on this path (a retried
  AG-UI run carries an idempotency key on the event and attaches). The inbox of `docs/orchestrator.md` arrives with
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
69 Postgres unreachable at boot, 71 listen address unavailable, 70 a component of
the service stopped or panicked, 1 anything else.

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

Items 3 and 5 concern operations that were removed on 2026-09-30 (`streamEvents`, `postMessage`,
`listEvents`); they are kept as the record of what the contract left open.

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
  the AG-UI run and connect routes and the log, restart safety (a killed instance and a new one on the
  *same* database, no gap and no duplicate), SSE resume (`agui_connect`), blocked/follow-up,
  cancel, releases, agent auth, what the agent says (`agent_message`, once,
  also across a crash). The tests that need separate connections (a stream on
  one replica fed by another, a cancel through a replica that runs no
  dispatcher) are Postgres-only. Each Postgres test gets its own schema
  (`search_path`), so they run in parallel and never see each other; every
  simulated instance opens its own pool and listener, like separate processes.
  Schemas older than an hour are dropped by later runs.
- **The binary.** `bin/orchestrator/tests/smoke.rs` starts the built executable
  against the test database and a fake agent: `/healthz`, `/readyz`, 401 without
  identity, a thread run over AG-UI (the default surface) that completes with the bearer from `tokenEnv`, JSON logs,
  and a clean exit on SIGTERM; the default surfaces (`agui` and the resource API, the removed legacy
  routes answering 404, or 405 for `POST /api/threads`); and, as two real processes on one database, a
  SIGKILL mid-task that the second process finishes (the message reaches the
  agent once), threads served by either process, and a SIGTERM with a running
  task that exits within `SHUTDOWN_GRACE_SECS`. It also runs the CLI: `--help` lists every flag
  and variable, each variable is read from the environment alone, a flag wins
  over its variable, an unknown flag is a usage error, and `ORCH_SURFACES` or `--surfaces` naming the
  removed `chat-api` exits 78 with the message in [`bin/orchestrator`](bin/orchestrator/README.md#surfaces). The multi-process tests drive their threads over
  AG-UI and read the log as a viewer does (the connect stream).
  Its configuration-error tests (and the unit tests
  in `config.rs`) need no database; the unreachable-database one waits out sqlx's
  30 s connect timeout.

- **Golden transcripts.** `crates/e2e/tests/golden.rs` writes what the orchestrator emits for
  each scripted agent behaviour to [`docs/api/examples`](../docs/api/examples/README.md) and
  fails when they differ (`UPDATE_GOLDEN=1` rewrites them). The web replays them.

The contract conformance test (`crates/api/tests/contract.rs`) starts the real router (the resource
API, with no interaction surface) on a TCP port over the in-memory stack, drives every operation it
serves (health, the agent list, the thread list, a thread, cancel) and validates each response body
against the schemas of the contract; it also validates the events of a real thread, and the golden
transcripts, against the contract's `Event` schema. `crates/surface-agui/tests/contract.rs` does the same for the
`/agui/*` operations, whose documented statuses must be the answered ones.
