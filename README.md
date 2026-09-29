# another-agentic-system

A protocol-agnostic **orchestration layer** for multi-agent work. You start a
job from a chat — or another system starts one over A2A, MCP or a webhook.
Agents plan it, work it in parallel, verify it against real checks, review it,
and hand back a pull request. You read the chat surface.

> **Status: MVP steps 1–2 are built** — the orchestrator (`orchestrator/`) and
> the chat surface (`web/`); later steps are still design ([MVP](docs/mvp.md)).
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
  edge -- "/api/*" --> orch[Orchestrator<br/>stateless Rust replicas]
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
| [Architecture](docs/architecture.md) | Components, agent hosts, **as built** (component diagram, a chat turn, thread state, AG-UI planned against built), the target job flow and lifecycle, where it runs |
| [Orchestrator](docs/orchestrator.md) | Ports & adapters, the crate dependency graph, event flow (design against built), outbox lifecycle, transition table, core types, data model, testing |
| [Orchestrator workspace](orchestrator/README.md) | Running it, configuration, shutdown, error classes; each crate has its own README (role, API, environment, tests) |
| [MVP](docs/mvp.md) | Build order, smallest working loop first, with what is built |
| [Chat API contract](docs/api/chat-api.yaml) | OpenAPI 3.1: the resource API (agents, threads, cancel, health) and the deprecated REST interaction endpoints |
| [AG-UI binding](docs/api/agui.md) | How the orchestrator speaks AG-UI 1.0: run and connect endpoints, log-to-AG-UI mapping, `vymalo.*` schemas |
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
| [0006](docs/decisions/0006-assistant-ui-external-store.md) | Chat surface: Next.js + assistant-ui with an external store |
| [0007](docs/decisions/0007-protocol-only-dependencies.md) | Protocol-only dependencies: an agnostic orchestration layer |
| [0008](docs/decisions/0008-platform-integration-via-a2a-extension.md) | Optional another-agentic-platform integration via an A2A extension |
| [0009](docs/decisions/0009-swappable-implementations-at-build-time.md) | Swappable implementations, selected at build time |
| [0011](docs/decisions/0011-web-shadcn-tailwind-feature-layout.md) | Chat surface: shadcn/ui on Tailwind v4, kebab-case feature layout |
| [0012](docs/decisions/0012-ag-ui-user-facing-protocol.md) | AG-UI 1.0 as the user-facing protocol; REST kept for resources; surfaces mounted by configuration |
| [0013](docs/decisions/0013-a2ui-generative-ui.md) | A2UI for generative UI, end to end over A2A and AG-UI |
| [0014](docs/decisions/0014-adam-coder-default-agent-over-a2a.md) | adam-coder is the default agent (first `AGENTS_FILE` entry), over plain A2A |

## Local development

`compose.yaml` runs everything except the agents' real work: Postgres, two
[WireMock](https://wiremock.org/) stand-ins for an A2A 1.0 coding agent, and, with the `app`
profile, the real orchestrator and chat UI behind one origin. Docker with Compose v2 is all it needs;
the mocks need no agent host, model or GitHub token. Reference and scenarios:
[`dev/README.md`](dev/README.md).

```sh
docker compose up -d --wait                          # postgres + mocks: nothing is built, seconds
docker compose --profile app up -d --build --wait    # + orchestrator, web, edge proxy (first build takes minutes)
open http://127.0.0.1:8080                           # the chat UI; pick "Mock coder" and say something
dev/try-thread.sh "add a health endpoint"            # or drive a thread from the terminal (curl, jq)
docker compose --profile app down -v                 # stop and forget the database
```

| Profile | Services | Ports on 127.0.0.1 |
|---|---|---|
| default | `postgres`, `mock-agent`, `mock-agent-releases` | 5432, 8081, 8082 |
| `app` | + `orchestrator`, `web`, `edge` | 8080 (the only one: `/api/*` to the orchestrator, the rest to the UI) |

The `edge` proxy replaces oauth2-proxy locally by injecting `X-Auth-Request-Email: dev@example.com`.
It authenticates nobody; it is for a laptop, never for production.

To run the code you are changing against the mocks (compose supplies only the infrastructure):

| Variable | Value | For |
|---|---|---|
| `DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch` | the orchestrator |
| `AGENTS_FILE` | `dev/agents.local.yaml` (agents on `127.0.0.1:8081`/`8082`) | the orchestrator |
| `MOCK_AGENT_TOKEN` | `dev-mock-token` (any non-empty value; the mocks only require a bearer) | the orchestrator, named by `tokenEnv` |
| `AUTH_DEV_USER` | `dev@example.com` | the orchestrator without the edge proxy |
| `ORCH_TEST_DATABASE_URL` | `postgres://postgres:postgres@localhost:5432/orch_test` | `cargo test --workspace` |
| `ORCH_TEST_MOCK_AGENT_URL`, `ORCH_TEST_MOCK_AGENT_RELEASES_URL` | `http://127.0.0.1:8081`, `http://127.0.0.1:8082` | `cargo test -p orch-e2e --test wiremock_agent` |

The mock agent picks its script from a word in your message:

| Say | The thread |
|---|---|
| anything else | works, opens a "pull request" artifact, ends `done` |
| `ask` | asks a question and ends `blocked`; answer it in the same thread |
| `fail` / `reject` / `error` | ends `failed` (agent failure / agent rejection / JSON-RPC error) |
| `slow` | like the default, over 8 seconds |

`mock-agent-releases` declares the release-channels extension, so only it shows the release
dropdown: channels `production`, `staging`, `latest` and three revisions. Ports can be moved with
`POSTGRES_PORT`, `MOCK_AGENT_PORT`, `MOCK_AGENT_RELEASES_PORT` and `EDGE_PORT`. CI keeps the mocks
honest: [`compose.yml`](.github/workflows/compose.yml) starts them, runs
[`dev/check-mocks.sh`](dev/check-mocks.sh) and the real orchestrator client against them.

## Related

- **another-agentic-platform** — the agent platform: versioned agent services,
  release channels, runtimes, harnesses. This system consumes it over A2A like
  any other host, with an optional release picker.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Vendored agent skills keep their own
licenses; see [third-party-notices.md](third-party-notices.md).
