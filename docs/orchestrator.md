# Orchestrator

The orchestrator is a Rust service that owns every thread's state. Its replicas
are stateless; all state is in Postgres (ADR 0001, ADR 0007). The design is that
it accepts input from **anything** (A2A, the chat, MCP, webhooks, timers, …) and
produces output to **anything** (A2A, MCP tools, the chat, Slack, GitHub,
webhooks, …). The way to get that without rewriting the core per protocol is
**ports and adapters**:

- Every input is translated at the edge into one canonical `Input`.
- Every output is one canonical `Command`.
- In the middle sits one **pure** function: `(state, input) → (next state, commands)`.

Adding a protocol means adding an adapter crate. The state machine does not
change.

> **What is built.** Facts in this page are marked **Built** (present in
> `orchestrator/` and checked against the code on 2026-09-29) or **Planned**
> (design only). Today: the chat surface's API, the pure core, the Postgres
> store, the durable dispatcher, the A2A client adapter, the AG-UI wire types
> and the pure AG-UI projection. Not yet: the AG-UI HTTP surface, an A2A or MCP
> server, webhooks, timers, an inbox, MCP tools, and the model endpoint. The
> whole picture, with diagrams, is in [Architecture: as built](architecture.md#as-built).

## It is symmetric

The orchestrator is meant to be simultaneously a server and a client of each
protocol:

| Protocol | As a server (input) | As a client (output) | Status |
|---|---|---|---|
| A2A | Other agents hand it jobs | Delegates each thread to a configured A2A agent, whatever hosts it | Client **built** (`orch-agent-a2a`); server **planned** (`orch-surface-a2a`, ADR 0012) |
| MCP | Claude Code, opencode or any MCP client can `start_job`, `get_job`, `answer` | Calls tools: GitHub, docs, search, … | **Planned** |
| Chat | The web posts user messages | Appends messages and cards to the thread | The chat API is **built** (`orch-surface-chat-api`, deprecated); AG-UI wire types and projection **built**, its HTTP surface **planned** |
| Webhooks | GitHub, CI, Slack events | Slack posts, outgoing webhooks | **Planned** |
| Timers | Scheduled events (timeouts, reminders, cron) | Schedules new timers | **Planned** |

The chat row's user-facing protocol is **AG-UI 1.0**, a pure projection of the event log, with a
small REST resource API beside it ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md),
binding in [`api/agui.md`](api/agui.md)). Each inbound surface (AG-UI, the legacy chat API, later
A2A) is an adapter crate behind a Cargo feature, and which ones are mounted is configuration
(`ORCH_SURFACES`). **Built:** the mechanism and the `chat-api` surface (the default and the only
name the binary accepts today). **Planned:** `agui` and `a2a`.

*Design, not built:* every event records its **origin**, and a `Reply` command goes back to
wherever the request came from: a job started over A2A gets A2A task updates; one started over MCP
gets MCP progress notifications; one started in the chat gets chat messages. Today every log event
records an `Actor` (`user`, `agent` or `system`), and the only reply channel is the thread's own
event log, which every surface reads.

## Crate layout

