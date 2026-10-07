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
> binary and image. The legacy chat API interaction routes were removed on 2026-09-30. Since MVP slice 5 the
> inbox, watches and timers are built: the inbox worker applies timers (the CI and verifier deadlines the
> gate schedules) and the reports that `App::receive` stores. Since slice 10 a verifier agent can be part of the verify/rework
> gate: the dispatcher asks it over A2A and its verdict decides. The MCP server (`start_job`,
> `get_job`, `wait_for_job` with progress notifications, `answer`, `cancel_job`, `list_agents` with bearer tokens, slices
> 11 and 12 of [`docs/mvp.md`](../docs/mvp.md)) is built and off unless `ORCH_SURFACES` names `mcp`. Since slices 6 and 9 the CI webhooks (`POST /webhooks/ci` generic and `POST /webhooks/github`,
> machine routes, `ORCH_SURFACES=agui,webhook-generic,webhook-github`) write those reports and `ci` is a source the gate honours.
> The planner and reviewers
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

**One YAML file** (`ORCH_CONFIG_FILE`, or `--config`; `version: 1`, secrets only by reference, every key and the
variable it replaces in [`docs/api/config.md`](../docs/api/config.md), the JSON Schema at
[`docs/api/config.schema.json`](../docs/api/config.schema.json), [ADR 0034](../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md))
configures the process; `orchestrator --print-config` prints what it would run with (secrets as references) and opens no
connection. **In this transition release every variable below still works and wins over the file**, each logging one
warning that names the variable and the key; unset `ORCH_CONFIG_FILE` and the environment alone configures the process, as
before, with one warning. The details are in [`bin/orchestrator`](bin/orchestrator/README.md#the-configuration-file).
The artifact store (`artifacts:`, [ADR 0032](../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md): `store: fs` or `s3`, `maxFileBytes`, `maxPerJobBytes`, `fetchHosts`) has no variable, only the file; without the section a file an agent hands over is refused ("no artifact store configured").

Every setting is a command-line flag with an environment fallback (`orchestrator
--help` lists both); the variables below are what deployments set, and a flag
wins over its variable. The process refuses to start, with a message naming the
culprit, when a value is wrong (exit code 78; a malformed command line exits 2).
An empty value counts as unset.

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string. Never logged. |
| `AGENTS_FILE` | required, unless `AGENT_REGISTRY_URL` is set | YAML list of `{id, name, aliases?, transport?, cardUrl?, tokenEnv?, agent?, gate?}` (`gate` is the entry's verification gate, see the `ORCH_GATE` rows below and [`bin/orchestrator`](bin/orchestrator/README.md#the-verification-gate); `transport` is `a2a`, the default, which needs `cardUrl`; `local` is an in-process agent named by `agent`, served only by a build with the Cargo feature `agent-local` and refused at startup otherwise), see [`agents.example.yaml`](agents.example.yaml). Ids are unique slugs; `aliases` are other names of the agent (an alias that is another entry's id or alias is a startup error): the agent is listed and a thread is created under its `id`, and a request, a mention or an older thread that says an alias is about it ([ADR 0049](../docs/decisions/0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md)); a `tokenEnv` that names an unset or empty variable is a startup error, not an unauthenticated agent. The first entry is the default agent the chat UI preselects ([ADR 0014](../docs/decisions/0014-adam-coder-default-agent-over-a2a.md)). |
| `AGENT_REGISTRY_URL` | unset | The platform's agent registry (`agent-registry/v1`, [ADR 0022](../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md)): the full URL of the document (`http` or `https`, no user name or password; flag `--registry-url`). Its agents are read live beside the `AGENTS_FILE` agents, which come first; a registry that cannot be read leaves only those and `GET /api/registry` says so. With it set `AGENTS_FILE` may be unset or empty. Needs the Cargo feature `registry-platform` (on by default; a URL without it is a startup error). |
| `AGENT_REGISTRY_TOKEN`, `AGENT_REGISTRY_AGENT_TOKEN` | unset | The bearer token sent to the registry, and the one sent to every agent it lists (open question 11). Never logged. |
| `AGENT_REGISTRY_TIMEOUT_SECS`, `AGENT_REGISTRY_MAX_AGE_SECS` | `3`, `60` | 1 to 60, and 1 to 3600: how long one read of the registry may take, and the longest a copy of the document is kept (whatever its `Cache-Control` allows). |
| `LISTEN_ADDR` | `0.0.0.0:8080` | Control plane: the API. Worker: the probes only. |
| `ORCH_ROLE` | `all` | What this process runs: `all`, `control-plane` (server, API and surfaces; no dispatcher, no inbox worker) or `worker` (dispatcher, inbox worker, and a router with only `/healthz` and `/readyz`). Flag `--role`; the enum is `adam_host::Role` ([ADR 0015](../docs/decisions/0015-control-plane-and-workers-on-adam-rs.md)). The role table is in [`bin/orchestrator`](bin/orchestrator/README.md#roles). An unknown value is a startup error. |
| `ORCH_SURFACES` | `agui` | Comma-separated interaction surfaces to mount (flag `--surfaces`). Known: `agui`, `mcp`, `thread-tools`, `webhook-generic` and `webhook-github`. `agui` is the AG-UI routes `POST /agui/agents/{agentId}`, `GET /agui/threads/{threadId}/connect` and `GET /agui/agents/{agentId}/capabilities` ([ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md), [`docs/api/agui.md`](../docs/api/agui.md)); `mcp` is the MCP server at `/mcp` ([ADR 0019](../docs/decisions/0019-mcp-server-over-streamable-http.md), [`orch-surface-mcp`](crates/surface-mcp/README.md)), which needs `MCP_TOKENS_FILE` and `MCP_ALLOWED_HOSTS` below; `thread-tools` is the per-thread MCP endpoint `/thread-tools/{threadId}/mcp` that an agent listing `thread-tools/v1` calls back with a short-lived token ([`docs/api/thread-tools-v1.md`](../docs/api/thread-tools-v1.md), [`orch-surface-thread-tools`](crates/surface-thread-tools/README.md)), which needs `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL` below, in every role; `webhook-generic` is `POST /webhooks/ci`, the generic signed CI report ([ADR 0017](../docs/decisions/0017-ci-results-by-webhook.md), [`docs/api/webhooks.md`](../docs/api/webhooks.md)), which needs `WEBHOOK_GENERIC_SECRETS` below; `webhook-github` is `POST /webhooks/github`, GitHub's `check_run` and `workflow_run` deliveries, which needs `WEBHOOK_GITHUB_SECRETS`. `mcp`, `thread-tools` and the webhooks are **machine routes** with no user identity. The legacy `chat-api` surface (`createThread`, `postMessage`, `listEvents`, `streamEvents`) was **removed on 2026-09-30**: naming it fails closed (exit 78, an error that says it was removed and points to AG-UI, see [`bin/orchestrator`](bin/orchestrator/README.md#surfaces)). An unknown name, an empty list (`,`), a repeat, or a surface whose Cargo feature (`surface-agui`, `surface-mcp`, `surface-thread-tools`, `surface-webhook`) is not in the build is a startup error. The resource API and health are always mounted. |
| `MCP_TOKENS_FILE` | required with `mcp` | YAML list of `{user, tokenEnv}` (flag `--mcp-tokens-file`): the e-mail that owns the jobs of a bearer token, and the environment variable that holds the token. An unset or empty variable, an unreadable or malformed file, a user that is not an e-mail address, or one token for two users is a startup error (78). A user may have several tokens (a rotation). Not read by a `worker`. See [`dev/mcp-tokens.yaml`](../dev/mcp-tokens.yaml). |
| `MCP_TOKEN_<NAME>` | required by the file | Whatever a `tokenEnv` names: the bearer token. Secret; never logged, compared in constant time; at least 32 bytes (`openssl rand -hex 32`), or startup fails (78). |
| `MCP_ALLOWED_HOSTS` | required with `mcp` | Comma-separated `Host` values the MCP server accepts (flag `--mcp-allowed-hosts`), for example `orch.example.com`; a name without a port matches any port. Anything else is 403 (DNS rebinding). Empty is a startup error, and so is an entry that is not `host` or `host:port` (a URL, `*`, a port that is not a number). |
| `MCP_WAIT_MAX_SECS` | `3600` | The largest `timeout_secs` the MCP tool `wait_for_job` honours (flag `--mcp-wait-max-secs`), 1 to 86400; a larger request is cut to it. Anything else is a startup error. |
| `MCP_WAIT_MAX_CONCURRENT`, `MCP_WAIT_MAX_PER_USER` | `256`, `16` | The most `wait_for_job` calls one process, and one user, may hold open (flags `--mcp-wait-max-concurrent`, `--mcp-wait-max-per-user`), at least 1; a call over either is a tool error "too many waits". |
| `MCP_ALLOWED_ORIGINS` | none | Comma-separated browser origins (`https://app.example.com`) the MCP server lets through (flag `--mcp-allowed-origins`). A request with any other `Origin` is 403; clients that are not browsers send none. |
| `ORCH_PUBLIC_URL` | unset | The chat's public origin, for example `https://chat.example.com` (flag `--public-url`). The MCP server gives `start_job` a `web_url` of `<origin>/threads/<job_id>` from it; without it there is none. A malformed value (not an http(s) origin) is a startup error. |
| `AUTH_DEV_USER` | unset | An e-mail served for requests **without** `X-Auth-Request-Email`. Development only: the orchestrator logs a warning at boot, and the file refuses it with an `auth.mode` other than `proxy_header`. Unset, such requests get 401. |
| `DATABASE_MAX_CONNECTIONS` | `10` | At least 2: the wakeup listener holds one connection. |
| `DISPATCHER_CONCURRENCY` | `32` | Delegations processed at the same time by this replica. |
| `ORCH_TITLE_MODEL`, `ORCH_MODEL_BASE_URL`, `ORCH_MODEL_API_KEY`, `ORCH_MODEL_TIMEOUT_SECS` | unset, unset, unset, `20` | The model that writes thread titles ([ADR 0005](../docs/decisions/0005-openai-compatible-model-endpoint.md)): an OpenAI-compatible endpoint (`…/v1`, up to and not including `/chat/completions`), its bearer token when it wants one, and the model's name there; the first reply of the agent asks it for a 3 to 6 word title (twice at most per thread; a person's rename is final). **`ORCH_TITLE_MODEL` unset turns titles off**; set, it needs `ORCH_MODEL_BASE_URL` (exit 78). A model that is down or says nothing costs the thread nothing. **Deprecated** for the configuration file's `models.endpoints` (several, each with its own key and timeout) and `tasks.title` / `tasks.description` (each with its endpoint, model, guidance, token limit and language rule: [`docs/api/config.md`](../docs/api/config.md), [ADR 0035](../docs/decisions/0035-utility-model-tasks.md)), where the thread's description is configured too. See [`bin/orchestrator`](bin/orchestrator/README.md#environment). |
| `OUTBOX_LEASE_SECS` | `30` | How long after a crash another replica waits before taking a delegation over (a graceful shutdown hands over at once). Also the lease of a local agent's run. |
| `AGENT_LOCAL_CONCURRENCY` | `4` | Only in a build with the Cargo feature `agent-local`: runs of local agents stepped at once by this replica (at least 1). The local agents open a pool of their own, this plus 4 connections, on top of `DATABASE_MAX_CONNECTIONS`. |
| `INBOX_LEASE_SECS` | `30` | At least 3. How long after a crash another replica waits before taking over an inbox row (a timer or a stored report) its worker had claimed (a graceful shutdown hands over at once). |
| `INBOX_POLL_SECS` | `2` | At least 1. Safety poll of the inbox worker when no wakeup arrives; a timer fires at most this late, because timers coming due are not announced. |
| `INBOX_PARKED_TTL_SECS` | `86400` | At least 1. How long a report that no thread watches yet waits (parked) before it expires. |
| `INBOX_MAX_ATTEMPTS` | `10` | At least 1. Claims of one inbox row before it is dead-lettered (a row that keeps failing retryably, or keeps killing its worker: one claimed more often than this without being finished is given up on before it is delivered). A claim handed back at shutdown, or one that only parked the row, is not counted. |
| `SHUTDOWN_GRACE_SECS` | `15` | Bound of each graceful-shutdown step. |
| `ORCH_GATE` | none | The sources every job that **pushed a commit** must pass before it is `done` (a job whose agent pushed nothing gave an answer and is not verified: [ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md#status-note-2026-10-04-only-pushed-work-is-verified)): a comma list of `ci`, `agent-checks`, `verifier` (flag `--gate`; [ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md)). None is no gate. This build honours all three (`ci` since slice 6, `verifier` since slice 10). A `ci` gate needs reports: mount `webhook-generic`. |
| `THREAD_TOOLS_SECRET` | none | The HMAC key of the thread-tools tokens, at least 32 bytes (`openssl rand -hex 32`; flag `--thread-tools-secret`). With `THREAD_TOOLS_URL` it makes the A2A adapter give a grant (the endpoint's address and a token) to every agent whose card lists `thread-tools/v1`, and it opens the surface `thread-tools`. **Set both or neither** (exit 78 otherwise), and give **every role** the same values: workers mint, the control plane serves, so naming `thread-tools` in `ORCH_SURFACES` requires them in a worker too. Never logged. |
| `THREAD_TOOLS_SECRET_PREVIOUS` | none | The previous key, for verifying only: tokens signed with it still open the endpoint, so the key rotates without refusing the tokens in flight (move the current key here, set a new one, remove this once the longest lifetime has passed). Must not be the current key. Never logged. |
| `THREAD_TOOLS_URL` | none | The base URL under which agents reach this orchestrator, `http` or `https` (`http://orchestrator:8080`); the grant tells an agent `<URL>/thread-tools/<threadId>/mcp`. Set with `THREAD_TOOLS_SECRET`. |
| `THREAD_TOOLS_TOKEN_TTL_SECS` | `7200` | How long a thread-tools token lives, 60 to 86400 (flag `--thread-tools-token-ttl-secs`). A task can run long, and a call that fails with 401 in the middle of one is hard for an agent to recover from. |
| `THREAD_TOOLS_ALLOWED_HOSTS` | the host of the URL | The `Host` values the surface `thread-tools` accepts, comma separated (flag `--thread-tools-allowed-hosts`); the default is the host of `THREAD_TOOLS_URL`, with its port when it names one (a name without a port matches any port). Guards against DNS rebinding; a request with another host is 403. |
| `ORCH_CI_REQUIRED` | none | Comma-separated names of the CI checks that must pass (flag `--ci-required`): GitHub's check name (`check_run`) or workflow name (`workflow_run`), or the `name` of a generic report. **A gate that requires `ci` must name at least one**, here or in an entry's `gate.ci.required`; without, the process exits 78 ("the first report decides" would let a red commit pass on a `skipped` report). A process that mounts no CI webhook surface refuses `ci` altogether (78; per-thread requests are 400): "no CI webhook surface is mounted (ORCH_SURFACES)". |
| `ORCH_CI_TIMEOUT_SECS` | `3600` | At least 1. How long a job waits for the CI reports its gate needs before it is blocked (`ci_timeout`; no attempt is used). An `AGENTS_FILE` entry's `gate.ci.timeoutSecs` overrides it. |
| `WEBHOOK_GENERIC_SECRETS` | none | One or two comma-separated shared secrets of `POST /webhooks/ci` (a signature by either is good, so a secret can be rotated). **Required when `ORCH_SURFACES` mounts `webhook-generic`** (exit 78 without, with a third, or with one under 32 bytes: `openssl rand -hex 32`); a `worker` serves no routes and does not need it. Never logged. |
| `WEBHOOK_GITHUB_SECRETS` | none | One or two comma-separated secrets of `POST /webhooks/github` (the secret of the repository's or organisation's GitHub webhook; `X-Hub-Signature-256` by either is good). **Required when `ORCH_SURFACES` mounts `webhook-github`** on a role that serves routes (exit 78 without, or with a third). Never logged. |
| `WEBHOOK_GENERIC_MAX_SKEW_SECS` | `300` | At least 1. How far `X-Vymalo-Timestamp` may be from the clock, either way. |
| `WEBHOOK_GITHUB_MAX_AGE_SECS` | `86400` | At least 1. How old the signed `completed_at` (`updated_at` for a workflow run) of a GitHub event may be; an older one is acknowledged (202) and not stored. |
| `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP` | `3`, `10` | Attempts a gated job's agent gets (the first included), and the most an `AGENTS_FILE` entry or a run may set them to (at most 100; a smaller cap lowers the default attempts). |
| `ORCH_VERIFIER` | none | The verifier agent's id: another configured agent than the ones it verifies (startup error otherwise). Used when the gate requires `verifier`. |
| `ORCH_VERIFIER_TIMEOUT_SECS` | `1800` | At least 1. How long a verification waits for the verifier's verdict before the thread waits for the user (no attempt is spent). |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | Names this replica in leases. |
| `RUST_LOG` / `LOG_FORMAT` | `info,rmcp=warn` / `json` | `LOG_FORMAT=text` for humans. The MCP library logs a line per request at `info`, so its default is `warn`; `RUST_LOG` replaces the default whole. |

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
| [`crates/ports`](crates/ports/README.md) | `orch-ports` | Traits `ThreadStore`, `Wakeup`, `AgentClient`, `ChatModel` (one question to a language model, with `NoModel`), `ToolServerClient` (an MCP server as the orchestrator calls it: list tools, call one; ADR 0024), `ArtifactStore` (the files agents hand over, by the hash of their content, with `NoArtifacts`), `Clock`, `IdGen`, and `ByTransport` (routes an endpoint to the A2A or the local client); feature `testkit` adds in-memory implementations, a scripted fake agent and the conformance testkit. |
| [`crates/agui-proto`](crates/agui-proto/README.md) | `orch-agui-proto` | AG-UI 1.0 wire types as closed serde enums (all 31 events, `RunAgentInput`), the vendored official JSON Schema, and a `testkit` that validates against it. No `orch-*` dependencies. |
| [`crates/agui-projection`](crates/agui-projection/README.md) | `orch-agui-projection` | Pure: the AG-UI view of the event log (`Projector`: events to frames, audiences, resume preamble) and the translation of a `RunAgentInput` to core inputs. Depends on `orch-core` and `orch-agui-proto` only; no async, no I/O. |
| [`crates/app`](crates/app/README.md) | `orch-app` | Thread service (`transition` + optimistic commit loop, live event streams), the pure roles-to-permissions model `authz` that it enforces on every read and act ([ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)), the durable outbox `Dispatcher` and the `InboxWorker` (timers and stored reports), written against the ports. |
| [`crates/api`](crates/api/README.md) | `orch-api` | The always-mounted HTTP edge: identity from the `Authenticator` (fail closed) and a 403 for roles that grant nothing, RFC 9457 problems, the resource API (agents, threads, cancel, `GET /api/me`), health, and `SurfaceRoutes`, the mounting point of interaction surfaces (behind the identity layer, or, as a `machine` route with its own guard, outside it). |
| [`crates/surface-mcp`](crates/surface-mcp/README.md) | `orch-surface-mcp` | The MCP server at `/mcp` (rmcp, streamable HTTP, stateless): `list_agents`, `start_job`, `get_job`, `wait_for_job`, `answer`, `cancel_job` behind static bearer tokens; a machine route straight to `App`. Feature `surface-mcp` of the binary, on by default, mounted by `ORCH_SURFACES=mcp`. |
| [`crates/config`](crates/config/README.md) | `orch-config` | Pure: the configuration file ([ADR 0034](../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md)): the typed keys, the JSON Schema committed at `docs/api/config.schema.json`, syntax / shape / rules passes that list every error and never a value, secrets only as `{ env }` or `{ file }` references. |
| [`crates/thread-token`](crates/thread-token/README.md) | `orch-thread-token` | Pure: the thread-tools token (HS256 JWS, ten claims, the current and the previous key), `mint`, `verify` and the issuer that gives an agent `{url, token, expiresAt}`; known-answer vectors. |
| [`crates/surface-thread-tools`](crates/surface-thread-tools/README.md) | `orch-surface-thread-tools` | The per-thread MCP endpoint `/thread-tools/{threadId}/mcp` (rmcp, streamable HTTP, stateless): `get_ui_catalog` behind the thread-scoped token, and the provider seam later slices add tools through, whose first provider is the relay of the MCP servers attached to the thread (`<server>__<tool>`, each call one step with the server's icon; the binary's feature `tool-relay`, on by default); a machine route straight to `App`. Feature `surface-thread-tools` of the binary, on by default, mounted by `ORCH_SURFACES=thread-tools`. |
| [`crates/surface-webhook`](crates/surface-webhook/README.md) | `orch-surface-webhook` | The webhook surfaces: `POST /webhooks/ci` (a signed CI report, HMAC-SHA-256 over the timestamp and body) and `POST /webhooks/github` (GitHub's own deliveries, HMAC over the body), secrets with rotation, stored in the inbox through `App::receive`. Machine routes. |
| [`crates/store-postgres`](crates/store-postgres/README.md) | `orch-store-postgres` | `ThreadStore` + `Wakeup` on Postgres (sqlx): per-thread `seq` from a counter row in the writing transaction, outbox claims with `FOR UPDATE SKIP LOCKED` leases, `LISTEN/NOTIFY`, embedded idempotent migrations. |
| [`crates/auth-jwt`](crates/auth-jwt/README.md) | `orch-auth-jwt` | `Authenticator` over OAuth2 bearer tokens: JWTs validated against the issuer's JWKS (RS256, RS384, ES256, EdDSA), a key cache refreshed after 10 minutes and fetched again for an unknown `kid` at most once in 30 s, failing closed. Feature `auth-jwt` of the binary. |
| [`crates/auth-header`](crates/auth-header/README.md) | `orch-auth-header` | `Authenticator` over `X-Auth-Request-Email` and the optional development user (what `orch-api` did before ADR 0033). Feature `auth-header` of the binary. |
| [`crates/registry-platform`](crates/registry-platform/README.md) | `orch-registry-platform` | `AgentRegistry` over the platform's `agent-registry/v1` (an RFC 9727-shaped linkset of agent cards): read live over HTTP with its cache headers and a validator, in this process only, single flight, failing closed (a read that fails drops the copy; its agents are not listed and the source says so). Feature `registry-platform` of the binary. |
| [`crates/agent-a2a`](crates/agent-a2a/README.md) | `orch-agent-a2a` | `AgentClient` over `a2a-client-lf` (A2A 1.0): live card and release-channels discovery, streaming delegation, resubscribe, polling, cancel. |
| [`crates/model-openai`](crates/model-openai/README.md) | `orch-model-openai` | `ChatModel` over an OpenAI-compatible `POST {base}/chat/completions` (`reqwest`, no vendor SDK): the orchestrator's first model call, the title of a thread. The key is a sensitive header, never in an error or a `Debug`. |
| [`crates/tools-mcp`](crates/tools-mcp/README.md) | `orch-tools-mcp` | `ToolServerClient` over MCP (`rmcp`'s client, streamable HTTP): list a server's tools and call one for the relay of the thread-tools endpoint ([ADR 0024](../docs/decisions/0024-mcp-tools-attached-per-conversation.md)), one session per request, the endpoint's timeout over the whole of it, the credentials on that server's requests only and in no error, a result cut at 256 KiB. Used by the relay of the thread-tools endpoint, composed by the binary behind its feature `tool-relay`. |
| [`crates/artifacts-fs`](crates/artifacts-fs/README.md) | `orch-artifacts-fs` | `ArtifactStore` over a directory ([ADR 0032](../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md)): temp file, fsync and atomic rename, mode `0600`, the meta in a JSON file beside the bytes; for development and one node. |
| [`crates/svg-clean`](crates/svg-clean/README.md) | `orch-svg-clean` | Pure: an allow-list sanitizer for SVG on `quick-xml` ([ADR 0032](../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md)); the API sends an SVG inline only through it |
| [`crates/artifacts-s3`](crates/artifacts-s3/README.md) | `orch-artifacts-s3` | `ArtifactStore` over an S3 bucket (AWS or any compatible server) through `object_store`'s S3 backend: one object per file, the media type as its `Content-Type`; the production store. |
| [`crates/agent-adam`](crates/agent-adam/README.md) | `orch-agent-adam` | `AgentClient` over adam-rs agents hosted in this process (`transport: local`), journaled in the orchestrator's Postgres under `orch_agent_`; only with the binary's feature `agent-local` (off by default). |
| [`crates/a2a-mapping`](crates/a2a-mapping/README.md) | `orch-a2a-mapping` | Pure: the mapping from A2A 1.0 stream items and tasks to `AgentEnvelope`s and idempotency keys (`StreamMapper`, `snapshot`). No I/O, no async, no HTTP client. |
| [`crates/testsupport`](crates/testsupport/README.md) | `orch-testsupport` | Test-only: an in-process fake A2A agent (`a2a-server-lf`), a running orchestrator on a TCP port, clients for the resource API and the AG-UI routes; the executable `orch-fake-agent` serves two scripted agents for the browser tests (`web/e2e-system`) and is never part of the image. |
| [`crates/e2e`](crates/e2e/README.md) | `orch-e2e` | Tests only: AG-UI surface, dispatcher and A2A adapter against a fake agent over real HTTP, on either store. |
| [`bin/orchestrator`](bin/orchestrator/README.md) | `orchestrator` | The composition root: flags, environment (clap) and `AGENTS_FILE` parsing (`config.rs`, unit-tested), the role (`ORCH_ROLE`, `adam_host::Role`), and the surfaces to mount and the wiring, startup and graceful shutdown on `adam_host::Host` (`boot.rs`). No logic of its own. |

Every crate has its own README (role, public API, environment, tests); update it
in the same change as the crate's API, environment variables or tests. The docs
check fails when one is missing.

Dependency direction: `core` ← `ports` ← `app` ← `api` ← the surface crates; adapters
(`store-postgres`, `agent-a2a`, `agent-adam`, `model-openai`, `tools-mcp`, `registry-platform`, `auth-jwt`, `auth-header`; `agent-a2a` and `agent-adam` build on the pure `a2a-mapping`) implement the ports;
only `bin/orchestrator` depends on all of them.

## Behaviour worth knowing

- **Identity.** An `Authenticator` ([ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md))
  decides who is calling, chosen by `auth.mode` in the configuration file (default `proxy_header`: the API trusts
  `X-Auth-Request-Email` and answers 401 without it on every path except `/healthz` and `/readyz`; `AUTH_DEV_USER`
  supplies an identity only when it is set). With `auth.mode: jwt` it validates the `Authorization: Bearer` token against
  the issuer's JWKS (401 with `WWW-Authenticate: Bearer`, 503 while the keys cannot be fetched, `/readyz` with it).
  The header is only trustworthy behind a proxy such as oauth2-proxy that strips client-supplied copies, and is refused
  when `server.environment` is `production`.
- **`thread_state` events** are appended only when a thread *enters* `blocked`,
  `done`, `failed` or `cancelled`; entering `queued`/`working` is implied by
  `user_message` / `agent_status`.
- **The inbox is for machine input only.** A surface a person uses (the AG-UI run route)
  runs the transition inside the request and writes the events and the outbox
  row in one transaction, so redeliveries cannot happen on this path (a retried AG-UI run
  carries an idempotency key on the event and attaches). What nobody asked for goes through the
  `inbox` table: `App::receive` stores a report (the same delivery id twice is one row) and the
  `InboxWorker` applies it in the thread's own commit; a report that no thread watches yet is
  parked, and the commit that starts the watch wakes it. A `Schedule` from the core is an inbox
  row too, due at the commit's time plus its delay, which the worker polls for. The webhook surface
  (`POST /webhooks/ci`, slice 6) is what calls `receive`: after it has checked the signature.
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

- **Static agents, and a registry.** The `AGENTS_FILE` is read once at boot;
  changing it means a restart. The agents of the platform's registry
  (`AGENT_REGISTRY_URL`, [ADR 0022](../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md))
  need no restart: they are read live, from a copy held in the process for as
  long as the registry's `Cache-Control` allows (at most
  `AGENT_REGISTRY_MAX_AGE_SECS`), and a registry that cannot be read lists none
  of its agents. Gate layers and the verifier (`gate` in the file, `ORCH_VERIFIER`)
  are for the file's agents only; a registry agent runs under the deployment's
  gate. Agent cards themselves are read live.
- **One user identity source.** The proxy header, or the dev user. No sessions,
  no roles: a user sees exactly their own threads.
- **Every replica migrates.** Boot runs the embedded migrations; sqlx's
  advisory lock serialises replicas. There is no separate migration job. **A rolling update is not safe for the
  release that adds `job_started`** ([ADR 0020](../docs/decisions/0020-a-thread-is-a-conversation.md)): an older
  replica cannot read the new event kind or an outbox `cancel` row that names its job, still answers a follow-up on a
  finished thread with 409, and writes `threads.job` back without `job.number` (the thread is job 1 again, and job
  2's ids repeat). Stop the old replicas, or upgrade in two steps (every replica to a build that only reads the new
  shapes, then the release that writes them), before any traffic reaches the new one.
- **Polling backs up every wakeup.** `LISTEN/NOTIFY` makes things prompt, but
  the dispatcher and the SSE streams also poll, so a lost notification only
  costs latency.
- **One agent per thread, no planner.** A thread delegates to the agent chosen
  at creation; the planner, reviewers and other inputs are later MVP steps (the verify/rework gate is built: the
  agent's own checks and a verifier agent).
- **A message on a finished thread starts its next job** (`done`, `failed` or `cancelled`: a new A2A task in the
  same context that names the previous task, [ADR 0020](../docs/decisions/0020-a-thread-is-a-conversation.md),
  [ADR 0021](../docs/decisions/0021-context-across-a2a-tasks.md)). Only an A2UI action on a card of a finished
  request is refused (409). The gate stays the thread's, fixed at creation.

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
