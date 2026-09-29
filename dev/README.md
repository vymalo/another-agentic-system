# Local development stack

What `compose.yaml` at the repository root starts, and how to steer the mock agents. The quick
start is in the root [README](../README.md#local-development); this page is the reference.

The mocks are [WireMock](https://wiremock.org/) 3.13.2 stubs of the one protocol the orchestrator
speaks to agents: A2A 1.0 over JSON-RPC and SSE. They stand in for a real coding agent so the whole
loop (chat, orchestrator, Postgres, A2A adapter) runs on a laptop without an agent host, a model or
a GitHub token. They hold no state and are not a substitute for the agent contract tests of a real
host.

## Services

| Service | Image | Host port (127.0.0.1) | Profile | What it is |
|---|---|---|---|---|
| `postgres` | `postgres:16.15-alpine` | `5432` (`POSTGRES_PORT`) | default | The orchestrator's database `orch`, and `orch_test` for `cargo test`. User and password are both `postgres`. Named volume `postgres-data`. |
| `mock-agent` | `wiremock/wiremock:3.13.2` | `8081` (`MOCK_AGENT_PORT`) | default | A fake A2A 1.0 coding agent. |
| `mock-agent-releases` | `wiremock/wiremock:3.13.2` | `8082` (`MOCK_AGENT_RELEASES_PORT`) | default | The same agent, declaring the [release-channels extension](https://github.com/vymalo/another-agentic-platform/blob/main/docs/extensions/release-channels-v1.md). |
| `orchestrator` | built from [`orchestrator/`](../orchestrator/Dockerfile) | not published | `app` | The real orchestrator, with [`dev/agents.yaml`](agents.yaml). |
| `web` | built from [`web/Dockerfile`](../web/Dockerfile) | not published | `app` | The real chat UI. |
| `edge` | `caddy:2.11.4-alpine` | `8080` (`EDGE_PORT`) | `app` | Stands in for oauth2-proxy: one origin for the UI and the API. |

The default profile builds nothing and starts in seconds. `--profile app` builds the two images
(the Rust build takes a few minutes the first time).

```mermaid
sequenceDiagram
  actor U as Browser or curl
  participant E as edge (Caddy)
  participant W as web
  participant O as orchestrator
  participant P as postgres
  participant M as mock-agent (WireMock)
  U->>E: GET / and /api/* on 127.0.0.1:8080
  E->>W: everything except /api/*
  E->>O: /api/* with X-Auth-Request-Email: dev@example.com (any client value replaced)
  O->>P: append the event, enqueue the delegation
  O->>M: GET /.well-known/agent-card.json
  M-->>O: card (streaming, bearer scheme, interface URL from the Host header)
  O->>M: POST /a2a SendStreamingMessage, Authorization: Bearer dev-mock-token
  M-->>O: SSE: task, statusUpdate, artifactUpdate, statusUpdate
  O->>P: append agent_status, artifact and thread_state events
  U->>E: GET /api/threads/{id}/stream
  E->>O: SSE (unbuffered)
  O-->>U: the events, live
```

### The edge is not oauth2-proxy

`edge` puts the chat UI and the chat API on one origin, as the production ingress does, and sets
`X-Auth-Request-Email: dev@example.com` on every API request, replacing whatever the client sent.
It authenticates nobody. It exists so the UI works locally without an identity provider; it must
never be exposed beyond `127.0.0.1` (the compose file binds it there) and never used in production,
where oauth2-proxy authenticates the user and the orchestrator trusts the header only because the
proxy owns it (see the identity notes in [`orchestrator/README.md`](../orchestrator/README.md)).

Without `--profile app`, talk to a host-run orchestrator with `AUTH_DEV_USER` or send the header
yourself, as [`try-thread.sh`](try-thread.sh) does.

## Mock agent scenarios

Every stub requires a bearer token (any non-empty value; `dev-mock-token` is what the compose
orchestrator sends) and answers `401 Unauthorized` with a plain-text body without one, like an auth
proxy in front of a real agent. The agent card is public. The scenario of a new task is chosen by a
**whole word in the message text**, case-insensitive; when several appear, the first row of the
table wins.

| Keyword in the text | `SendStreamingMessage` answers with | Thread ends |
|---|---|---|
| `error` | JSON-RPC error `-32602` (HTTP 200): a permanent rejection, no retry | `failed`, `error` event |
| `reject` | task `submitted`, then `rejected` with a message | `failed` |
| `fail` | `submitted`, `working`, `failed` with a message | `failed` |
| `ask` | `submitted`, `working`, `input-required` ("Which branch should I base the change on?") | `blocked` |
| `slow` | the default script, dribbled over 8 s in 16 chunks (frames split mid-line) | `done`, after 8 s |
| none | `submitted`, `working`, artifact "Pull request" with the URL `https://github.com/example/sandbox/pull/1`, `completed` | `done` |

A **follow-up** message on an existing task (the message carries a `taskId`, which is what the
orchestrator sends when you answer a `blocked` thread) gets `working`, a new artifact and
`completed`; with the keyword `ask` again it asks another question ("One more question: should I
add tests?"), so multi-turn threads can be tried.

Other methods, whatever the text:

| Method | Answer |
|---|---|
| `SendMessage` | One task in the final state: `completed` with the PR artifact; `fail` gives `failed`, `ask` gives `input-required`. |
| `GetTask` | The task `params.id`, `completed`, with the PR artifact. |
| `CancelTask` | The task `params.id`, `canceled`. |
| `ListTasks` | An empty page (the orchestrator then treats the agent as unable to find a task by message). |
| `SubscribeToTask` | `-32001` task not found, as a real agent answers for a task that is not executing; the orchestrator falls back to `GetTask` polling. |
| anything else, for example the A2A 0.3 `message/send` | `-32601` method not found |

```mermaid
stateDiagram-v2
  [*] --> Received: POST /a2a with a bearer token
  [*] --> Unauthorized: no bearer token (401, plain text)
  Received --> FollowUp: message has a taskId
  Received --> Errored: keyword error
  Received --> Rejected: keyword reject
  Received --> Failed: keyword fail
  Received --> Asking: keyword ask
  Received --> Slow: keyword slow
  Received --> Completed: no keyword
  FollowUp --> Asking: keyword ask
  FollowUp --> Completed: any other text
  Asking --> [*]: stream ends, the task stays input-required
  Slow --> Completed: frames arrive over 8 s
  Completed --> [*]
  Failed --> [*]
  Rejected --> [*]
  Errored --> [*]
  Unauthorized --> [*]
```

### Release channels (`mock-agent-releases`)

Its card declares `https://agents.vymalo.com/a2a/extensions/release-channels/v1` with the example of
the extension contract: default channel `production`; channels `production` = `coder-r47`, `staging` =
`coder-r51`, `latest` = `coder-r53`; revisions `coder-r53`, `coder-r51`, `coder-r47`. The UI's
release dropdown appears for it, and only for it. The scenario keywords above work the same.

On a new task the mock reads `params.message.metadata[<extension URI>].release`, resolves it and
echoes `{"requested": …, "revision": …}` in the task metadata of every event (no release means the
default channel, echoed as `requested: production`). The thread's events then carry the revision
(`actor.revision`). A `release` that is neither a channel nor a listed revision **fails the task**,
naming the value; it never falls back to the default. (The orchestrator already refuses such a thread
with a 400 before it reaches the agent, so the failure is only reachable by calling the mock directly.)

### Limits

- **No state.** Ids are derived from the request: the task id is `task-<messageId>`, message ids
  are built from it, the context id is the request's. `GetTask` therefore returns `completed` for any
  id, and a task's release metadata is echoed only on its first turn.
- **JSON-RPC ids are echoed as strings**, which is what the A2A SDK the orchestrator uses sends
  (UUIDv7). A client that sends numeric ids gets them back quoted.
- **Text is not echoed.** Putting request text into a JSON body without escaping could produce
  invalid JSON, so the answers are fixed sentences.
- Only the JSON-RPC binding is offered (`supportedInterfaces` lists `JSONRPC`, version `1.0`).

## Driving it

```sh
docker compose up -d --wait
dev/check-mocks.sh                                # one call per scenario (curl, jq)
```

With the `app` profile up, or any orchestrator that serves the chat API:

```sh
dev/try-thread.sh "add a health endpoint"                        # done: 5 events
dev/try-thread.sh "ask me which branch"                          # blocked, prints the thread id
THREAD_ID=<id> dev/try-thread.sh "use main"                      # the answer: done
dev/try-thread.sh "fail please"                                  # failed
AGENT_ID=mock-coder-releases RELEASE=staging dev/try-thread.sh "ship it"   # events carry [coder-r51]
```

The script prints one line per event (`seq`, kind, detail) and exits 0 for `done` and `blocked`.
`BASE_URL` (default `http://127.0.0.1:8080`, the edge) and `AUTH_EMAIL` point it elsewhere.

### The orchestrator on the host

```sh
docker compose up -d --wait                       # postgres + mocks only
cd orchestrator
MOCK_AGENT_TOKEN=dev-mock-token \
DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch \
AGENTS_FILE=../dev/agents.local.yaml \
AUTH_DEV_USER=dev@example.com \
LOG_FORMAT=text \
LISTEN_ADDR=127.0.0.1:8090 \
  cargo run -p orchestrator
BASE_URL=http://127.0.0.1:8090 ../dev/try-thread.sh "add a health endpoint"
```

For the UI on top of it, `MOCK_API_ORIGIN=http://127.0.0.1:8090 pnpm dev` in `web/` (that variable is
the dev rewrite of `/api/*`, whatever serves it). `pnpm dev:mock` is the web app's own contract mock
and needs none of this.

### The Rust test against the mocks

`orchestrator/crates/e2e/tests/wiremock_agent.rs` runs the real chat API, dispatcher and A2A adapter
against the mocks: the default script, `ask` and its answer, `fail`, `error` and `reject`, cancelling
a blocked thread, and the release echo. It skips unless told where the mocks are:

```sh
docker compose up -d --wait mock-agent mock-agent-releases
cd orchestrator
ORCH_TEST_MOCK_AGENT_URL=http://127.0.0.1:8081 \
ORCH_TEST_MOCK_AGENT_RELEASES_URL=http://127.0.0.1:8082 \
  cargo test -p orch-e2e --test wiremock_agent
```

`cargo test --workspace` against the compose database works too:
`ORCH_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_test`.

## Changing a mock

Stubs are files: `wiremock/<mock>/mappings/*.json` (matching and response settings, one stub per file,
lower `priority` wins) and `wiremock/<mock>/__files/*` (bodies; JSON-RPC frames use Handlebars
templates, see WireMock's response templating). The two mocks are separate directories so each can
diverge; a change to a shared behaviour goes into both. The directories are mounted read-only, so
after editing run `docker compose restart mock-agent mock-agent-releases`, or reload the stubs with
`curl -X POST http://127.0.0.1:8081/__admin/mappings/reset`. To see what a client actually sent:
`curl http://127.0.0.1:8081/__admin/requests` and, for requests no stub matched,
`/__admin/requests/unmatched`.

## What was checked

*Verified 2026-09-29* against the `wiremock-standalone-3.13.2.jar` (the version compose pins, run
with the same `--global-response-templating` flag and the same directories): every scenario above by
`dev/check-mocks.sh`; the real `orchestrator` binary (debug build) on Postgres 16 driven with
`try-thread.sh` for the default, `ask` plus its answer, `fail`, `error`, `reject`, `slow`, the three
release selections and cancel; `wiremock_agent.rs` (6 tests); `dev/Caddyfile` with `caddy validate`
and `caddy run` from the 2.11.4 release (client-supplied `X-Auth-Request-Email` replaced, SSE passing
through); `docker compose config` for both profiles; the image tags exist on Docker Hub.

*Unverified*: `docker compose up` itself, that is the containers, the image healthchecks, the two
image builds and the WireMock image's argument handling. The machine that wrote this had no Docker
daemon. CI (`.github/workflows/compose.yml`) starts the default profile and runs the same checks;
the `app` profile is only parsed there.