**Built.** The workspace (`orchestrator/Cargo.toml`, `members = ["crates/*", "bin/*"]`) has eleven
library crates and one binary. Dependencies below are read from the `Cargo.toml` files. Each crate has
a README with its API, environment and tests; the [workspace README](../orchestrator/README.md#crates)
has the same map with one line per crate.

```mermaid
flowchart TB
  subgraph G_CORE["Core: pure, no async, no I/O"]
    core["<b>orch-core</b><br/>ThreadState, Event, Input, Command,<br/>transition(), error classes"]
  end
  subgraph G_PORTS["Ports: traits only"]
    ports["<b>orch-ports</b><br/>ThreadStore, Wakeup, AgentClient,<br/>Clock, IdGen, Ports<br/>feature testkit: memory impls + conformance"]
  end
  subgraph G_ADAPT["Adapters: implement the ports"]
    pg["<b>orch-store-postgres</b><br/>ThreadStore + Wakeup<br/>sqlx, LISTEN/NOTIFY, migrations"]
    a2a["<b>orch-agent-a2a</b><br/>AgentClient over A2A 1.0<br/>a2a-client-lf"]
  end
  subgraph G_MAP["Pure helper of the A2A adapter: no async, no I/O"]
    a2amap["<b>orch-a2a-mapping</b><br/>A2A values to envelopes<br/>and idempotency keys"]
  end
  subgraph G_APP["Application: written against the ports"]
    app["<b>orch-app</b><br/>App: transition + commit loop, event_stream<br/>Dispatcher: durable outbox worker"]
  end
  subgraph G_EDGE["HTTP edge"]
    api["<b>orch-api</b><br/>identity, RFC 9457 problems, resource API,<br/>health, SurfaceRoutes"]
  end
  subgraph G_SURF["Interaction surfaces: mounted by ORCH_SURFACES"]
    chat["<b>orch-surface-chat-api</b><br/>legacy createThread, postMessage,<br/>listEvents, streamEvents"]
    surfagui["<b>orch-surface-agui</b><br/>planned: run, connect, capabilities"]:::planned
  end
  subgraph G_AGUI["AG-UI: pure, no async, no I/O"]
    proto["<b>orch-agui-proto</b><br/>AG-UI 1.0 wire types, vendored schema,<br/>feature testkit"]
    proj["<b>orch-agui-projection</b><br/>Projector: events to frames<br/>translate: RunAgentInput to Input"]
  end
  subgraph G_BIN["Binary: the composition root"]
    bin["<b>orchestrator</b><br/>flags, env, AGENTS_FILE, wiring, shutdown<br/>features: surface-chat-api (default)<br/>planned: surface-agui"]
  end
  subgraph G_TEST["Test support: publish = false"]
    ts["<b>orch-testsupport</b><br/>fake A2A agent, test instance, clients"]
    e2e["<b>orch-e2e</b><br/>end-to-end tests, memory and Postgres"]
  end

  ports --> core
  pg --> ports
  a2a --> ports
  a2a --> a2amap
  a2amap --> ports
  app --> ports
  api --> app
  api --> ports
  chat --> api
  chat --> app
  proj --> proto
  proj --> core
  bin --> app
  bin --> api
  bin --> pg
  bin --> a2a
  bin -. "feature surface-chat-api" .-> chat
  bin -. "feature surface-agui, planned" .-> surfagui
  surfagui -.-> api
  surfagui -.-> app
  surfagui -.-> proj
  ts --> api
  ts --> app
  ts --> chat
  e2e -.-> ts
  e2e -.-> pg
  e2e -.-> a2a
  e2e -.-> api
  a2a -.-> ts

  classDef planned stroke-dasharray: 5 5,fill:none
```

How to read it: an arrow points from a crate to a crate it depends on; a dotted arrow is a
dependency behind a Cargo feature, a dev-dependency (tests only), or a planned one. To keep it
readable the graph omits the `orch-core` edge of every crate but `orch-agui-proto` (which has no
`orch-*` dependency at all) and the `orch-ports` edge of the binary, the surface crate and the test
crates.

Rules the graph enforces, each checkable in the manifests:

- **`orch-core` and `orch-agui-projection` are pure.** Neither depends on `tokio`, `sqlx`, `axum` or an
  HTTP client, so the compiler keeps the state machine and the AG-UI view a function of their
  inputs (ADR 0001, ADR 0004, ADR 0012).
- **Adapters depend on `orch-core` and `orch-ports` only.** `orch-store-postgres` and
  `orch-agent-a2a` name no other orchestrator crate (ADR 0009, rule 5: no implementation type in a
  port signature), except that `orch-agent-a2a` uses `orch-a2a-mapping`, its own pure helper (the
  mapping from A2A values to envelopes, itself depending on `orch-core` and `orch-ports` only and
  on no HTTP client), not another adapter.
- **`orch-app` and `orch-api` name no adapter.** Only `bin/orchestrator` depends on
  the Postgres and A2A crates and chooses them (`type Stack = PortSet<PgStore, PgWakeup,
  A2aAgentClient, SystemClock, UuidV7Ids>` in `boot.rs`).
- **A surface depends on `orch-app` and `orch-api`, never on an adapter.** `orch-api` names no surface.
- **`orch-agui-proto` depends on nothing of ours**, so it can be checked against the vendored
  AG-UI schema and moved on its own.

| Crate (directory) | Role | Status |
|---|---|---|
| `orch-core` (`crates/core`) | Contract types and `transition` | **Built** |
| `orch-ports` (`crates/ports`) | `ThreadStore`, `Wakeup`, `AgentClient`, `Clock`, `IdGen`, the `Ports` bundle; feature `testkit`: `MemoryStore`, `MemoryWakeup`, `ScriptedAgent` and the conformance macros `thread_store_conformance!`, `wakeup_conformance!`, `agent_client_conformance!` | **Built** |
| `orch-store-postgres` (`crates/store-postgres`) | `ThreadStore` + `Wakeup` on Postgres | **Built** |
| `orch-agent-a2a` (`crates/agent-a2a`) | `AgentClient` over A2A 1.0 | **Built** |
| `orch-a2a-mapping` (`crates/a2a-mapping`) | Pure mapping of A2A stream items and tasks to `AgentEnvelope`s and idempotency keys; no I/O, no async | **Built** |
| `orch-app` (`crates/app`) | `App`, `Dispatcher` | **Built** |
| `orch-api` (`crates/api`) | HTTP edge, resource API, `SurfaceRoutes` | **Built** |
| `orch-surface-chat-api` (`crates/surface-chat-api`) | Legacy interaction routes; deprecated | **Built** |
| `orch-agui-proto` (`crates/agui-proto`) | AG-UI 1.0 wire types, conformance testkit | **Built** |
| `orch-agui-projection` (`crates/agui-projection`) | `Projector`, `translate` | **Built** |
| `orch-surface-agui` | Run, connect and capabilities routes over the projection | **Planned** ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md)) |
| `orch-surface-a2a` | A2A inbound | **Planned** (ADR 0012) |
| MCP client and server, webhook, timer, GitHub and Slack adapters | The other rows of the table above | **Planned** |
| `orch-testsupport`, `orch-e2e` (`crates/testsupport`, `crates/e2e`) | Test-only | **Built** |
| `orchestrator` (`bin/orchestrator`) | The composition root | **Built** |

### Cargo features

