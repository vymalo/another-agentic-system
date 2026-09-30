# Local development stack

What `compose.yaml` at the repository root starts, and how to steer the mock agents. The quick
start is in the root [README](../README.md#local-development); this page is the reference.

The mocks are [WireMock](https://wiremock.org/) 3.13.2 stubs of the one protocol the orchestrator
speaks to agents: A2A 1.0 over JSON-RPC and SSE. They stand in for a real coding agent so the whole
loop (chat, orchestrator, Postgres, A2A adapter) runs on a laptop without an agent host, a model or
a GitHub token. They hold no state and are not a substitute for the agent contract tests of a real
host. The `app` profile also runs a real agent, adam-coder, the default agent
([below](#the-default-agent)).

## Services

| Service | Image | Host port (127.0.0.1) | Profile | What it is |
|---|---|---|---|---|
| `postgres` | `postgres:16.15-alpine` | `5432` (`POSTGRES_PORT`) | default | The orchestrator's database `orch`, and `orch_test` for `cargo test`. User and password are both `postgres`. Named volume `postgres-data`. |
| `mock-agent` | `wiremock/wiremock:3.13.2` | `8081` (`MOCK_AGENT_PORT`) | default | A fake A2A 1.0 coding agent. |
| `mock-agent-releases` | `wiremock/wiremock:3.13.2` | `8082` (`MOCK_AGENT_RELEASES_PORT`) | default | The same agent, declaring the [release-channels extension](https://github.com/vymalo/another-agentic-platform/blob/main/docs/extensions/release-channels-v1.md). |
| `orchestrator` | built from [`orchestrator/`](../orchestrator/Dockerfile) | not published | `app` | The real orchestrator, with [`dev/agents.yaml`](agents.yaml): the coder first (the default agent), then the two mocks. `ORCH_ROLE` is `all` unless `ORCHESTRATOR_ROLE` says otherwise, and `ORCH_SURFACES` is not set, so it is the default `agui`: the AG-UI routes the web and the scripts here run on, beside the resource API. The legacy chat API routes were removed on 2026-09-30 (`ORCH_SURFACES` naming `chat-api` stops the orchestrator at startup). |
| `web` | built from [`web/Dockerfile`](../web/Dockerfile) | not published | `app` | The real chat UI. |
| `edge` | `caddy:2.11.4-alpine` | `8080` (`EDGE_PORT`) | `app` | Stands in for oauth2-proxy: one origin for the UI, the API (`/api/*`) and the AG-UI routes (`/agui/*`, streams unbuffered). |
| `orchestrator-worker-1`, `orchestrator-worker-2` | the `orchestrator` image | not published | `split` | Workers: `ORCH_ROLE=worker`, so the dispatcher and a port that serves only `/healthz`, `/readyz` and `/metrics`. The instance id is the service name (it is the `lease_owner` of the outbox rows they hold) and the lease is 5 s. See [the split profile](#the-split-profile-a-control-plane-and-two-workers). |
| `coder` | `ghcr.io/vymalo/another-adam-rs/coder`, pinned by tag and digest | `8090` (`CODER_PORT`) | `app` | adam-coder, the default agent: an A2A agent that turns a task into a branch and a pull request. About 2.9 GB, `linux/amd64` only. |
| `coder-postgres` | `postgres:16.15-alpine` | not published | `app` | The coder's own database, `coder`. Named volume `coder-postgres-data`. |
| `mock-openai` | `wiremock/wiremock:3.13.2` | `8091` (`MOCK_OPENAI_PORT`) | `app` | The coder's model endpoint: two scripts, `mock-coder` and `mock-opencode`. Vendored, see [`coder/UPSTREAM`](coder/UPSTREAM). |
| `mock-github` | `wiremock/wiremock:3.13.2` | `8092` (`MOCK_GITHUB_PORT`) | `app` | The GitHub REST subset the coder uses to open a pull request. Vendored. |
| `git-server` | built from [`coder/git-server/`](coder/git-server/Dockerfile) | `8093` (`GIT_SERVER_PORT`) | `app` | A git remote over smart HTTP, seeded with `local/sandbox.git`. No authentication. Vendored. |

The default profile builds nothing and starts in seconds. `--profile app` builds the two images
(the Rust build takes a few minutes the first time) and the git server, and pulls the coder image.

```mermaid
sequenceDiagram
  actor U as Browser or curl
  participant E as edge (Caddy)
  participant W as web
  participant O as orchestrator
  participant P as postgres
  participant M as mock-agent (WireMock)
  U->>E: GET / , /api/* and /agui/* on 127.0.0.1:8080
  E->>W: everything except /api/* and /agui/*
  E->>O: /api/* and /agui/* with X-Auth-Request-Email: dev@example.com (any client value replaced)
  O->>P: append the event, enqueue the delegation
  O->>M: GET /.well-known/agent-card.json
  M-->>O: card (streaming, bearer scheme, interface URL from the Host header)
  O->>M: POST /a2a SendStreamingMessage, Authorization: Bearer dev-mock-token
  M-->>O: SSE: task, statusUpdate, artifactUpdate, statusUpdate
  O->>P: append agent_status, artifact and thread_state events
  U->>E: POST /agui/agents/{agentId} (the run), or GET /agui/threads/{id}/connect
  E->>O: SSE (unbuffered)
  O-->>U: the AG-UI frames, live
```

### The edge is not oauth2-proxy

`edge` puts the chat UI, the resource API and the AG-UI routes on one origin, as the production ingress does, and sets
`X-Auth-Request-Email: dev@example.com` on every API and AG-UI request, replacing whatever the client sent.
It authenticates nobody. It exists so the UI works locally without an identity provider; it must
never be exposed beyond `127.0.0.1` (the compose file binds it there) and never used in production,
where oauth2-proxy authenticates the user and the orchestrator trusts the header only because the
proxy owns it (see the identity notes in [`orchestrator/README.md`](../orchestrator/README.md)).

Without `--profile app`, talk to a host-run orchestrator with `AUTH_DEV_USER` or send the header
yourself, as [`try-thread.sh`](try-thread.sh) does.

## The default agent

The first entry of [`agents.yaml`](agents.yaml) is the default agent, and it is
[adam-coder](https://github.com/vymalo/another-adam-rs) ([ADR 0014](../docs/decisions/0014-adam-coder-default-agent-over-a2a.md)):
`GET /api/agents` lists it first and the chat UI preselects it. The two WireMock mocks stay in the
file, after it, to try the other thread endings.

**Provenance.** The image is the published one, pinned in `compose.yaml` by tag `sha-<7>` and digest.
Everything else the coder needs is vendored from the same adam-rs commit, named in
[`coder/UPSTREAM`](coder/UPSTREAM), byte for byte, under `coder/`:

| Vendored path | Upstream path | What it is |
|---|---|---|
| `coder/wiremock/mock-openai/` | `dev/wiremock/mock-openai/` | `mappings/coder-script.json` and `opencode-script.json`, plus the bodies they reference (`opencode-bash.sse`, `opencode-done.sse`, and `chat-text.sse` and `chat-text.json` as OpenCode's fallbacks). Nothing else of the upstream mock: an off-script request must be a 404. |
| `coder/wiremock/mock-github/` | `dev/wiremock/mock-github/` | `mappings/pulls.json` and its two bodies. |
| `coder/git-server/` | `dev/git-server/` | The Dockerfile, nginx config, entrypoint and the seed of `local/sandbox.git`. |

Do not edit them here. [`coder/check-vendored.sh`](coder/check-vendored.sh) compares every one with
`raw.githubusercontent.com` at the commit in `UPSTREAM`, checks that nothing is missing (every body file
a vendored mapping names is vendored too, and `coder/git-server/` holds exactly the files of
`dev/git-server/` upstream, listed through the GitHub API), and checks that `compose.yaml` pins the
image of that commit (`sha-<first 7 characters>@sha256:`); CI runs it first. The mappings are a
deliberate subset, the scripted coder run only: a mapping the coder starts to need upstream shows up
as an unmatched request in `dev/coder-e2e.sh`. To move to a newer adam-rs
commit, change the commit in `UPSTREAM`, refresh the copies, and re-pin the image, all in one change.

**The scripted run.** The coder's model is `mock-coder`, a script the mock follows by looking at which
tool-call ids the conversation already holds (it keeps no state). The message names the seeded
repository, and the coder then calls `prepare_workspace`, `delegate_to_opencode`, `run_checks`,
`commit_and_push` and `open_pull_request`, and ends with a text. OpenCode (model `mock-opencode`) runs
one `bash` call, `echo hello > hello.txt`. With `[mock:no-opencode]` in the message OpenCode is not
started and `run_checks` makes the file itself. The result is a branch `agent/<run id prefix>` on
`git-server` with `hello.txt` = `hello`, and one pull request created on `mock-github`.

```mermaid
sequenceDiagram
  actor U as coder-e2e.sh
  participant E as edge
  participant O as orchestrator
  participant C as coder
  participant M as mock-openai
  participant G as git-server
  participant H as mock-github
  U->>E: GET /api/agents, POST /agui/agents/coder (the first agent)
  E->>O: with X-Auth-Request-Email
  O->>C: SendStreamingMessage, bearer CODER_A2A_TOKEN
  C->>M: chat completions, model mock-coder (tool calls, one per turn)
  C->>G: clone local/sandbox.git, push agent/run-prefix
  C->>M: OpenCode runs, model mock-opencode, bash echo hello
  C->>H: POST /repos/local/sandbox/pulls
  C-->>O: artifacts branch and pull_request, then completed
  O-->>U: RUN_FINISHED, then the frames with the two artifacts
  U->>H: __admin journal, exactly one POST
  U->>M: __admin journal, nothing unmatched
  U->>G: ls-remote and clone, hello.txt is hello
```

```mermaid
stateDiagram-v2
  [*] --> Created: POST /agui/agents/coder
  Created --> Delegated: dispatcher sends the message
  Delegated --> Scripted: mock-coder answers each turn
  Scripted --> Scripted: next tool call
  Scripted --> Published: branch pushed, pull request created
  Scripted --> Off_script: a request the script does not know
  Off_script --> Failed: 404 from mock-openai
  Published --> Done: task completed
  Done --> Verified: journals and git match
  Failed --> [*]
  Verified --> [*]
```

**Run it.**

```sh
docker compose --profile app up -d --build --wait     # pulls the coder, builds git-server, orchestrator, web
dev/coder-e2e.sh                                       # OpenCode makes the change
NO_OPENCODE=1 dev/coder-e2e.sh                         # the check command makes it
docker compose down -v                                 # also forgets the pushed branches
```

`dev/coder-e2e.sh` goes through the edge and speaks AG-UI, as the web does (the legacy chat API was removed on
2026-09-30): it checks the default agent, runs the thread with one
`POST /agui/agents/coder` (a UUID it mints as `threadId`), waits for the thread to end `done`,
and prints one `ok` or `FAIL` line for each check: the run stream ends with `RUN_FINISHED`, the two artifacts
(the JSON the coder sent is in the `content.text` of the `vymalo.artifact` activities of
`GET /agui/threads/{id}/connect?mode=run`), exactly one `POST /repos/local/sandbox/pulls` on `mock-github`
with the branch as head and `main` as base, no unmatched request on `mock-openai` and at least one
`mock-opencode` request (none with `NO_OPENCODE=1`), and the branch with `hello.txt` on `git-server`. It
resets both journals first, so it can be run repeatedly. To use the chat by hand, open
http://127.0.0.1:8080, keep the coder selected and send
`In http://git-server:8080/local/sandbox.git (base branch main), add hello.txt containing hello.`
The pull request appears as JSON in the `pull_request` artifact, not as a link (adam-coder sends its URL
in a data part).

**Limits.**

- `linux/amd64` only and about 2.9 GB: the first `--profile app` run pulls it. Compose on an arm64
  machine needs emulation. The default profile does not start it.
- The coder's card advertises `PUBLIC_URL`, `http://coder:8080/` in compose, so **an orchestrator on the
  host cannot use it**: [`agents.local.yaml`](agents.local.yaml) leaves it out, and its default is
  `mock-coder`. Run the orchestrator in the compose network to use the coder.
- The model is a script, not a model: any task text other than the one above runs the same script
  (it always clones `local/sandbox.git` and writes `hello.txt`). `mock-openai` has no fallback for
  `mock-coder`, so a change of the coder's tool calls upstream shows up as a 404 and a failed thread.
- `mock-openai`, `mock-github` and `git-server` are the adam-rs mocks with all their limits (no
  authentication, no state beyond the journal and the repository). `docker compose down -v` is what
  gives the next run a fresh repository and fresh databases.
- The coder does not support `ListTasks`: if the orchestrator dies between sending a message and
  recording the task, the retry starts a second run (ADR 0014).

## The split profile: a control plane and two workers

The `split` profile runs what [ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)
describes: the `orchestrator` service as a **control plane** (the API and the surfaces, no dispatcher) and two
**workers** (`orchestrator-worker-1` and `-2`, the dispatcher only), all on the one Postgres. They share nothing
else: a thread created through the control plane is `queued` until a worker claims its outbox row.

```sh
ORCHESTRATOR_ROLE=control-plane docker compose --profile app --profile split up -d --build --wait
dev/split-e2e.sh
docker compose --profile app --profile split down -v
```

`ORCHESTRATOR_ROLE` is the role of the `orchestrator` service (default `all`, as before); the workers are always
`worker`. Left at `all`, the orchestrator delivers threads itself and `dev/split-e2e.sh` fails saying so. The edge
and the web UI are unchanged: they talk to `orchestrator:8080`, the control plane. The workers publish nothing;
every process serves `GET /metrics` (the outbox queue, see
[`docs/orchestrator.md`](../docs/orchestrator.md#observability-and-scaling)), which the edge does not route, so read it
from inside the network: `docker compose exec -T edge wget -qO- http://orchestrator-worker-1:8080/metrics`.
Log lines start `role=worker instance=orchestrator-worker-1`.

**The handover.** `dev/split-e2e.sh` creates a thread for `mock-coder` with the keyword `slow` (an 8 s answer, see
[the scenarios](#mock-agent-scenarios)), waits for `working`, reads which worker holds the delegate row
(`SELECT lease_owner FROM outbox ...`), kills that container with `SIGKILL`, and checks that the thread still ends
`done`: the survivor claims the row once the 5 s lease has lapsed, resubscribes (the mock answers task not found) and
polls `GetTask`, which is `completed`. It then asserts the row was claimed twice and ended `delivered`, that the
thread has exactly one `thread_state: done` event and that the control plane's `/metrics` shows nothing due or
leased. The killed worker is started again when the script ends. It prints one `ok` or `FAIL` line per check, like
`coder-e2e.sh`, and needs `curl`, `jq` and `docker compose`; CI runs it at the end of the Coder E2E workflow.

```mermaid
sequenceDiagram
  actor U as split-e2e.sh
  participant E as edge
  participant C as orchestrator (control plane)
  participant P as postgres
  participant A as orchestrator-worker-1
  participant B as orchestrator-worker-2
  participant M as mock-agent
  U->>E: POST /agui/agents/mock-coder (text with slow)
  E->>C: with X-Auth-Request-Email
  C->>P: thread, events, outbox row (pending)
  A->>P: claim the row (lease 5 s, attempts 1)
  A->>M: SendStreamingMessage, an 8 s answer
  U->>E: GET /api/threads/id, until working
  U->>P: lease_owner of the inflight row is worker-1
  U->>A: docker compose kill -s SIGKILL
  Note over A: no goodbye, no lease release
  B->>P: claim the row once the lease lapsed (attempts 2)
  B->>M: SubscribeToTask (task not found), then GetTask
  M-->>B: completed
  B->>P: thread_state done, row delivered
  U->>E: GET /api/threads/id, done, and connect?mode=run holds one RUN_FINISHED
  U->>C: /metrics through edge: due 0, leased 0
  U->>A: docker compose up (started again)
```

```mermaid
stateDiagram-v2
  [*] --> Queued: control plane commits the thread and the outbox row
  Queued --> HeldByOne: a worker claims the row (attempts 1)
  HeldByOne --> Working: the agent answers, task known
  Working --> Orphaned: the holder is killed, its lease runs out
  Orphaned --> HeldBySurvivor: the other worker claims it (attempts 2)
  HeldBySurvivor --> Done: task completed, row delivered
  Done --> Verified: one done event, nothing due or leased
  Verified --> [*]
```

**Limits.** Only the mock agent is used, so the resubscribe falls back to polling: a real agent that supports
`SubscribeToTask` is covered by the binary's smoke tests instead. The workers have no healthcheck (the image has no
shell), so `up --wait` returns when they run, and the script waits for each `/metrics` itself. Both workers' logs are
in `docker compose logs`.

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

With the `app` profile up, or any orchestrator that serves the AG-UI routes (the default):

```sh
dev/try-thread.sh "add a health endpoint"                        # done
dev/try-thread.sh "ask me which branch"                          # blocked, prints the thread id
THREAD_ID=<id> dev/try-thread.sh "use main"                      # the answer: done
dev/try-thread.sh "fail please"                                  # failed
AGENT_ID=mock-coder-releases RELEASE=staging dev/try-thread.sh "ship it"   # events carry [coder-r51]
```

The script runs the thread with `POST /agui/agents/{agentId}` (a UUID it mints as the thread id; `THREAD_ID`
sends a follow-up run to an existing thread) and prints one line per AG-UI frame of the thread (number,
event type, detail; `[coder-r51]` marks the revision that answered), read from
`GET /agui/threads/{id}/connect?mode=run`. It exits 0 for `done` and `blocked`.
`BASE_URL` (default `http://127.0.0.1:8080`, the edge) and `AUTH_EMAIL` point it elsewhere. Its default
target is `mock-coder`, not the default agent: the real coder is driven by
[`coder-e2e.sh`](#the-default-agent).

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

`orchestrator/crates/e2e/tests/wiremock_agent.rs` runs the real dispatcher and A2A adapter (driven over the AG-UI run route of the test instance: `Chat::create_thread`, `Chat::follow_up`)
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
the `app` profile is only parsed there, and run by the `Coder E2E` workflow (below).

The default agent (the `coder` service, its mocks and `dev/coder-e2e.sh`):

*Verified 2026-09-29*:

- The vendored files are byte-identical to `vymalo/another-adam-rs` at `0e08fe07eaea` (the scripted mocks landed in `22d7c6e126b8` and did not change since) (`dev/coder/check-vendored.sh`, over
  `raw.githubusercontent.com`; the same script also asserts the image pin, see below).
- The vendored WireMock mappings load in `wiremock-standalone-3.13.2.jar` (16 mappings for `mock-openai`, 13 for
  `mock-github`, run with `--global-response-templating`), and the whole `mock-coder` script, the `mock-opencode` turns
  and the pull request creation answer as scripted when driven turn by turn with the histories a client sends.
- `dev/coder-e2e.sh`, both variants, against those two WireMock instances, a bare git repository seeded from the
  vendored `seed/` and a stand-in chat API that ran the script and pushed the branch: every check printed `ok`, exit 0;
  with a thread that ended `failed` it printed `FAIL` lines and exited 1; with the API down it failed at the first call.
  This proves the script's jq paths and journal queries, not the real stack.
- The scripts on AG-UI (2026-09-29, after the chat API went off by default): `dev/try-thread.sh` (a new thread, a
  blocked one and its answer with `THREAD_ID`) and the AG-UI half of `dev/coder-e2e.sh` (the run stream ends
  with `RUN_FINISHED`, the thread ends `done`, its frames are fetched with `connect?mode=run`; the artifact `jq`
  paths on hand-written frames) against the real `orchestrator` binary at its default `ORCH_SURFACES` (`agui`), the
  `orch-fake-agent` and a real Postgres; the background run and the frame count of `dev/split-e2e.sh` the same
  way. This proves the AG-UI calls and the `jq` paths, not the compose stack: the mock and git checks of
  `coder-e2e.sh` and the docker steps of `split-e2e.sh` still run only in CI.
- `docker compose config` (Compose v5.1.1, no daemon) accepts both profiles; `shellcheck dev/*.sh dev/coder/*.sh` and
  `actionlint` are clean; the docs check passes.
- The ghcr manifest and digest of `coder:sha-0e08fe0` (anonymous token, `docker-content-digest`), and the tag list of
  the repository. adam-rs's `coder` workflow built, smoke-tested and ran its own compose e2e (both variants) on this
  image before pushing it.

The split profile (`orchestrator-worker-*`, `dev/split-e2e.sh`):

*Verified 2026-09-29*: `docker compose config` (Compose v5.1.1, no daemon) accepts every combination of the `app`
and `split` profiles, and the resolved `orchestrator` service differs from the earlier one only by `ORCH_ROLE=all`;
`shellcheck` and `actionlint` are clean; `dev/split-e2e.sh` against a stand-in `docker` and a stand-in chat API over a
real Postgres, in the passing case and with the wrong owner, a missing `/metrics`, a second done event and a wrong
attempt count (this proves the script's control flow, its queries and its `jq` paths, not the stack); the binary's smoke
test with one control plane and two workers, a SIGKILL of the holder and a finish by the other, over real processes and
a real Postgres.

*Unverified*: the split profile running in containers: `up --wait` with the workers (no healthcheck), the recreation of
`orchestrator` as a control plane while `edge` runs, `docker compose exec edge wget` (busybox `wget` in the caddy image is
assumed, as its healthcheck uses it), `docker compose kill` and `up` of a single worker, and the mock agent's
`SubscribeToTask` and `GetTask` answers during the handover (from `dev/README.md`, checked only by curl). The first run is
the `Coder E2E` workflow.

*Unverified*:

- **A run of the stack.** The machine that wrote this had no Docker daemon: the coder container, its health check and
  environment (variable names were read from `crates/adam-coder/src/config.rs` at `22d7c6e`, unchanged at `0e08fe0`), the git-server build,
  `up --no-build --wait`, and `dev/coder-e2e.sh` against the real orchestrator, web, edge and coder. The first run is the
  `Coder E2E` workflow.
- That `mock-openai` matches every request of a real coder and OpenCode without the upstream `models.json`,
  `chat-completions.json` and `errors.json`, which are not vendored: the "nothing unmatched" check will say.
- That the orchestrator's delegation of a run that takes minutes stays within its own limits on the CI runner.
