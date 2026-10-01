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
([`orch-surface-agui`](../../crates/surface-agui/README.md), [`orch-surface-mcp`](../../crates/surface-mcp/README.md)). Processes are stateless; the only
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
| `src/config.rs` | `Args` (clap derive: a flag per setting, falling back to its environment variable), `Config`, `McpSettings` (the tokens as `SecretString`s read through the `env` and `read` closures, the hosts and the public URL, when `mcp` is mounted on a role that serves HTTP), `Surface`, `LocalAgentKind` (the closed set of in-process agent kinds `transport: local` may name; `Echo` so far; compiled in with the feature `agent-local`; `needs_model()` says whether a kind calls a model), `LogFormat`, `ConfigError`. Clap only collects raw strings; `Config::load` validates them, reading the agent file and the `tokenEnv` variables through closures, so tests build `Args` by hand and never touch the process environment. Every problem names the variable, file or agent at fault, carries no secret and exits 78 |
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
| `ORCH_VERIFIER_WATCH_SECS` | `5` | at least 1 (`--verifier-watch-secs`); how often a verification looks at its thread while it waits for the verifier. When the thread has moved on (the deadline held it, it was cancelled, the user wrote) the verifier is told to stop and the row ends: this is how late |
| `INBOX_LEASE_SECS` | `30` | at least 3; how long a crashed replica's claim on an inbox row blocks others |
| `INBOX_POLL_SECS` | `2` | at least 1; the inbox worker's safety poll, and so the latest a timer fires after its time |
| `INBOX_PARKED_TTL_SECS` | `86400` | at least 1; how long a report that no thread watches yet waits before it expires |
| `INBOX_MAX_ATTEMPTS` | `10` | at least 1; claims of one inbox row before it is dead-lettered (a row claimed more often without being finished is dead-lettered undelivered; claims handed back at shutdown or ended by a park are not counted) |
| `AGENT_LOCAL_CONCURRENCY` | `4` | only with the feature `agent-local`: runs of local agents stepped at once (at least 1); the local agents' pool is this plus 4 connections |
| `SHUTDOWN_GRACE_SECS` | `15` | |
| `ORCH_SURFACES` | `agui` | comma-separated surfaces to mount (`--surfaces`), as far as the build has them: `agui`, `mcp`, `thread-tools`, `webhook-generic`, `webhook-github`; unknown, empty, repeated or not compiled in is a startup error, and so is the removed `chat-api` (see [Surfaces](#surfaces)) |
| `WEBHOOK_GENERIC_SECRETS` | none | one or two comma-separated shared secrets of `POST /webhooks/ci` (`--webhook-generic-secrets`; a signature by either is good, so a secret can be rotated). **Required when a role that serves routes (`all`, `control-plane`) mounts `webhook-generic`** (exit 78); a third secret, or one under 32 bytes, is a startup error; never logged, and hidden in `--help` |
| `WEBHOOK_GITHUB_SECRETS` | none | one or two comma-separated secrets of `POST /webhooks/github` (`--webhook-github-secrets`); **required when a role that serves routes mounts `webhook-github`** (exit 78), a third secret is a startup error; never logged, hidden in `--help` |
| `WEBHOOK_GENERIC_MAX_SKEW_SECS` | `300` | at least 1; how far `X-Vymalo-Timestamp` may be from the clock, either way (`--webhook-generic-max-skew-secs`) |
| `WEBHOOK_GITHUB_MAX_AGE_SECS` | `86400` | at least 1; how old the signed `completed_at` (`updated_at` for a workflow run) of a GitHub event may be; an older event is acknowledged (202) and not stored (`--webhook-github-max-age-secs`) |
| `ORCH_CI_REQUIRED` | none | comma-separated names of the CI checks that must pass, the deployment's `ci.required` (`--ci-required`); **a gate that requires `ci` must name at least one** (here or in an entry's `gate.ci.required`), else exit 78; `ci` is refused (78) when no CI webhook surface is mounted |
| `ORCH_CI_TIMEOUT_SECS` | `3600` | at least 1; how long a job waits for the CI reports its gate needs before it is blocked with `ci_timeout` (no attempt is used); an `AGENTS_FILE` entry's `gate.ci.timeoutSecs` overrides it (`--ci-timeout-secs`) |
| `ORCH_GATE` | none | sources every job must pass before it is `done`, a comma list of `ci`, `agent-checks`, `verifier` (`--gate`). Empty is no gate: an agent that completes is done. **`ci` and `agent-checks` are accepted by this build** (`ci` since slice 6; it needs reports, so mount `webhook-generic`); `verifier` is a startup error (78) naming the slice that enables it |
| `ORCH_GATE` | none | sources every job must pass before it is `done`, a comma list of `ci`, `agent-checks`, `verifier` (`--gate`). Empty is no gate: an agent that completes is done. **All three are accepted by this build** (`ci` since slice 6, it needs reports, so mount `webhook-generic`; `verifier` since slice 10). `verifier` needs a verifier agent (`ORCH_VERIFIER`, or `gate.verifier` in an entry) |
| `ORCH_MAX_ATTEMPTS` | `3` | attempts a gated job's agent gets, the first included (`--max-attempts`); at least 1 and at most the cap |
| `ORCH_MAX_ATTEMPTS_CAP` | `10` | the most an `AGENTS_FILE` entry or a run may set the attempts to (`--max-attempts-cap`); at most `100`. When only the cap is set below `3`, the default attempts are lowered to it; an explicit `ORCH_MAX_ATTEMPTS` above the cap is a startup error |
| `ORCH_VERIFIER` | none | the verifier agent's id (`--verifier`): another configured agent than the ones it verifies (startup error otherwise, naming the agent and how to fix it). Used when the gate requires `verifier` |
| `ORCH_VERIFIER_TIMEOUT_SECS` | `1800` | how long a verification may wait for the verifier's verdict before the thread waits for the user (`--verifier-timeout-secs`); at least 1. Waiting does not use an attempt |
| `THREAD_TOOLS_SECRET` | none | the HMAC key of the thread-tools tokens, at least 32 bytes (`--thread-tools-secret`; `openssl rand -hex 32`). With `THREAD_TOOLS_URL` it makes the A2A adapter give a grant to every agent whose card lists `thread-tools/v1`, and it opens the surface `thread-tools`. **Both or neither** (a half is exit 78), and **in every role** when `thread-tools` is in `ORCH_SURFACES`: a worker mounts no route, but it mints; a process that does not name the surface still reads and checks them when they are set. A `SecretString`, never logged, hidden in `--help` |
| `THREAD_TOOLS_SECRET_PREVIOUS` | none | the previous key, verifying only (`--thread-tools-secret-previous`): for a rotation, move the current key here and set a new one, then remove it once the longest token lifetime has passed. At least 32 bytes, not the current key, and not without a current one (78) |
| `THREAD_TOOLS_URL` | none | the base URL under which agents reach this orchestrator (`--thread-tools-url`), `http` or `https`, a host, optionally a port and a path prefix, no credentials, query or fragment; the grant tells an agent `<URL>/thread-tools/<threadId>/mcp` |
| `THREAD_TOOLS_TOKEN_TTL_SECS` | `7200` | how long a token lives (`--thread-tools-token-ttl-secs`), 60 to 86400 |
| `THREAD_TOOLS_ALLOWED_HOSTS` | the host of the URL | comma-separated `Host` values the surface accepts (`--thread-tools-allowed-hosts`), each `host` or `host:port` (a URL, `*` or a bad port is 78); the default is the host of `THREAD_TOOLS_URL`, with its port when the URL names one |
| `MCP_TOKENS_FILE` | required with `mcp` | YAML list of `{user, tokenEnv}` (`--mcp-tokens-file`): who each bearer token is; read by the roles that serve HTTP only. A `tokenEnv` variable that is unset or empty is `McpTokenEnvMissing` and an unreadable file `McpTokensFileRead` (both 78) |
| `MCP_TOKEN_<NAME>` | required by the file | the variable a `tokenEnv` names: the bearer token (a `SecretString`, never logged), at least 32 bytes (shorter is `Invalid`, 78) |
| `MCP_ALLOWED_HOSTS` | required with `mcp` | comma-separated `Host` values the MCP server accepts (`--mcp-allowed-hosts`), each `host` or `host:port`: a URL, `*` or a port that is not a number is `Invalid` (78) |
| `MCP_WAIT_MAX_SECS` | `3600` | the largest `timeout_secs` of `wait_for_job` (`--mcp-wait-max-secs`), 1 to 86400, larger requests are cut to it |
| `MCP_WAIT_MAX_CONCURRENT`, `MCP_WAIT_MAX_PER_USER` | `256`, `16` | the most `wait_for_job` calls one process, and one user, may hold open (`--mcp-wait-max-concurrent`, `--mcp-wait-max-per-user`), at least 1 |
| `MCP_ALLOWED_ORIGINS` | none | comma-separated browser origins the MCP server lets through (`--mcp-allowed-origins`), each `http(s)://host[:port]`; a request with another `Origin` is 403 |
| `ORCH_PUBLIC_URL` | unset | the chat's public origin (`--public-url`), for the `web_url` of `start_job`; an origin with no path |
| `ORCH_INSTANCE_ID` | `$HOSTNAME-<uuid>` | names this replica in leases |
| `RUST_LOG`, `LOG_FORMAT` | `info,rmcp=warn`, `json` | `LOG_FORMAT=text` for humans; `RUST_LOG` replaces the default whole (the MCP library logs a line per request at `info`) |

### The verification gate

[ADR 0018](../../../docs/decisions/0018-verification-gate-and-rework-loop.md). Three layers (sources can only be added, attempts set anywhere
within the cap): the deployment (`ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_MAX_ATTEMPTS_CAP`, `ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`), an agent's
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
- id: builder
  name: Builder
  cardUrl: https://builder.example.com/.well-known/agent-card.json
  gate:
    require: [verifier]       # another agent reviews the pushed commit; its `verdict` artifact decides
    verifier: reviewer        # a configured agent (below), never this one
- id: reviewer
  name: Reviewer
  cardUrl: https://reviewer.example.com/.well-known/agent-card.json
  tokenEnv: REVIEWER_A2A_TOKEN   # its own credentials: it never receives the worker's
```

Startup validates all of it and exits **78** with a message naming the variable or the agent: an unknown source; a
`maxAttempts` outside `1..=cap`; an entry whose `require` leaves out a source the deployment requires; a `verifier`
that is not another configured agent; a gate that requires `verifier` with no verifier configured; an agent whose own
gate requires the verifier and that is the verifier itself (its entry must leave `verifier` out of its `require`, the one
removal a layer may make); an unknown member of `gate`. `ci` is honoured since slice 6 (the inbox and timers, slice 5,
and the CI webhook write and apply its reports): a deployment or an entry may require it and set `ci: {required, timeoutSecs}`; a run may add `ci` to `require`
but not set the `ci` settings. **A gate that requires `ci` names the checks that count** (`ci.required`, `ORCH_CI_REQUIRED` for the deployment): the process
exits 78 for a deployment or an entry that requires `ci` without a name, and a run that adds `ci` on a policy with none is a 400.
A process that serves routes and mounts neither `webhook-generic` nor `webhook-github` refuses `ci` in every layer (78 at
startup, 400 for a run) with "no CI webhook surface is mounted (ORCH_SURFACES)"; a `worker` serves no routes and cannot tell, so it
honours what the control plane decides. A source this build cannot honour would be refused in every layer, naming the slice
that enables it (none is left). A run that asks for the same is a 400.

**The verifier** (slice 10): when the worker completes under a gate that requires it, the dispatcher asks the verifier agent
over A2A, in a context of its own (`<thread>-verify-<attempt>-<verification>`), to review the commit the worker pushed, and
its `verdict` artifact `{passed, findings[]}` decides like any other source: findings send the worker back (quoted as
untrusted data), a pass counts toward done. No verdict is a failed check. A verifier that cannot be used (its task fails, it
cannot be reached, it does not answer within `ORCH_VERIFIER_TIMEOUT_SECS`) leaves the thread waiting for the user, `blocked`,
without spending an attempt. A thread may require the verifier in its run but not choose which agent it is.

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
| `surface-mcp` | yes | [`orch-surface-mcp`](../../crates/surface-mcp/README.md), the surface name `mcp` ([ADR 0019](../../../docs/decisions/0019-mcp-server-over-streamable-http.md)). On by default like `surface-agui`, so the image has it; it is mounted only when `ORCH_SURFACES` names it |
| `surface-thread-tools` | yes | [`orch-surface-thread-tools`](../../crates/surface-thread-tools/README.md) (the surface name `thread-tools`) and [`orch-thread-token`](../../crates/thread-token/README.md) ([`docs/api/thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md)). On by default; mounted only when `ORCH_SURFACES` names it, and then `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL` are required in every role. Without the feature those variables are not read and naming the surface is refused |
| `surface-webhook` | yes | [`orch-surface-webhook`](../../crates/surface-webhook/README.md), the surface names `webhook-generic` (`POST /webhooks/ci`) and `webhook-github` (`POST /webhooks/github`) |
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
`GET /api/threads/{id}/export`, `POST /api/threads/{id}/cancel`) and health are `orch-api`'s and are mounted whatever
`ORCH_SURFACES` says. The interaction surfaces are chosen by it:

| `ORCH_SURFACES` | Serves |
|---|---|
| unset, or `agui` (the default) | the AG-UI routes: `POST /agui/agents/{agentId}`, `GET /agui/threads/{threadId}/connect`, `GET /agui/agents/{agentId}/capabilities` |
| `agui,mcp` | and the MCP server at `/mcp`: a **machine route** outside the identity layer, guarded by `Authorization: Bearer <token>`; needs `MCP_TOKENS_FILE` and `MCP_ALLOWED_HOSTS` (a missing piece is exit 78 before anything connects) |
| `agui,thread-tools` (with `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL`) | and the per-thread MCP endpoint `/thread-tools/{threadId}/mcp` ([`docs/api/thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md)): a **machine route** outside the identity layer, guarded by the HMAC token the A2A adapter mints for an agent that lists `thread-tools/v1` (a bad, expired or foreign token is one `401`; no route is served for a thread that does not exist or is another agent's); the tool is `get_ui_catalog`. It is not under `/mcp`, and the edge should not route it from the public side: agents call the orchestrator directly |
| `webhook-generic` (with `WEBHOOK_GENERIC_SECRETS`) | `POST /webhooks/ci`: a signed CI report, a **machine route**: no user identity (it never reads `X-Auth-Request-Email`), guarded by an HMAC-SHA-256 over `"<timestamp>.<body>"`. The edge must pass `/webhooks/*` to the orchestrator without injecting an identity ([`api/webhooks.md`](../../../docs/api/webhooks.md)) |
| `webhook-github` (with `WEBHOOK_GITHUB_SECRETS`) | `POST /webhooks/github`: GitHub's `check_run` and `workflow_run` deliveries (`completed` only; `ping` is 204, every other event is 202 and ignored), a **machine route** guarded by `X-Hub-Signature-256` over the raw body |

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

* Unit tests in `src/config.rs`: no database, no environment (the gate: the defaults, the variables, `ci`
  accepted and `ORCH_CI_TIMEOUT_SECS` read (and overridden by a target's `ci.timeoutSecs`), the webhook: mounted by name with its secrets (both webhooks), a mounted route without secrets refused (a worker need not have them), the count, the 32-byte minimum, the skew and the redaction of the secrets, its feature off; a `ci` gate with no names or no webhook surface refused, and honoured beside a webhook or in a worker; the verifier read from `ORCH_GATE`, `ORCH_VERIFIER` and
  `ORCH_VERIFIER_TIMEOUT_SECS` (a missing or unknown verifier, a self-verifying one, a bad timeout refused), strict parsing of `gate:`, a
  target that weakens the deployment or exceeds the cap; defaults, the
  environment/flag mapping, unknown, empty and repeated surfaces, the removed
  `chat-api` refused with an error naming it and AG-UI (`RemovedSurface`, from the variable and from the
  flag), `--help` naming every variable, the role: default `all`, each
  value, blank, unknown, the flag collected raw). `src/main.rs` maps every
  `HostError` to exit 70. `src/logging.rs`: role and instance first on every JSON and text
  line (an instance with a quote stays valid JSON, no fields means the stock line).
* Unit tests of the thread-tools settings in `src/config.rs`: the key, the URL, the lifetime and the hosts read (the default host from the URL and its port, a rotation, a lifetime from 60 to 86400), read by every role and without the surface when set, refused when the surface is named without them, when only one of the key and the URL is set, when a key is short, repeated as the previous one or the previous one has no current key, and for a URL, lifetime or host that is not one; nothing prints a key, in `Debug` or in an error; a build without the feature refuses the surface.
* Unit tests of the MCP settings in `src/config.rs`: tokens, hosts, public URL and wait bound read and normalised (user lower-cased, token trimmed, hosts split and required to be authorities, origins, the 32-byte token minimum, the wait limits; `MCP_WAIT_MAX_SECS` 1 to 86400), nothing read unless `mcp` is mounted (and not by a `worker`), every missing piece named (`Missing`, `McpTokenEnvMissing`, `McpTokensFileRead`, `Invalid` for a bad file, host list or URL), a rotation allowed and a shared token refused, no token in `Debug`; `src/main.rs` maps the new errors to exit 78.
* `tests/local.rs` (`#![cfg(feature = "agent-local")]`, run with `--features agent-local`): the executable hosting
  a local `echo` agent answers an AG-UI run and the journal holds the run (`orch_agent_runs`); a `control-plane`
  process with a local agent starts, accepts a run and leaves it `queued` with an empty journal until a `worker`
  process starts and completes it. Unit tests in `src/config.rs` cover the flavours: without the feature a local agent is
  refused naming `agent-local`, with it it is accepted, and `AGENT_LOCAL_CONCURRENCY` defaults to 4.
* `tests/smoke.rs`: the built executable as a process. Without the feature, `transport: local` exits 78 naming `agent-local`. Configuration-error
  tests always run (including a gate this build cannot run (the verifier with no agent to ask), in the environment and in `AGENTS_FILE`: exit 78; the unreachable-database one waits out sqlx's 30 s
  connect timeout). The CLI tests spawn the executable: `--help`, each variable
  read from the environment alone, a flag over its variable, a usage error, the removed
  `chat-api` (`the_removed_chat_api_surface_is_a_config_error_pointing_to_agui`: from the variable,
  beside `agui`, from the flag, and the flag over a valid variable: exit 78, the log says it was
  removed on 2026-09-30 and points to AG-UI, nothing connects first), and the default serving `agui`
  and the resource API only (the AG-UI route answers 400 to `{}` and 401 without identity; the
  four removed legacy routes answer 404, or 405 for `POST /api/threads`, while the thread list, an
  unknown thread and cancel answer as resources). With a database: a gate that requires the verifier (`ORCH_GATE`,
  `ORCH_VERIFIER`, `ORCH_VERIFIER_TIMEOUT_SECS`) through two fake agents, the verifier finding something once
  (one run, the verifier a subagent of its own, two verifications in contexts of their own, its own credentials);
  `/healthz`, `/readyz`, 401 without
  identity, a thread run and completed over AG-UI (the default surface) through a fake agent with the bearer from
  `tokenEnv`, JSON logs, a clean exit on SIGTERM, and two processes on one
  database with a SIGKILL mid-task. The MCP surface (`mcp_without_its_tokens_or_hosts_is_a_config_error`: each missing piece is
  exit 78 naming it, before anything connects, with no token in the log; `mcp_is_mounted_by_its_name_and_a_token_lists_the_tools`:
  the process mounted with `ORCH_SURFACES=agui,mcp` answers 401 with the challenge without or with a wrong token, `tools/list` (six tools) and
  `list_agents` with the token, 403 for a `Host` that is not listed; `mcp_is_not_there_unless_it_is_named`: 404). The thread-tools surface (`the_thread_tools_surface_fails_closed_and_never_prints_its_key`: named with no key, with a key and no URL, with a short key or a bad URL, in `all` and in a `worker`, is exit 78 naming the variable before anything connects and never prints the key; `the_thread_tools_endpoint_is_mounted_by_its_name_and_a_minted_token_opens_it`: the process mounted with `ORCH_SURFACES=agui,thread-tools` refuses no token, a token that is not one, a token for a thread nobody created and a token for another agent with the same `401`, and a token minted with the configured key for the thread a run created lists `get_ui_catalog`, with the key never in the log; `thread_tools_is_not_there_unless_it_is_named`: not mounted, with the key and URL set). The roles: `--role worker` (over a
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