| Crate | Feature | Default | Effect |
|---|---|---|---|
| `orchestrator` | `surface-chat-api` | yes | Compiles in `orch-surface-chat-api`; it decides what *can* be mounted, `ORCH_SURFACES` what *is* |
| `orch-ports` | `testkit` | no | In-memory implementations and the conformance testkit; enable as a dev-dependency feature in adapter crates |
| `orch-agui-proto` | `testkit` | no | `assert_conforms` and friends against the vendored schema (`jsonschema`); enable as a dev-dependency feature |

There is no Cargo feature that selects the store or the A2A client: the binary depends on
`orch-store-postgres` and `orch-agent-a2a` unconditionally, because there is one implementation of
each. ADR 0009 says the built-in implementations are features of the default binary; that switch
is not built (see the status note in [ADR 0009](decisions/0009-swappable-implementations-at-build-time.md)).

## Event flow

**Design.** This is the flow for every input, including webhooks and MCP, which need the inbox to
dedupe redeliveries:

```mermaid
sequenceDiagram
  participant In as Inbound adapters<br/>(A2A · chat · MCP · webhooks · timers)
  participant DB as Postgres
  participant C as Core (pure fn, no I/O)
  participant D as Dispatcher
  participant Out as Outbound adapters<br/>(A2A · MCP · chat · Slack · GitHub · webhooks)
  In->>In: authenticate (A2A auth, MCP OAuth, HMAC webhook signature, OIDC user)
  In-->>In: unverified → 401, never enqueued (fail closed)
  In->>DB: INSERT inbox (source, idempotency_key UNIQUE) — redeliveries dedupe here
  DB->>C: claim job (SKIP LOCKED), load state
  C->>C: transition(&state, &input) → (next, commands)
  C->>DB: ONE txn: update job (version+1), INSERT outbox, append chat events, mark inbox applied
  D->>DB: claim outbox rows (SKIP LOCKED)
  D->>Out: execute command (match on variant)
  Out-->>In: async results come back as NEW inbound events (correlated by id)
  D->>DB: delivered | retry with backoff
```

**Built today** differs in four places, all consequences of having one input path (a person using
the chat API) and one output (an A2A agent):

| Design | Built |
|---|---|
| Inbound adapters write an `inbox` row; a worker claims it and runs the transition | No inbox table. The request handler runs `transition` itself inside `App::apply` and commits state, events and outbox rows in one transaction, so a redelivery cannot happen on this path. The inbox arrives with the webhook and MCP inputs |
| Inbound events are deduplicated by `UNIQUE (source, idempotency_key)` | Events carry an optional `idempotency_key`, unique per thread (`events_idempotency`); the dispatcher derives keys from the agent's own ids, so a resumed or replayed stream never duplicates an event |
| The job row holds the state as `jsonb` | The `threads` row holds `state` as text with a `CHECK`; the state has no payload |
| Async results re-enter as new inbound events | The dispatcher turns everything the agent reports into `Input::Agent` and calls `App::apply`, the same entry point the chat API uses |

