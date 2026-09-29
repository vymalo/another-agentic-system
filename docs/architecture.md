# Architecture

> Status: **MVP steps 1–2 built** (one thread, one A2A agent, durable delegation); the rest is
> design. [As built](#as-built) is what the code does today, with diagrams; the sections after
> it, [How a job flows](#how-a-job-flows) and [Job lifecycle](#job-lifecycle), are the target
> design. Facts about third-party components are marked **verified** (checked against source,
> docs or a live system on 2026-09-28) or **unverified**; statements about this repository's
> code were checked against it on 2026-09-29.

## Goal

You start a job from a chat — or another system starts one over A2A, MCP or a
webhook. Agents plan it, split it, work in parallel, verify against real
checks, review, and hand back a pull request plus a summary. You read the chat
surface.

This system is the **orchestration layer**. It does not host agents: it drives
any agent that speaks A2A, uses tools over MCP, reacts to webhooks, and can
itself be driven the same way. Its processes are stateless; the only durable
state is the job ledger and event log (the chat) in Postgres.

## Principles

1. **Protocols only.** Everything this system depends on is a protocol: A2A,
   MCP, webhooks, an OpenAI-compatible model endpoint. No agent host, gateway
   product or SDK is a hard dependency.
   ([ADR 0007](decisions/0007-protocol-only-dependencies.md))
2. **Stateless processes, one event log.** Jobs outlive processes, so the job
   ledger and event log live in Postgres; nothing else does.
   ([ADR 0001](decisions/0001-rust-state-machine-on-postgres.md))
3. **Verification over consensus.** Quality comes from tests/CI, with bounded
   rework loops — not from agents agreeing.
   ([ADR 0002](decisions/0002-verification-over-consensus.md))
4. **git is the artifact.** Every step that changes code ends as a pushed
   branch; the result is a PR.
   ([ADR 0003](decisions/0003-git-as-durable-state-ephemeral-workers.md))
5. **The orchestrator never sees a protocol.** Adapters translate to one
   canonical event/command model around a pure core.
   ([orchestrator](orchestrator.md))

## Components

| Component | Role | Notes |
|---|---|---|
| **Orchestrator** (Rust, this repo) | Durable thread state machine; decides what happens next | Stateless replicas over Postgres, run as a **control plane** (API, surfaces) and **workers** (dispatcher, in-process agents), or both in one process. ([ADR 0001](decisions/0001-rust-state-machine-on-postgres.md), [ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md)) |
| **Postgres (CNPG)** | Threads, the event log (= the chat), the A2A binding, the outbox | Also the work queue (`SKIP LOCKED`) and the wake-up bus (`LISTEN/NOTIFY`). The inbox and timers are planned. |
| **Web chat surface** (Next.js + assistant-ui, this repo) | Chat surface and thread list | Renders the event log the orchestrator serves. Built: it follows the chat API's SSE stream ([ADR 0006](decisions/0006-assistant-ui-external-store.md)). Planned: AG-UI 1.0 ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md), binding in [`api/agui.md`](api/agui.md)); generative UI is A2UI ([ADR 0013](decisions/0013-a2ui-generative-ui.md)). It has no server-side code: the browser talks to the orchestrator through the edge. |
| **Agents** (external) | Planner, coding workers, reviewers, specialists | Anything reachable by an A2A agent-card URL. |
| **Tools** (external) | GitHub, docs, search, … | MCP servers. Planned. |
| **Model endpoint** (external) | The orchestrator's own model calls | Any OpenAI-compatible endpoint — EAIG / Agent Router, AISIX, … ([ADR 0005](decisions/0005-openai-compatible-model-endpoint.md)). Planned: nothing calls a model yet. |

### Agent hosts

This system does not care where an agent runs. Known hosts:

| Host | What it adds |
|---|---|
| **adam-coder** (default agent) | An A2A 1.0 agent that clones a repository, works on a branch, runs the checks, pushes and opens a pull request. It is the default because it is the first entry of `AGENTS_FILE`; the orchestrator has no code path of its own for it. Published as an image (about 2.9 GB, `linux/amd64`). ([ADR 0014](decisions/0014-adam-coder-default-agent-over-a2a.md), [The default agent](#the-default-agent)) |
| **another-agentic-platform** (first-class) | Versioned agent services, release channels, scale-to-zero runtimes, per-run worktrees, credential broker. Its coding harness is ADK-Rust driving `opencode acp`. When a target comes from the platform, this system offers **release selection** through the platform's A2A extension. ([ADR 0008](decisions/0008-platform-integration-via-a2a-extension.md)) |
| **kagent** | Declarative agents on Kubernetes, reached over A2A. |
| **Anything else** | Any A2A server. |

Considered and **not** chosen for this layer: **eve** (TypeScript durable
agents — an agent host, not an orchestration layer), **Restate** (BSL server,
extra stateful system — see ADR 0001), **OpenHands Agent Canvas** (what we ran
first — see [lessons](lessons-from-agent-canvas.md)).

## As built

What runs today (MVP steps 1–2, plus the AG-UI wire types and projection), read from the code on
2026-09-29. Solid boxes exist; dashed boxes are planned. The design that goes beyond it is
[further down](#how-a-job-flows); the crate map is in [Orchestrator: crate layout](orchestrator.md#crate-layout).

### Components

```mermaid
flowchart LR
  browser(("Browser"))
  subgraph EDGE["Edge: one origin"]
    edge["oauth2-proxy in production<br/>Caddy stand-in in compose: authenticates nobody<br/>sets X-Auth-Request-Email"]
  end
  web["<b>web</b>: Next.js + assistant-ui<br/>serves the UI only<br/>no API routes, no server-side calls"]
  subgraph REPLICA["Orchestrator process: stateless, any number, one binary (ORCH_ROLE: all, control-plane, worker)"]
    direction TB
    api["<b>orch-api</b><br/>identity layer, resource API, health"]
    surfaces["surfaces mounted by ORCH_SURFACES<br/>agui (run, connect, capabilities), chat-api: built, the default<br/>a2a: planned"]
    app["<b>orch-app</b><br/>App: transition + commit loop, event streams"]
    disp["<b>Dispatcher</b><br/>claims outbox rows, delegates, applies replies"]
    adapters["adapters chosen in bin/orchestrator<br/>PgStore, PgWakeup, A2aAgentClient"]
    api --> surfaces --> app
    disp --> app
    app --> adapters
    disp --> adapters
  end
  subgraph PG["Postgres: the only state"]
    tables[("threads · events (the chat)<br/>a2a_bindings · outbox")]
    notify(["LISTEN / NOTIFY<br/>orch_thread · orch_outbox<br/>hints, never data"])
  end
  subgraph AGENTS["A2A agents: any host"]
    card["agent card, read live<br/>release-channels extension, optional"]
    a2a["A2A 1.0 JSON-RPC + SSE<br/>bearer token from AGENTS_FILE tokenEnv"]
  end
  browser -- "GET / : the UI" --> edge
  browser -- "/api/* incl. SSE" --> edge
  edge -- "everything else" --> web
  edge -- "/api/*, /healthz, /readyz" --> api
  adapters -- "sqlx: one txn per commit" --> tables
  tables --> notify
  notify -. "wake every replica" .-> adapters
  adapters -- "SendStreamingMessage · SubscribeToTask<br/>GetTask · CancelTask" --> a2a
  adapters -- "read card: agent list, release check" --> card
  more["replicas 2…N: the same process"] --- tables
  mcp["MCP tools · model endpoint<br/>webhooks · timers · A2A / MCP callers"]:::planned
  mcp -.-> api
  classDef planned stroke-dasharray: 5 5,fill:none
  style surfaces stroke-dasharray: 5 5
```

- **The web never talks to the orchestrator server to server.** It serves the UI; the browser calls
  `/api/*` on its own origin, and the edge routes that path to the orchestrator and everything else
  to the web. There are no Next.js API routes, no server-side fetches and no secrets in the web
  (`web/README.md`). SSE goes browser → edge → orchestrator, unbuffered.
- **The edge owns identity.** The orchestrator trusts `X-Auth-Request-Email` and answers 401
  without it (fail closed), so it must only run behind a proxy that strips client-supplied copies.
  Locally, `edge` is Caddy and replaces the header with `dev@example.com`
  ([`dev/README.md`](../dev/README.md)).
- **A process runs the halves its role asks for** (`ORCH_ROLE`, ADR 0015): the HTTP server
  (`orch-api` plus the mounted surfaces) as the **control plane**, the dispatcher as a **worker**, or
  both (`all`, the default). A worker serves only `/healthz` and `/readyz`. The two halves meet only in
  Postgres, so control planes and workers scale apart. A process that dies loses nothing: its
  outbox leases lapse and another replica resumes the agent's task. Any replica can serve any thread's stream, because streams read the log.
- **Agents are A2A agent-card URLs from a static `AGENTS_FILE`**, read once at boot; the cards
  themselves are read live and never cached. A release selection is offered only when the live card
  advertises the release-channels extension (ADR 0008).

### A chat turn

One user message, from the browser to the agent and back, over the **legacy chat API**. The web no
longer takes this path: it sends `POST /agui/agents/{agentId}` and follows
`GET /agui/threads/{id}/connect` (below); the sequence from the orchestrator inward is the same.
The dispatcher may run on a different replica than the one that took the request.

```mermaid
sequenceDiagram
  autonumber
  actor B as Browser (web UI)
  participant E as Edge proxy
  participant A as Replica A<br/>orch-api + chat-api + App
  participant DB as Postgres
  participant D as Dispatcher<br/>(any replica)
  participant G as A2A agent
  alt first message
    B->>E: POST /api/threads {target, text}
  else follow-up or answer to a question
    B->>E: POST /api/threads/{id}/messages {text}
  end
  E->>A: the request plus X-Auth-Request-Email
  A->>A: validate (with a release: read the live agent card, fail closed)
  A->>A: transition(state, UserMessage) → (state, [Append user_message, Delegate])
  A->>DB: ONE txn: (thread and A2A binding, first message only), state + version, event user_message, outbox row (pending), NOTIFY
  A-->>B: 201 Thread (first message) or 202 user_message event
  B->>E: GET /api/threads/{id}/stream (EventSource)
  E->>A: same, unbuffered
  A->>DB: events after Last-Event-ID (none: from seq 1), then wait for NOTIFY, poll every 5 s
  A-->>B: SSE: id: seq, event: user_message
  DB-->>D: NOTIFY orch_outbox (or the 2 s poll)
  D->>DB: claim_outbox: SKIP LOCKED, 30 s lease, status inflight
  D->>G: read the agent card, then SendStreamingMessage<br/>messageId = outbox row id, contextId = thread id, bearer token
  G-->>D: SSE frames: task submitted, working, artifact, completed
  D->>DB: first frame: mark_sent (sent_at, task id)
  loop each frame the agent sends
    D->>DB: App.apply(Input::Agent, idempotency key): read the thread, transition, commit at the expected version (events, thread_state, NOTIFY)
    DB-->>A: NOTIFY orch_thread (every replica with an open stream)
    A->>DB: events after the stream's cursor
    A-->>B: SSE: id: seq, event: agent_status, artifact or thread_state
  end
  D->>DB: the turn ended: outbox row delivered
  Note over B,A: the connection drops, or replica A is killed
  B->>E: GET /api/threads/{id}/stream, Last-Event-ID: n
  E->>A: served by any replica
  A->>DB: events after n
  A-->>B: seq n+1 …, then live (the client dedupes by seq)
```

Prose for what the diagram compresses:

- **One transaction is the whole decision** (the `ONE txn` message). `App::apply` reads the thread, runs the pure
  `transition`, and asks the store to commit the new state, the events and the outbox rows at the
  version it read. A lost race is a `VersionConflict`; `apply` re-reads and retries up to 8 times,
  then answers 503. A replayed input carries an idempotency key and is a no-op (`Duplicate`).
- **A crash cannot lose or repeat the delegation.** The outbox row exists exactly when the message
  is in the log. If the dispatcher's replica dies, the lease (30 s by default) expires and another
  replica re-claims the row; because `sent_at` is set it resumes the agent's task
  (`SubscribeToTask`, else polling `GetTask`) instead of sending again, and the agent's own ids
  make each stored update idempotent.
- **An `input-required` turn ends the delegation, not the thread.** The thread becomes `blocked`;
  the user's answer is a new `user_message`, which re-queues it and sends a new outbox row that
  continues the *same* A2A task. A2A method names are those of A2A 1.0 (`SendStreamingMessage`;
  the 0.3 spelling is `message/stream`); recorded in `orch-agent-a2a`, *verified* against the SDK
  sources 2026-09-29 by that crate's author, not re-checked for this page.
- **The stream is the log.** `App::event_stream` replays every event with `seq >` the cursor, then
  follows; a clamped cursor, a poll under the `NOTIFY`, and a subscribe-before-read order mean no
  gap and no duplicate. When the process shuts down and the stream has caught up, it ends, so the
  client reconnects to another replica with `Last-Event-ID`. The web falls back to replaying from
  seq 1 and deduplicating when the browser gives up on its own retry.

### Thread state

The state of a thread is `orch-core`'s `ThreadState`; `transition(&state, &input)` is the only
place it changes. It is a subset of the [target job lifecycle](#job-lifecycle) below.

```mermaid
stateDiagram-v2
  [*] --> Queued: thread created with its first user message
  state Open {
    Queued --> Working: the agent reports working
    Queued --> Blocked: input or auth required; retryable delivery failure
    Working --> Blocked: input or auth required; retryable delivery failure
    Blocked --> Queued: a user message (the answer, or a follow-up)
    Blocked --> Working: the agent resumes the same task
  }
  Open --> Done: the agent completed
  Open --> Failed: the agent failed or rejected; permanent delivery failure
  Open --> Cancelled: the agent cancelled; cancelled before the agent started
  Done --> [*]
  Failed --> [*]
  Cancelled --> [*]
```

`Open` is `queued`, `working` and `blocked`; `done`, `failed` and `cancelled` absorb. Inside
`Open`, a user message in `queued` or `working` keeps the state and sends another delegation, a
cancel keeps the state and asks the agent to cancel (the outcome arrives as the agent's `canceled`),
and artifacts and agent messages keep the state. On a finished thread a user message is refused
(409, start a new thread) and a late agent update is dropped. A `thread_state` event is appended
only when a thread *enters* `blocked`, `done`, `failed` or `cancelled`. The full table, row by row,
is in [Orchestrator: thread state and transitions](orchestrator.md#thread-state-and-transitions).

### The default agent

The default agent is the first entry of `AGENTS_FILE`, and in the dev stack that is
[adam-coder](https://github.com/vymalo/another-adam-rs) ([ADR 0014](decisions/0014-adam-coder-default-agent-over-a2a.md)).
`GET /api/agents` keeps the file's order, the chat UI preselects the first agent, and if its card
cannot be read it stays first, listed without live details, instead of giving way to the next agent.
The orchestrator reads the coder like any other agent (a card URL and a bearer token from `tokenEnv`)
and sends the chat text unchanged, so the first message has to name the repository and the base branch.

```mermaid
sequenceDiagram
  participant D as Orchestrator dispatcher
  participant C as adam-coder
  participant G as git remote
  participant H as GitHub API
  D->>C: GET /.well-known/agent-card.json (no token)
  C-->>D: card (streaming, JSON-RPC URL = PUBLIC_URL)
  D->>C: SendStreamingMessage, bearer token, text = repository + base branch
  C-->>D: task submitted, then status working
  C->>G: clone, work on a branch, run the checks, push
  C-->>D: artifact branch (JSON data part)
  C->>H: open the pull request
  C-->>D: artifact pull_request (JSON data part)
  C-->>D: status completed
  Note over D,C: A dropped stream resumes with SubscribeToTask, or GetTask when the task is unknown. ListTasks is unsupported.
```

A2A task states become thread states through the pure transition function, exactly as for every
other agent; the coder adds no state of its own. `submitted` changes nothing, and `rejected` is a
failure whose detail starts with `rejected:`.

```mermaid
stateDiagram-v2
  [*] --> Queued: thread created, the task is submitted
  Queued --> Working: task working
  Queued --> Blocked: task input-required or auth-required
  Working --> Blocked: task input-required or auth-required
  Blocked --> Working: task working again
  Blocked --> Queued: the user answers, a new message
  Queued --> Done: task completed
  Working --> Done: task completed
  Queued --> Failed: task failed or rejected
  Working --> Failed: task failed or rejected
  Queued --> Cancelled: task canceled
  Working --> Cancelled: task canceled
  Done --> [*]
  Failed --> [*]
  Cancelled --> [*]
```

The `branch` and `pull_request` artifacts arrive as JSON data parts, so the chat shows their JSON
(`data.text` of the artifact event) and not a link until the coder also sends a URL part. The dev
stack runs the published image against scripted mocks, and [`dev/coder-e2e.sh`](../dev/coder-e2e.sh)
turns one chat message into a pull request (how: [`dev/README.md`](../dev/README.md#the-default-agent)).

### AG-UI: planned against built

The user-facing protocol is decided to be **AG-UI 1.0**, with the event log as the only source of
truth ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md); the mapping tables, endpoints and
`vymalo.*` schemas are [`api/agui.md`](api/agui.md)). The chat turn above is what runs. The
projection is built as pure code, and the routes that serve it are built: the run route, the connect
stream (replay, cursor, following across runs) and the capabilities document. The web runs on them
([`web/README.md`](../web/README.md#the-chat-layer)).

```mermaid
flowchart LR
  subgraph INB["Inbound: RunAgentInput to a core input"]
    post["POST /agui/agents/{agentId}<br/>RunAgentInput"]
    parse["orch-agui-proto<br/>RunAgentInput::parse<br/>drops unknown members, rejects malformed"]
    tr["orch-agui-projection<br/>translate(input, ThreadView)"]
    apply["orch-app<br/>App::apply(Input)"]
    post --> parse --> tr --> apply
  end
  subgraph OUTB["Outbound: log event to AG-UI frames"]
    log[("events<br/>Postgres")]
    es["orch-app<br/>App::event_stream(user, thread, after)<br/>replay, then live"]
    pj["orch-agui-projection<br/>Projector::apply(event, Audience)<br/>resume_preamble()"]
    fr["Frame: AG-UI event + resume_id"]
    sse["orch-surface-agui<br/>SSE: data: frame, id: seq<br/>run route and connect stream"]
    cli["AG-UI client"]
    log --> es --> pj --> fr --> sse --> cli
  end
  apply --> log
  classDef planned stroke-dasharray: 5 5,fill:none
```

| | Built | Planned |
|---|---|---|
| Wire types | `orch-agui-proto`: all 31 AG-UI 1.0 events and `RunAgentInput` as closed enums, checked against the vendored official schema | |
| Log → frames | `orch-agui-projection`: `Projector` (audiences, runs, subagents, interrupts, `resume_preamble`); a function of the log, with no async and no I/O | |
| `RunAgentInput` → input | `orch-agui-projection::translate` (new message, `resume`, cancel, attach, refusals with their HTTP status) | |
| Conformance | Schema validation of every frame; well-formedness properties; resume-from-any-point property; goldens read through `@ag-ui/client` 1.0.0 by `tools/agui-conformance` in CI | |
| HTTP routes | `orch-surface-agui`: `POST /agui/agents/{agentId}` (a consumer-minted thread id, id reconciliation, `resume`, refusals as RFC 9457 problems before the stream); `GET /agui/threads/{id}/connect` (replay, `Last-Event-ID`, `?mode=run`, keepalive, follows across runs and replicas); `GET /agui/agents/{agentId}/capabilities`; `agui` as an `ORCH_SURFACES` value and a `surface-agui` feature | |
| Idempotent runs | A retried POST attaches instead of duplicating: the idempotency key `agui:<threadId>:msg:<messageId>` on the event log (there is no inbox table yet) | |
| The web | `@assistant-ui/react-ag-ui` (pinned, one patch) over a `ThreadAgent`: the connect stream with `Last-Event-ID`, runs by `POST /agui/agents/{agentId}`, interrupts by `resume`, Cancel by the resource API ([ADR 0012](decisions/0012-ag-ui-user-facing-protocol.md#the-web)) | |
| Generative UI | | A2UI ([ADR 0013](decisions/0013-a2ui-generative-ui.md)): `ui_surface` and `ui_action` events; `forwardedProps.a2uiAction` is ignored with a warning today |
| Deprecating the chat API's interaction routes | The crate is separate and mounted by flag | The `deprecated` markers in `chat-api.yaml` and the `Deprecation` header of ADR 0012 |

`ORCH_SURFACES` accepts `agui` and `chat-api` and defaults to both; the web no longer uses `chat-api`,
so the next slice makes `agui` the default. A name whose Cargo feature is not compiled in is a startup
error (exit 78).

## How a job flows

**Target design** (MVP steps 3–7; steps 1–2 exist, see [As built](#as-built)). The built system
delegates a whole thread to one agent; this is the multi-agent flow it grows into.

```mermaid
sequenceDiagram
  actor U as You
  participant CP as Web chat surface (Next.js + assistant-ui)
  participant O as Orchestrator (Rust, stateless)
  participant DB as Postgres (CNPG)
  participant P as Planner (any A2A agent)
  participant W as Workers (A2A agents, e.g. platform coder)
  participant V as Verifier (CI via webhook, or a verifier agent)
  participant R as Reviewers (A2A agents)
  U->>CP: start job "…" (target agent, optional release)
  CP->>O: POST /api/threads (through the edge)
  O->>DB: thread + first event + outbox row, one txn
  DB-->>O: NOTIFY → dispatcher claims it
  O->>P: A2A task: plan
  P-->>O: subtasks + acceptance criteria
  par each subtask
    O->>W: A2A task (repo, branch, criteria)
    W-->>O: branch pushed
  end
  O->>V: run checks on branches
  V-->>O: fail + findings
  O->>W: rework with findings
  V-->>O: green
  O->>R: review diff
  R-->>O: approve / change requests
  O->>DB: every step appended to the event log
  DB-->>O: NOTIFY (every replica)
  O-->>CP: SSE from the orchestrator → chat renders live
  O-->>U: PR + summary in chat
```

## Job lifecycle

**Target design.** The built states are the subset `queued`, `working`, `blocked`, `done`, `failed`
and `cancelled` ([Thread state](#thread-state)); planning, verifying and reviewing arrive with MVP
steps 3–5.

```mermaid
stateDiagram-v2
  [*] --> Queued
  Queued --> Planning
  Planning --> Working
  Working --> Verifying: branches pushed
  Verifying --> Working: checks fail, attempts < N
  Verifying --> Failed: attempt budget spent
  Verifying --> Reviewing: green
  Reviewing --> Working: change requests
  Reviewing --> AwaitingYou: approved → PR open
  Planning --> Blocked: needs your input
  Working --> Blocked: needs your input
  Blocked --> Planning: answered in chat
  AwaitingYou --> Done: you merge
  Queued --> Cancelled: cancel
  Planning --> Cancelled: cancel
  Working --> Cancelled: cancel
  Failed --> [*]
  Done --> [*]
  Cancelled --> [*]
```

The two backward edges (checks fail → rework, change requests → rework) carry
concrete findings and are bounded by budgets (attempts, wall clock, tokens).
Exhausting a budget ends in `Failed` with the findings in the chat — never a
silent "done".

## Where it runs

- **netcup** (`kubectl --context admin@netcup`) is the natural home: CNPG runs
  there and `*.sls.servers.segning.pro` resolves to its Traefik.
- Deployed via ArgoCD from `WhyThatFunction/home-os` like everything else.
- The orchestrator is one image with a role per Deployment: `control-plane` pods for users and
  `worker` pods for agent work, or `all` in one pod for development and small installs
  ([ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md)).
- The system's own footprint is small: stateless orchestrator replicas, the
  Next.js web chat surface, and a Postgres database. Agents run on their hosts.
