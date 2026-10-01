# another-agentic-system

<img src="web/public/brand/panda.svg" alt="A boring giant panda in three colours, the mark of another-agentic-system" width="96" height="96" align="right">

A protocol-agnostic **orchestration layer** for multi-agent work. You start a
job from a chat — or another system starts one over A2A, MCP or a webhook.
Agents plan it, work it in parallel, verify it against real checks, review it,
and hand back a pull request. You read the chat surface.

> **Status: MVP steps 1–2 are built** — the orchestrator (`orchestrator/`) and
> the chat surface (`web/`). The owner judged this plumbing, not yet the system they want; what the
> system is meant to be is the [vision](docs/vision.md), and the [MVP](docs/mvp.md) is re-planned toward it.
> What exists, with diagrams: [Architecture: as built](docs/architecture.md#as-built).
> Decisions are recorded as ADRs; what is not yet verified is listed in
> [open questions](docs/open-questions.md).

It hosts no agents. It drives anything that speaks A2A, uses tools over MCP,
reacts to webhooks, and can be driven the same way. Its processes are
stateless; the only durable state is the job ledger and event log (the chat)
in Postgres.

## In one picture

```mermaid
flowchart LR
  you((You)) -- browser --> edge[Edge proxy<br/>oauth2-proxy]
  edge -- UI --> cp[Web chat surface<br/>Next.js + assistant-ui]
  edge -- "/api/*, /agui/*" --> orch[Orchestrator<br/>stateless Rust replicas]
  orch <--> db[(Postgres / CNPG<br/>chat · threads · outbox)]
  orch -- A2A --> agents[Agents — any A2A host<br/>another-agentic-platform · kagent · …]
  agents -- push branch --> git[(git → PR)]
  ext[Other systems · MCP clients · webhooks · timers]:::planned -. A2A / MCP / HTTP .-> orch
  orch -. MCP .-> tools[Tools: GitHub, docs, search…]:::planned
  orch -. OpenAI-compatible .-> gw[Model endpoint<br/>EAIG / Agent Router · AISIX · …]:::planned
  classDef planned stroke-dasharray: 5 5,fill:none
```

Solid is built, dashed is planned. The web serves the UI only; the browser talks to the
orchestrator through the edge. Component, request and state diagrams:
[Architecture](docs/architecture.md#as-built).

## Principles

1. **Protocols only** — no agent host, gateway product or SDK is a hard
   dependency. ([ADR 0007](docs/decisions/0007-protocol-only-dependencies.md))
2. **Stateless processes, one event log.**
   ([ADR 0001](docs/decisions/0001-rust-state-machine-on-postgres.md))
3. **Verification over consensus.**
   ([ADR 0002](docs/decisions/0002-verification-over-consensus.md))
4. **git is the artifact.**
   ([ADR 0003](docs/decisions/0003-git-as-durable-state-ephemeral-workers.md))
5. **The orchestrator never sees a protocol** — adapters around a pure core.
   ([orchestrator](docs/orchestrator.md))

## Documents

| Document | What it covers |
|---|---|
| [Architecture](docs/architecture.md) | Components, agent hosts, **as built** (component diagram, a chat turn over AG-UI, thread state, how AG-UI is served), the target job flow and lifecycle, where it runs |
| [Orchestrator](docs/orchestrator.md) | Ports & adapters, the crate dependency graph, event flow (design against built), outbox lifecycle, transition table, core types, data model, live updates (how AG-UI streams come from the log), testing |
| [Orchestrator workspace](orchestrator/README.md) | Running it, configuration, shutdown, error classes; each crate has its own README (role, API, environment, tests) |
| [MVP](docs/mvp.md) | Build order, smallest working loop first, with what is built |
| [API contract](docs/api/chat-api.yaml) | OpenAPI 3.1: the resource API (agents, threads, cancel, health), the AG-UI operations (the legacy REST interaction endpoints were removed on 2026-09-30) |
| [AG-UI binding](docs/api/agui.md) | How the orchestrator speaks AG-UI 1.0: run and connect endpoints, log-to-AG-UI mapping, `vymalo.*` schemas |
| [Webhooks](docs/api/webhooks.md) | CI results by webhook: the generic signed shape (built) and the GitHub adapter (planned): headers, HMAC, body, conclusions, response codes, a worked signature and known-answer vectors |
| [API index](docs/api/README.md) | What is in `docs/api/`, and the optional A2A extensions the orchestrator speaks: [`ui-catalog/v1`](docs/api/ui-catalog-v1.md) (the web's component catalog, sent to agents) and [`thread-tools/v1`](docs/api/thread-tools-v1.md) (a per-thread MCP endpoint for agents, with an HMAC token); contracts accepted 2026-10-01 and built (the catalog handshake, the thread tools with their token and `get_ui_catalog`), except what [`thread-tools/v1`](docs/api/thread-tools-v1.md) marks as later slices |
| [Open questions](docs/open-questions.md) | Open, closed, and moved to the platform |
| [Lessons from Agent Canvas](docs/lessons-from-agent-canvas.md) | What running OpenHands Agent Canvas taught us, as requirements |

### Decisions

| ADR | Decision |
|---|---|
| [0001](docs/decisions/0001-rust-state-machine-on-postgres.md) | Orchestrator is a Rust state machine on Postgres (not eve, not Restate) |
| [0002](docs/decisions/0002-verification-over-consensus.md) | Verification over consensus |
| [0003](docs/decisions/0003-git-as-durable-state-ephemeral-workers.md) | git is the durable artifact; workers are ephemeral |
| [0004](docs/decisions/0004-closed-enums-over-dyn-registry.md) | Protocols as closed enums, not a dynamic adapter registry |
| [0005](docs/decisions/0005-openai-compatible-model-endpoint.md) | Model access through any OpenAI-compatible endpoint |
| [0006](docs/decisions/0006-assistant-ui-external-store.md) | Chat surface: Next.js + assistant-ui (an external store at first, now the AG-UI runtime per 0012) |
| [0007](docs/decisions/0007-protocol-only-dependencies.md) | Protocol-only dependencies: an agnostic orchestration layer |
| [0008](docs/decisions/0008-platform-integration-via-a2a-extension.md) | Optional another-agentic-platform integration via an A2A extension |
| [0009](docs/decisions/0009-swappable-implementations-at-build-time.md) | Swappable implementations, selected at build time |
| [0011](docs/decisions/0011-web-shadcn-tailwind-feature-layout.md) | Chat surface: shadcn/ui on Tailwind v4, kebab-case feature layout |
| [0012](docs/decisions/0012-ag-ui-user-facing-protocol.md) | AG-UI 1.0 as the user-facing protocol; REST kept for resources; surfaces mounted by configuration |
| [0013](docs/decisions/0013-a2ui-generative-ui.md) | A2UI for generative UI, end to end over A2A and AG-UI |
| [0014](docs/decisions/0014-adam-coder-default-agent-over-a2a.md) | adam-coder is the default agent (first `AGENTS_FILE` entry), over plain A2A |
| [0015](docs/decisions/0015-control-plane-and-workers-on-adam-rs.md) | Control plane and workers on adam-rs (`Role` enum, git rev); in-process agents behind a feature; amends 0001 and 0007 |
| [0016](docs/decisions/0016-inbox-timers-and-job-ledger-on-the-thread.md) | Inbox, timers and the job ledger on the thread (`threads.job`); unsolicited machine input only. *Slices 2 and 5 built* |
| [0017](docs/decisions/0017-ci-results-by-webhook.md) | CI results by webhook: a GitHub adapter and a generic signed shape. *Generic route built (slice 6); GitHub adapter planned* |
| [0018](docs/decisions/0018-verification-gate-and-rework-loop.md) | Configurable verification gate (CI, agent checks, verifier agent) and a bounded rework loop; refines 0002. *Built for the agent's own checks (slices 2, 3) and the verifier agent (slice 10); CI built (slice 6)* |
| [0019](docs/decisions/0019-mcp-server-over-streamable-http.md) | MCP server over streamable HTTP, bearer tokens first, OIDC later; bypasses the inbox. *Tools, bearer tokens and `wait_for_job` built; OIDC planned* |
| [0020](docs/decisions/0020-a-thread-is-a-conversation.md) | A thread is a conversation: a message on a finished thread starts its next job (`Job.number`, `job_started`); amends 0012, 0016, 0018, 0019 |
| [0021](docs/decisions/0021-context-across-a2a-tasks.md) | Context across A2A tasks: same context, `referenceTaskIds` of the previous task on every new task of a thread, never for the verifier |
| [0022](docs/decisions/0022-platform-provisions-agents-system-discovers-them.md) | The platform provisions A2A agents and the system discovers them through an `AgentRegistry` port; the platform knows nothing about the UI. *Accepted 2026-10-01 (owner's delegation)* |
| [0023](docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md) | A UI component catalog on A2UI catalogs, as an optional A2A extension, sent at conversation start, with a refetch seam. *Accepted 2026-10-01 (owner's delegation)* |
| [0024](docs/decisions/0024-mcp-tools-attached-per-conversation.md) | MCP tools attached per conversation from the UI and relayed to agents by the orchestrator, which holds the credentials. *Accepted 2026-10-01 (owner's delegation)* |
| [0025](docs/decisions/0025-nested-steps-events-carry-their-source-path.md) | Nested steps: events carry their source path; a collapsible tree in the web. *Accepted 2026-10-01 (owner's delegation)* |
| [0026](docs/decisions/0026-agent-mentions-as-structured-references.md) | Agent mentions as structured references; the addressed agent coordinates the mentioned agents with `ask_agent`. *Accepted 2026-10-01 (owner's delegation)* |

## Local development

`compose.yaml` runs everything except the agents' real work: Postgres, two
[WireMock](https://wiremock.org/) stand-ins for an A2A 1.0 coding agent, and, with the `app`
profile, the real orchestrator and chat UI behind one origin, plus the default agent, adam-coder
([ADR 0014](docs/decisions/0014-adam-coder-default-agent-over-a2a.md)), on scripted mocks of its
model, GitHub and git remote, and a CI stand-in. Docker with Compose v2 is all it needs; the mocks need no agent host,
model or GitHub token. **Start with [Test it locally](dev/README.md#test-it-locally)** (prerequisites, URLs, what the chat shows, MCP,
going live, troubleshooting); the rest of [`dev/README.md`](dev/README.md) is the reference and the scenarios.

```sh
docker compose up -d --wait                          # postgres + mocks: nothing is built, seconds
docker compose --profile app up --build              # the whole system (first build takes minutes, the coder image is 2.9 GB); add -d --wait to return when healthy
open http://127.0.0.1:8080                           # the chat UI; the coder is preselected, "Mock coder" is one click away
dev/e2e-all.sh                                       # every scenario against the running stack, then a summary (curl, jq, git, openssl)
dev/greeting-e2e.sh                                  # or one of them: "hi" gets a greeting that says the coder's name, not a request for a task
dev/coder-e2e.sh                                     # or one of them: a chat message becomes a pull request, gated on the coder's checks and CI
dev/ci-e2e.sh                                        # a gated mock agent: a signed CI report sends it back, then ends the job
dev/try-thread.sh "add a health endpoint"            # or drive a mock thread from the terminal (curl, jq)
dev/mcp-e2e.sh                                       # or start a job as an MCP client would, with a bearer token (curl, jq)
docker compose --profile app down -v                 # stop and forget the database
```

The `split` profile adds two workers, so the orchestrator can run as a control plane beside them
([`dev/README.md`](dev/README.md#the-split-profile-a-control-plane-and-two-workers)):

```sh
ORCHESTRATOR_ROLE=control-plane docker compose --profile app --profile split up -d --build --wait
dev/split-e2e.sh                                     # kills the worker that holds a task; the other finishes it
```

| Profile | Services | Ports on 127.0.0.1 |
|---|---|---|
| default | `postgres`, `mock-agent`, `mock-agent-releases`, `mock-verifier` | 5432, 8081, 8082, 8083 |
| `app` | + `orchestrator`, `web`, `edge` | 8080 (`/api/*` to the orchestrator, the rest to the UI) |
| `split` | + `orchestrator-worker-1`, `orchestrator-worker-2` (dispatcher only; beside `app`, with `ORCHESTRATOR_ROLE=control-plane`) | none published |
| `app` | + `coder`, `coder-postgres`, `mock-openai`, `mock-github`, `git-server` (the default agent and its mocks), `mock-ci` (a CI stand-in: the coder is gated on its own checks and on CI and ends `done` when `mock-ci` has reported the pushed commit, [`dev/README.md`](dev/README.md#ci-the-gate-by-webhook)), `mock-mcp-search` (a mock web-search MCP server with canned results, [`dev/README.md`](dev/README.md#mock-web-search-mcp)). The coder reads its agent folder (name, card, instructions) from [`dev/coder/agent/`](dev/coder/agent/instructions.md), mounted at `/etc/adam/agent`: edit it and `docker compose --profile app up -d coder`, no rebuild ([Change what the coder says](dev/README.md#change-what-the-coder-says)) | 8090 (`coder`), 8091 (`mock-openai`), 8092 (`mock-github`), 8093 (`git-server`), 8096 (`mock-mcp-search`); `coder-postgres` is not published |
| `smee` | + `smee`, `smee-proxy` (opt-in: forwards GitHub webhooks from smee.io, a third party that sees them; needs `SMEE_URL`) | none published |
| `local-agent` | `orchestrator-local`, `local-postgres` (opt-in: the orchestrator built with `agent-local`, hosting an `echo` agent) | 8095 |

To point the coder at a real model and GitHub, copy [`.env.example`](.env.example) to `.env` and add the override:
`docker compose -f compose.yaml -f compose.live.yaml --profile app up --build` (Compose v2.24.4 or newer;
[`dev/README.md`](dev/README.md#going-live)).

The `edge` proxy replaces oauth2-proxy locally by injecting `X-Auth-Request-Email: dev@example.com`.
It authenticates nobody; it is for a laptop, never for production.

To run the code you are changing against the mocks (compose supplies only the infrastructure):

| Variable | Value | For |
|---|---|---|
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch` | the orchestrator |
| `AGENTS_FILE` | `dev/agents.local.yaml` (agents on `127.0.0.1:8081`/`8082`/`8083`) | the orchestrator |
| `MOCK_AGENT_TOKEN` | `dev-mock-token` (any non-empty value; the mocks only require a bearer) | the orchestrator, named by `tokenEnv` |
| `AUTH_DEV_USER` | `dev@example.com` | the orchestrator without the edge proxy |
| `ORCH_TEST_DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch_test` | `cargo test --workspace` |
| `ORCH_TEST_MOCK_AGENT_URL`, `ORCH_TEST_MOCK_AGENT_RELEASES_URL`, `ORCH_TEST_MOCK_VERIFIER_URL` | `http://127.0.0.1:8081`, `http://127.0.0.1:8082`, `http://127.0.0.1:8083` | `cargo test -p orch-e2e --test wiremock_agent` |

The mock agent picks its script from a word in your message:

| Say | The thread |
|---|---|
| anything else | works, opens a "pull request" artifact, ends `done` |
| `ask` | asks a question and ends `blocked`; answer it in the same thread |
| `fail` / `reject` / `error` | ends `failed` (agent failure / agent rejection / JSON-RPC error) |
| `slow` | like the default, over 8 seconds |

`mock-agent-releases` declares the release-channels extension, so only it shows the release
dropdown: channels `production`, `staging`, `latest` and three revisions. Ports can be moved with
`POSTGRES_PORT`, `MOCK_AGENT_PORT`, `MOCK_AGENT_RELEASES_PORT`, `MOCK_VERIFIER_PORT`, `EDGE_PORT`, `CODER_PORT`,
`MOCK_OPENAI_PORT`, `MOCK_GITHUB_PORT`, `GIT_SERVER_PORT` and `MOCK_MCP_SEARCH_PORT`. CI keeps the mocks
honest: [`compose.yml`](.github/workflows/compose.yml) starts them, runs
[`dev/check-mocks.sh`](dev/check-mocks.sh) (and [`dev/check-agent-mocks.sh`](dev/check-agent-mocks.sh) for the mock web search) and the real orchestrator client against them, and
[`coder-e2e.yml`](.github/workflows/coder-e2e.yml) runs the whole `app` profile, coder included, and
checks that the mocks and the agent folder vendored under `dev/coder` still equal upstream.

The coder is not reachable from an orchestrator running on the host (its card advertises
`http://coder:8080/`), so `dev/agents.local.yaml` leaves it out.

## Related

- **another-agentic-platform** — the agent platform: versioned agent services,
  release channels, runtimes, harnesses. This system consumes it over A2A like
  any other host, with an optional release picker.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Vendored agent skills keep their own
licenses; see [third-party-notices.md](third-party-notices.md).