The turn as the code runs it, step by step, is a sequence diagram in
[Architecture: a chat turn](architecture.md#a-chat-turn).

## Command (outbox) lifecycle

**Built.** Two outbox kinds exist: `delegate` (send the user's text to the agent) and `cancel`
(ask the agent to cancel its task). Rows have the statuses below (`outbox.status`, `OutboxStatus`
in `orch-ports`):

```mermaid
stateDiagram-v2
  [*] --> Pending: inserted in the same txn as the state change
  Pending --> Inflight: claimed (FOR UPDATE SKIP LOCKED), lease taken, attempts + 1
  Inflight --> Inflight: heartbeat renews the lease
  Inflight --> Inflight: lease expired (worker died): another replica re-claims
  Inflight --> Pending: transient error, attempts < max, backoff
  Inflight --> Delivered: the turn ended (done, blocked, failed, cancelled), or a cancel went through
  Inflight --> Dead: permanent error, attempts exhausted, or the agent lost the task
  Pending --> Skipped: a cancel arrived before the message was sent
  Inflight --> Skipped: same, with an expired lease; or the thread was already finished
  Dead --> [*]: DeliveryFailed applied to the thread
  Delivered --> [*]
  Skipped --> [*]
```

`Dead → DeliveryFailed` is the fail-closed edge: a delegation that could not be delivered re-enters
the state machine as `Input::DeliveryFailed { reason, retryable }`, so the thread goes `blocked` (a
retryable failure: the user can send another message) or `failed` instead of carrying on as if the
step happened. The dispatcher applies it with the idempotency key `dead:<row id>`.

What the diagrams cannot say:

- **Per-thread order.** A `delegate` row is claimable only if no older `pending` or `inflight`
  `delegate` row exists for the same thread; `cancel` rows are not held back.
- **Resume, not resend.** `sent_at` is set when the agent's first frame arrives (with the A2A task
  id). A worker that re-claims a row with `sent_at` set resumes the task (`SubscribeToTask`, then
  polling `GetTask`) instead of sending the message again. On a later attempt of a row whose `sent_at` was
  never set, the worker first asks the agent whether the message id, which is the outbox row id, already
  created a task (`find_task_by_message`).
- **Defaults** (`DispatcherConfig::default()`, verified in the code 2026-09-29): 32 concurrent rows,
  30 s lease renewed every 10 s (the binary sets the lease from `OUTBOX_LEASE_SECS` and renews at a
  third of it), 5 send attempts, retry delay 1 s doubling to 60 s (or the agent's `Retry-After`
  when longer), `GetTask` polling from 1 s to 10 s with at most 10 consecutive failures, 10 cancel
  attempts, a 2 s outbox poll as a safety net under `LISTEN/NOTIFY`.
- **Shutdown.** The dispatcher stops its workers and sets `lease_until = now` on its rows, so another
  replica takes them at once ([`orchestrator/README.md`](../orchestrator/README.md#shutdown)).

### Fenced commits

A lease is a promise that lapses, not a lock: a worker paused past its lease (a stopped process, a
long GC or network stall) resumes believing it still owns the row, while another worker has claimed
it and is streaming the same task. Renewing the lease cannot help, because the paused worker cannot
renew. So the store, not the worker, decides. Every claim hands out a **fencing token**, the row's
`attempts` counter, which grows on each claim. A `Lease { id, owner, attempt }` is good only while
the row is `inflight`, held by `owner`, at exactly `attempt`. Every write a worker makes on behalf
of its row carries the lease: `renew_lease`, `mark_sent`, `retry_outbox`, `complete_outbox`, and the
thread `commit` that records what the agent reported (`Commit.lease`). A commit under a stale lease
writes nothing and answers `CommitOutcome::Fenced`; `App::apply` reports it as
`ApplyOutcome::Fenced` without retrying, and the worker logs `lease lost; the late agent result was
dropped` and stops.

```mermaid
sequenceDiagram
  participant A as Worker A (paused)
  participant S as Store (Postgres)
  participant B as Worker B
  A->>S: claim_outbox: row inflight, attempts = 1
  Note over A: paused past the lease
  B->>S: claim_outbox: lease expired, attempts = 2
  B->>S: commit(events, lease attempt 2)
  S-->>B: Applied
  Note over A: resumes, the agent finished its turn
  A->>S: commit(events, lease attempt 1)
  S->>S: lock thread, then row: attempts is 2, not 1
  S-->>A: Fenced (nothing written)
  A->>A: log "lease lost" and stop, no complete_outbox
```

```mermaid
stateDiagram-v2
  [*] --> Held: claim_outbox, attempts = n
  Held --> Held: heartbeat renews
  Held --> Expired: no renewal before lease_until
  Expired --> Held: same worker renews or commits (nobody re-claimed)
  Expired --> Superseded: another claim, attempts = n + 1
  Held --> Superseded: skipped by a cancel, or finished by its own worker
  Superseded --> Fenced: any write under the old lease
  Fenced --> [*]: refused, nothing written
```

What the diagrams cannot say:

- **Expiry alone does not fence.** A lapsed lease that nobody claimed again is still the current
  claim, so the worker's late result is kept: throwing it away would only redo work. What revokes a
  lease is a later claim, and a row that left `inflight` (delivered, dead, retried, skipped).
- **The same owner name is not enough.** The owner is one name per process, and a process that
  re-claims its own expired row is "the same owner" again. Only `attempt` tells the two claims apart.
- **Postgres.** `commit` locks the thread row, then checks the claim with
  `SELECT 1 FROM outbox WHERE id = $1 AND thread_id = $2 AND lease_owner = $3 AND attempts = $4 AND
  status = 'inflight' FOR SHARE`, before the version and idempotency checks. The share lock keeps a
  claimer out until the commit ends (`claim_outbox` skips locked rows), so the check cannot go stale.
  Lock order is thread, then outbox row, then binding; no statement takes a thread lock while
  holding an outbox row lock, so the two cannot deadlock. No migration: `attempts` and
  `lease_owner` were already there.
- **API commits carry no lease** (`None`): a user's message or a cancel is not a claim, and
  `create_thread` ignores the field.
- **What is not fenced.** `skip_unsent_delegates` and `release_leases` take no lease; they act on
  the thread's rows by status and time.

## Thread state and transitions

**Built.** The states, the events they append and the pure function that decides both are in
`orch-core`. The state diagram is in [Architecture: thread state](architecture.md#thread-state);
this is the same machine as a table (`crates/core/tests/transition_table.rs` has one test per row):

| Input | `queued` / `working` | `blocked` | `done` / `failed` / `cancelled` |
|---|---|---|---|
| `UserMessage` | State kept; append `user_message`, `Delegate` | → `queued`; same commands | `Err(Finished)` (HTTP 409) |
| `Cancel` | State kept; `RequestCancel` | State kept; `RequestCancel` | No-op |
| Agent status `submitted` | Nothing | Nothing | `Err(InvalidInState)`, dropped as late |
| Agent status `working` | `queued` → `working`; append `agent_status`. Repeated in `working`: shown only with a detail | → `working` | `Err(InvalidInState)` |
| Agent status `input_required`, `auth_required` | → `blocked`; append `agent_status`, `thread_state` | State kept; shown again only with a detail | `Err(InvalidInState)` |
| Agent status `completed` | → `done`; `agent_status`, `thread_state` | → `done` | `Err(InvalidInState)` |
| Agent status `failed`, `rejected` | → `failed` (`rejected` prefixes the detail); `agent_status`, `thread_state` | → `failed` | `Err(InvalidInState)` |
| Agent status `canceled` | → `cancelled`; `agent_status`, `thread_state` | → `cancelled` | `Err(InvalidInState)` |
| Agent artifact, agent message | State kept; append `artifact` or `agent_message` | Same | `Err(InvalidInState)` |
| `DeliveryFailed`, retryable | → `blocked`; append `error`, and `thread_state` on entering | State kept; append `error` | State kept; append `error` |
| `DeliveryFailed`, permanent | → `failed`; `error`, `thread_state` | → `failed` | State kept; append `error` |
| `CancelledBeforeStart` | → `cancelled`; `thread_state` | → `cancelled` | No-op |
| `CancelRejected` | State kept; append `error` | Same | No-op |

`thread_state` is appended only when the thread *enters* `blocked`, `done`, `failed` or
`cancelled`; entering `queued` or `working` is implied by `user_message` and `agent_status`. The
property test (`tests/properties.rs`) checks that terminal states absorb, that a `thread_state`
event names the state the thread entered, and that `completed` reaches `done` from every open state.

## Core types

**Built.** A separate crate with no async, no sqlx and no HTTP, so purity is enforced by the
compiler, not by convention. The public surface that matters, as in `crates/core/src`:

```rust
// crate `orch-core` — types + one function. No I/O.

pub enum ThreadState { Queued, Working, Blocked, Done, Failed, Cancelled }

/// Everything that can happen to a thread, already protocol-neutral.
pub enum Input {
    UserMessage { user: UserId, text: String, message_id: Option<String>, run_id: Option<String> },
    Cancel { user: UserId },
    Agent { agent: AgentId, revision: Option<String>, update: AgentUpdate },
    DeliveryFailed { reason: String, retryable: bool },
    CancelledBeforeStart,
    CancelRejected { reason: String, retryable: bool },
}

/// What the application must do. The application turns these into ONE store commit.
pub enum Command {
    Append(EventDraft),          // → an event in the thread's log
    Delegate { text: String },   // → an outbox row, kind `delegate`
    RequestCancel,               // → an outbox row, kind `cancel`
}

/// The log the chat renders: `seq`, thread, time, `Actor { user | agent | system, name, revision? }`, body.
pub enum EventBody { UserMessage(_), AgentMessage(_), AgentStatus(_), Artifact(_), ThreadState(_), Error(_) }

pub enum TransitionError {
    Finished { state: ThreadState },                        // a user message on a finished thread
    InvalidInState { state: ThreadState, input: &'static str }, // a late agent update
}

pub fn transition(state: &ThreadState, input: &Input)
    -> Result<(ThreadState, Vec<Command>), TransitionError>;
```

An agent is a configured A2A agent-card URL and nothing host-specific. `AgentEndpoint { id, transport }`
in `orch-ports` says how to reach it, and `AgentTransport` is a closed enum (ADR 0004) with one
variant, `A2a { card_url, bearer }`. It lives in the ports, not in the core, because it carries a
secret (the resolved bearer, redacted in `Debug`) that `transition` never sees. A second variant
arrives with its implementation, not before: in-process adam agents behind `agent-local` are the
next one ([ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md), migration step 12), and
adding it makes the compiler list every `match` that must handle it. In `AGENTS_FILE` an entry may
say `transport: a2a`, which is also the default when the key is absent, so existing files are
unchanged; any other value is a startup error. The chat API's `AgentInfo.cardUrl` stays required for
now; step 12 makes it optional, since a local agent has no card URL. A release selection travels as
`AgentTarget.release` and is only accepted when the *live* card advertises the release-channels
extension (ADR 0008).

**Planned** (in the earlier design, not in the code): an `Origin` on every input (user, A2A, MCP,
webhook, timer), inputs for `Approval`, `CheckCompleted`, `ToolResult` and `TimerFired`, and the
commands `CallTool`, `Reply`, `Notify` and `Schedule`. They arrive with the MCP, webhook and timer
steps ([MVP](mvp.md)); the closed enums make the compiler list every `match` that must handle them
(ADR 0004). The AG-UI design adds `ui_surface` and `ui_action` events ([ADR 0013](decisions/0013-a2ui-generative-ui.md)).

## Process roles

**Built** (checked against `bin/orchestrator/src/boot.rs` on 2026-09-29). One binary, one image. What a
process runs is its **role**, `ORCH_ROLE` or `--role` ([ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md)).
The enum, `Role`, and the supervisor, `Host`, come from the `adam-host` crate of adam-rs (a git
dependency pinned to a commit sha); this repository adds no role and no supervisor of its own.

| Role | Runs | Serves on `LISTEN_ADDR` |
|---|---|---|
| `control-plane` | migrations, the HTTP server: the resource API, the surfaces in `ORCH_SURFACES`, health. No dispatcher | the full API |
| `worker` | migrations, the dispatcher (including the transitions for agent updates) | health and metrics only (`/healthz`, `/readyz`, `/metrics`), so probes and scrapes work; everything else is 404 |
| `all` (default) | both, as before the role existed | the full API |

The halves are already decoupled: the only things they share are the outbox, the thread version
compare-and-swap and `LISTEN/NOTIFY`, all in Postgres, so there is no new protocol between them and
`transition` stays the one pure function both call. A thread created through a control plane is
`queued` until a worker claims its outbox row; a control plane never calls an agent. Wake-ups
between processes are the ones the replicas already use (Postgres `LISTEN/NOTIFY`, backed up by the
dispatcher's and the streams' own polling), so a lost notification costs latency, not correctness.

```mermaid
sequenceDiagram
  participant S as Signal (SIGTERM)
  participant B as boot::run
  participant H as adam_host::Host
  participant C as HTTP server (control plane)
  participant W as Dispatcher (worker)
  participant P as Health router (worker role only)
  B->>H: register every component, Host starts those the role asks for
  S-->>B: shutdown
  B->>B: /healthz and /readyz answer 503
  B->>H: shutdown resolved
  H->>C: cancel, drain (SHUTDOWN_GRACE_SECS)
  C-->>H: drained, or aborted after the grace
  H->>W: cancel, stop workers, release leases (SHUTDOWN_GRACE_SECS)
  W-->>P: as it ends, stop the probe router
  W-->>H: stopped
  H-->>B: Ok, or the first failure by component name (exit 70)
  B->>B: close the pool, exit
```

```mermaid
stateDiagram-v2
  [*] --> Starting: configuration valid, role known
  Starting --> Running: migrated, listener bound, components of the role started
  Starting --> [*]: database down (69), address taken (71), bad configuration (78)
  Running --> DrainingControlPlane: shutdown signal, or a component ended on its own
  DrainingControlPlane --> StoppingWorkers: server drained, or the grace is over
  StoppingWorkers --> [*]: dispatcher stopped (exit 0 after a signal, 70 after a failure)
```

A worker is ready when the store answers and its dispatcher has started; the other roles are ready
once the database is migrated and answers. The probe router of a worker is a worker component that
ends after the dispatcher, so a draining worker answers 503 rather than refusing connections.

### Observability and scaling

**Built** (checked against `orchestrator/bin/orchestrator/src/logging.rs` and
`orchestrator/crates/api/src/metrics.rs` on 2026-09-29).

**Logs.** Every log line carries the process's `role` and `instance` (the id that owns its outbox
leases): the first two keys of the JSON object, or a `role=worker instance=w1 ` prefix in text
format. The event formatter adds them, not a root span, because the dispatcher runs each outbox
row in a spawned task that would not inherit one. Lines written while a row is processed also sit
in an `outbox` span (`id`, `thread`, `kind`, `attempt`). The configuration is read before logging
starts, so a line about an invalid configuration has no role yet.

**Metrics.** Every role serves `GET /metrics` on `LISTEN_ADDR`, without an identity (it is part of
the health routes). It reads the outbox from Postgres on each scrape (one aggregate over the open
rows, `ThreadStore::outbox_stats`), so the numbers are **global**: every replica reports the same
queue, whatever it runs itself.

| Sample | Meaning |
|---|---|
| `orch_outbox_rows{state="due"}` | claimable now: `pending` and due, or `inflight` with a lapsed lease |
| `orch_outbox_rows{state="waiting"}` | `pending` in retry backoff |
| `orch_outbox_rows{state="leased"}` | `inflight` under a live lease: a worker is on it |
| `orch_outbox_oldest_due_age_seconds` | whole seconds since the oldest due row became due, `0` when none |

The queue that matters for scaling is `due + leased`: rows waiting for a worker plus rows workers
are busy with. Because the values are global, an aggregation across the replicas that report them
takes `max`, never `sum` (three replicas would triple the count).

```mermaid
sequenceDiagram
  participant K as KEDA (scaler)
  participant P as Prometheus
  participant C as Control plane /metrics
  participant D as Postgres
  participant W as Worker Deployment
  loop every scrape interval
    P->>C: GET /metrics
    C->>D: outbox_stats(now): count due, waiting, leased
    D-->>C: counts, oldest due time
    C-->>P: orch_outbox_rows{state}, orch_outbox_oldest_due_age_seconds
  end
  loop every polling interval
    K->>P: query max(due) + max(leased)
    P-->>K: rows in flight or waiting
    K->>W: replicas = ceil(rows / DISPATCHER_CONCURRENCY), 0 when none
  end
```

```mermaid
stateDiagram-v2
  [*] --> Waiting: pending, next_attempt_at in the future (retry backoff)
  [*] --> Due: pending, next_attempt_at reached
  Waiting --> Due: backoff over
  Due --> Leased: a worker claims it
  Leased --> Leased: heartbeat renews the lease
  Leased --> Waiting: delivery failed, retry_outbox
  Leased --> Due: lease lapsed (worker died), or released on shutdown
  Leased --> [*]: complete_outbox (delivered, dead, skipped)
```

*Unverified* (KEDA and Prometheus behaviour from memory of their documentation, not checked
against a running cluster): a `prometheus` trigger with the query
`max(orch_outbox_rows{state="due"}) + max(orch_outbox_rows{state="leased"})` and a threshold equal to
`DISPATCHER_CONCURRENCY` (default 32) gives one worker per 32 open rows. Scaling a worker
Deployment **to zero** needs a control plane that stays up and is scraped, since a worker that does
not run cannot report the rows waiting for it. Without Prometheus, KEDA's `postgresql` scaler can
run the same count directly: `SELECT count(*) FROM outbox WHERE (status = 'pending' AND
next_attempt_at <= now()) OR (status = 'inflight')` (every `inflight` row is either leased or due,
so both are counted). The edge proxy of the compose stack routes only the application and does not
expose `/metrics`; scrape the pods, not the public ingress.

The `split` profile of the compose stack ([`dev/README.md`](../dev/README.md#the-split-profile-a-control-plane-and-two-workers))
runs a control plane and two workers, and `dev/split-e2e.sh` kills the worker that holds a task.

**Trace context** is not carried yet. A W3C `traceparent` would have to cross the outbox, which
means a column (a migration), and an OpenTelemetry exporter; see the open question in
[open-questions.md](open-questions.md).

**Testing.** `ThreadStore::outbox_stats` has a conformance case (`outbox_stats`, run by the memory
and Postgres stores: empty, due, backoff, live and lapsed leases, completed rows); the exposition
text is checked against a golden; `/metrics` is checked without identity on the full and the
health-only router; the binary's smoke tests read it from a control plane (one due row before a
worker exists, none after) and from a worker, and parse every JSON log line of a worker for its
`role` and `instance`.

## Design choices

- **Closed enums + `match`, not a `dyn Adapter` registry. Built.** The set of
  channels is compiled in; nothing is loaded at runtime. Adding a channel adds
  a variant, and the compiler points at every `match` that must handle it —
  the exhaustiveness is what we want as the list grows. Every `match` over
  `ThreadState`, `Input`, `AgentUpdate` and `AgentTaskState` in `orch-core` has no wildcard arm.
  The ports use `impl Future` methods and static dispatch (`Ports` with associated types), so
  there is no vtable and no `async-trait` boxing on the hot path.
- **Authentication at the edge, authorization in the core.** The edge half is **built**: `orch-api`
  reads `X-Auth-Request-Email` (set by oauth2-proxy), answers 401 without it on every path but
  `/healthz` and `/readyz`, and a surface cannot forget it because `router_with_surfaces` wraps every
  route. The core half is **partly built**: `App` scopes every read and write to the owner (someone
  else's thread is a 404, never a 403), but `transition` does not yet decide by origin, because
  only a signed-in user and the delegated agent can send inputs. *Planned:* a webhook must not be
  able to approve a PR.
- **Request/response protocols return immediately. Built.** `postMessage` answers 202 with the
  `user_message` event and the work continues in the dispatcher; `createThread` answers 201. A2A
  has this built in (`SendStreamingMessage` streams the task; `SubscribeToTask` and `GetTask`
  resume it). For MCP, *planned*: `start_job` returns a job id at once.
- **Optional protocol extensions are capability-detected. Built.** The A2A adapter reads each agent
  card live on every call, never caches it, and offers release selection only when the card declares
  the release-channels extension with well-formed parameters; a selected release is refused, never
  run as the default, when the live card no longer offers it. A selection is sent as the
  `A2A-Extensions` header plus namespaced message metadata (ADR 0008).
- **Idempotency. Built, and different from the design.** Redeliveries are absorbed by the
  per-thread `events.idempotency_key` (unique index) and, towards the agent, by the A2A `messageId`,
  which is the outbox row id. The inbox table with `UNIQUE (source, idempotency_key)` is *planned*
  with webhooks.
- **Optimistic concurrency on threads. Built.** `threads.version`; `ThreadStore::commit` takes the
  expected version, and `App::apply` re-reads and retries a lost race up to `max_commit_attempts`
  (8), then answers 503 (`Conflict`).
- **At-least-once outbound. Built.** A delegation is retried until it is delivered or dead-lettered;
  the message id makes a repeat harmless where the agent records it.
- **One error model. Built.** Every error enum implements `orch_core::Classify`; retry, HTTP status
  and exit code decide by `ErrorClass`, never by variant ([`orchestrator/README.md`](../orchestrator/README.md#errors)).

## Data model

**Built.** One migration, `crates/store-postgres/migrations/0001_init.sql`, applied at boot by every
replica (sqlx takes an advisory lock; an applied migration is never edited).

```mermaid
erDiagram
  threads ||--o{ events : "log, PK thread_id + seq"
  threads ||--|| a2a_bindings : "A2A context and task"
  threads ||--o{ outbox : "delegate, cancel"
  threads {
    uuid id PK
    text owner
    text title
    text agent_id
    text release
    text state "queued working blocked done failed cancelled"
    bigint version "optimistic concurrency"
    bigint last_seq "per-thread counter"
  }
  events {
    uuid thread_id PK
    bigint seq PK
    text kind "user_message agent_message agent_status artifact thread_state error"
    jsonb actor
    jsonb data
    text idempotency_key "unique per thread when set"
  }
  a2a_bindings {
    uuid thread_id PK
    text context_id "the thread id"
    text task_id
    text task_state
    text revision
  }
  outbox {
    uuid id PK "also the A2A messageId"
    bigserial ord "global insertion order"
    uuid thread_id
    text kind "delegate cancel"
    text status "pending inflight delivered dead skipped"
    int attempts
    timestamptz next_attempt_at
    text lease_owner
    timestamptz lease_until
    timestamptz sent_at
  }
```

| Table | Holds | Key points |
|---|---|---|
| `threads` | Owner, title, target agent and release, current state, `version`, `last_seq` | The snapshot; the state is also implied by the log. `last_seq` is the per-thread counter row: it is bumped in the transaction that inserts the events, under the row lock, so `seq` has no gaps and no duplicates |
| `events` | The append-only event log | **This is the chat.** Primary key `(thread_id, seq)`; cascade-deleted with the thread |
| `a2a_bindings` | The A2A context id (the thread id), the current task id and state, the serving revision | Written with the commit that causes it, or by `mark_sent` |
| `outbox` | Commands to dispatch (`delegate`, `cancel`) | Status, attempts, `next_attempt_at`, lease owner and expiry, `sent_at`; two partial indexes over the open rows |

**Planned** tables of the design: `inbox` (received, authenticated events, for webhooks and MCP),
`timers` (scheduled events) and a job-level snapshot for multi-step jobs. The chat API needs none of
them.

**Live updates.** Postgres `LISTEN/NOTIFY` carries hints, never data. `PgStore` sends
`pg_notify` inside the writing transaction (channels `orch_thread` with the thread id as payload,
and `orch_outbox`), and every orchestrator replica holds one `LISTEN` connection (`PgWakeup`) that
fans the hints out to its own subscribers: its dispatcher (claim outbox rows) and its open SSE
streams (`App::event_stream`). After a reconnect of the listener, or when a subscriber lags, every
subscriber receives `Topic::Resync` and re-reads the store. A stream also polls every 5 s and the
dispatcher every 2 s, so a lost notification costs latency, not correctness. The stream is served
by the orchestrator: the web has no server-side code and never touches Postgres. No separate broker.

## Testing

**Built.**

- **Replay and determinism:** because `transition` is pure, folding the same inputs twice gives the
  same state and the same commands (`replay_is_deterministic`).
- **Transition table and properties:** one test per row of the table above; `proptest` over random
  input sequences checks that terminal states absorb, that `thread_state` events mark entry and
  that every open state can reach `done`.
- **Wire shapes:** `orch-core`'s JSON must match the schemas of [`api/chat-api.yaml`](api/chat-api.yaml);
  `orch-surface-chat-api` drives every operation and validates each response against them.
- **Conformance testkit per port:** `thread_store_conformance!` and `wakeup_conformance!` run
  against the in-memory implementations and against Postgres, so "does my store behave" is a test,
  not a reading exercise (ADR 0009, rule 3). `agent_client_conformance!` does the same for
  `AgentClient`: twelve cases (a live card and a transient failure for an unreachable one; the first
  envelope names the task; unique idempotency keys; `get_task` agrees with the stream; a follow-up
  continues an `input-required` task; `resubscribe` yields the rest under the same keys, or says
  `Unsupported`; a finished or unknown task is not found; a turn outlives its dropped stream; a
  running task is cancelled and a completed one is refused; a failed task carries its message;
  `find_task_by_message` never names a wrong task; an unreachable send has a public detail without an
  address). Each has a 10 s timeout, and an implementation supplies an `AgentFixture`. It runs
  against the scripted in-memory agent and against the A2A adapter over real HTTP, with an in-process
  A2A 1.0 agent behind it.
- **End to end, on both stores:** `orch-e2e` runs each scenario as `<name>::memory` and
  `<name>::postgres` (chat API, dispatcher, A2A adapter, a fake agent): restart mid-stream with no
  gap and no duplicate, several replicas on one database, SSE resume, blocked and follow-up, cancel,
  releases, agent auth. The binary is also tested as a process, including a SIGKILL of one of two
  replicas mid-task.
- **Golden transcripts:** [`api/examples`](api/examples/README.md) pin what the orchestrator emits;
  the web and its mock replay them, and `orch-agui-projection` projects them to AG-UI streams that
  `tools/agui-conformance` reads through the reference client (`@ag-ui/client` 1.0.0) in CI.
- **AG-UI:** every emitted event is validated against the vendored AG-UI 1.0 JSON Schema; property
  tests check that streams are well formed at every prefix and that resuming from any resume point
  yields exactly the remaining suffix.

**Planned:** contract tests per further protocol against recorded fixtures.

## Libraries

Versions are from `orchestrator/Cargo.toml` (*verified* 2026-09-29, from the repository).

| Need | Choice | Status |
|---|---|---|
| A2A client | [`a2aproject/a2a-rs`](https://github.com/a2aproject/a2a-rs): `a2a-lf` 0.3.1, `a2a-client-lf` 0.2.5 (`a2a-server-lf` 0.4.4 in the test fake agent only) | In use since MVP step 2. Protocol notes in `orch-agent-a2a` are *verified* against the SDK sources on 2026-09-29 by that crate's author; not re-checked for this page. Maturity: open question 4 |
| A2A server | Not chosen yet | **Planned** (`orch-surface-a2a`). The SDK's server crate, `a2a-server-lf`, is used today only by the test fake agent |
| Postgres | `sqlx` 0.9 with rustls; `jiff-sqlx` | In use |
| HTTP | `axum` 0.8, `tower-http` | In use |
| Configuration | `clap` 4 with environment fallback | In use (`bin/orchestrator`) |
| Errors | `thiserror` in library crates, `anyhow` in the binary | House rule, in use |
| Time | `jiff`; no `f64` durations | In use |
| AG-UI schema validation | `jsonschema` 0.58, test-only, against the vendored 1.0 schema | In use (tests) |
| Model endpoint | Any OpenAI-compatible endpoint (ADR 0005) | **Planned**: nothing in the workspace calls a model yet |
| Durable execution engine | none — Postgres state machine | Restate considered; BSL server + extra stateful system. Revisit only if waits/timers get complex. |
