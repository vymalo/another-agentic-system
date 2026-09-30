# orch-surface-mcp

The MCP server surface. An MCP client (Claude Code, opencode, any client that speaks
[streamable HTTP](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)) gives the
orchestrator a job and follows it with six tools at `/mcp`: `list_agents`, `start_job`, `get_job`, `wait_for_job`,
`answer` and `cancel_job`. The decision, with its diagrams, is
[ADR 0019](../../../docs/decisions/0019-mcp-server-over-streamable-http.md).

## Where it sits

An adapter over [`orch-app`](../app/README.md)'s `App`, like every inbound surface
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)). It depends on `App`,
[`orch-core`](../core/README.md), `orch-ports` (the `Ports` bound) and [`orch-api`](../api/README.md) (the shared
`SurfaceRoutes` and problems), and on [`rmcp`](https://crates.io/crates/rmcp) 3.5 for the protocol. It calls `App`
directly (`create_thread_as`, `submit`, `cancel`, `get_thread`, `list_events`, `event_stream`), **not** through the inbox: the caller
is authenticated and waits for the answer. It is a Cargo feature of the binary ([`orchestrator`](../../bin/orchestrator/README.md),
feature `surface-mcp`, on by default) and is mounted by `ORCH_SURFACES` (name `mcp`).

## API at a glance

| Item | What |
|---|---|
| `routes::<P>(Arc<App<P>>, McpConfig) -> orch_api::SurfaceRoutes` | `/mcp` as a **machine route**: outside the identity layer and the request timeout, guarded by the bearer check |
| `McpConfig::new(TokenTable, allowed_hosts)`, `.with_public_url(origin)`, `.with_wait_max(d)`, `.with_heartbeat(d)` | the tokens, the `Host` values the server accepts (never empty: an empty list would allow every host), the chat's public origin for `web_url`, the largest `timeout_secs` of `wait_for_job` (default 3600 s) and its heartbeat interval (default 60 s; tests shorten it). `ConfigError` says what is wrong |
| `TokenTable::new([(UserId, SecretString)])`, `.authenticate(&str)` | bearer tokens and their users; tokens are hashed at once (SHA-256) and compared with `subtle` in constant time against every entry; `TokenError` for none, an empty one, or one shared by two users. `Debug` shows users, never tokens |
| `McpUser` | the identity the guard puts in the request; the only identity a tool has (`X-Auth-Request-Email` is never read) |
| `ToolName` | the closed set of tools, their names, schemas and definitions |
| `StartJobArgs`, `GetJobArgs`, `WaitForJobArgs`, `AnswerArgs`, `CancelJobArgs`, `NoArgs` | the arguments of the tools; unknown members are refused |
| `wait::wait_for_job(app, user, id, &WaitRequest, &impl ProgressSink, &CancellationToken) -> Waited` | the wait loop behind `wait_for_job`, apart from the protocol: reads `App::event_stream`, reports through a `ProgressSink`, ends as a `WaitEnd` (`Finished`, `Blocked`, `TimedOut`, `Interrupted`) and says `resume_after_seq`. It keeps no state, so tests drive it on a paused clock |
| `job_id_for(&UserId, client_request_id) -> ThreadId` | the deterministic job id of a `start_job` with a `client_request_id` |
| `JobSummary` (and `Branch`, `PullRequest`, `LastCheck`, `Findings`) | what `get_job` says |
| `MCP_PATH` | `/mcp` |

### Authentication

`Authorization: Bearer <token>`. No header, another scheme, or a token nobody has is `401` with
`WWW-Authenticate: Bearer` (an RFC 9457 problem body that does not say which), before any tool runs and with nothing
written. The token's user owns the jobs the caller starts and sees, exactly as an edge identity does. The server also
checks `Host` against the list it was given (DNS rebinding; `403`).

### The tools

| Tool | Arguments | Result (structured content, also as text) |
|---|---|---|
| `list_agents` | none | `{agents: [{id, name, description}]}`, in configuration order; the first is the default |
| `start_job` | `text`, `agent?`, `title?`, `client_request_id?` | `{job_id, state, created, web_url?}` at once. With a `client_request_id`, a retry (on any replica, even at the same time) returns the same `job_id` with `created: false`; without one, every call starts a job |
| `get_job` | `job_id` | `{job_id, title, agent, state, finished, hold?, attempt?, max_attempts?, branch?, pull_request?, ci?, findings?, last_seq, created_at, updated_at}` |
| `answer` | `job_id`, `text` | `{job_id, state}`. A message to a finished job is refused, as in the chat |
| `wait_for_job` | `job_id`, `after_seq?`, `timeout_secs` | the `get_job` summary plus `outcome` (`finished`, `blocked`, `timed_out`, `interrupted`) and `resume_after_seq`; see below |
| `cancel_job` | `job_id` | `{job_id, state, finished}`. Cancelling a finished job changes nothing |

A job that is not the caller's, that does not exist, or whose id is not an id is the same tool error, `no such job`
(never "forbidden"). Failures the caller can act on are tool results with `isError: true`; a malformed call (unknown
tool, arguments that do not fit, including `gate`, which is not offered yet) and a fault of the server are protocol
errors.

### `wait_for_job`

Follows a job through the event log, with no state in the process: it reads `App::event_stream` from a cursor and reports
through `notifications/progress` when the request carries a `progressToken`.

* **Cursor.** `after_seq` omitted: only what happens from now on (a job that is already finished or blocked answers at
  once). `0`: the whole log. A value past the end is the end. `resume_after_seq` in every answer is the last event the
  call read: call again with it as `after_seq` and nothing is lost or repeated, on any replica.
* **Progress.** One notification per event (`#3 artifact: Pull request`; partial agent messages are skipped) and a
  heartbeat every 60 s (`still waiting (job working, last event #7)`), with an integer `progress` counter that starts at
  1 and only increases. Agent text in a message is untrusted and cut to one line of 200 characters.
* **End.** `finished` (`done`, `failed`, `cancelled`) or `blocked` (waiting for an `answer`) when the event that says so is
  the last of the log; `timed_out` after `timeout_secs` (0 to `MCP_WAIT_MAX_SECS`; a larger value is cut to the bound);
  `interrupted` when the process is shutting down or the client is gone. Events that were already in the log when the call
  began are always reported, even with a timeout of 0.

### Stateless

`StreamableHttpService` runs with `legacy_session_mode = false` and a `NeverSessionManager`: a fresh handler per
request, no `Mcp-Session-Id`, no state in the process, so any replica serves any request. Both handshakes work with
rmcp's own client: the `initialize` handshake of the older protocol versions and the `server/discover` lifecycle of
`2026-07-28`. *Unverified:* Claude Code and opencode against it (not tried).

## Features and environment

No Cargo features. The crate reads no environment: the binary reads these and builds an `McpConfig` from them.

| Variable | Meaning |
|---|---|
| `ORCH_SURFACES` | include `mcp` to mount this crate |
| `MCP_TOKENS_FILE` | YAML list of `{user, tokenEnv}`; required with `mcp` on a role that serves HTTP |
| `MCP_TOKEN_<NAME>` | whatever `tokenEnv` names; the bearer token, secret; unset or empty is a startup error |
| `MCP_ALLOWED_HOSTS` | the `Host` values accepted, comma separated; required with `mcp` |
| `ORCH_PUBLIC_URL` | the chat's public origin; without it `start_job` has no `web_url` |
| `MCP_WAIT_MAX_SECS` | the largest `timeout_secs` of `wait_for_job`; 1 to 86400, default 3600 |

## Tests

| File | Covers |
|---|---|
| `src/*` (unit) | the token table (users, rotation, constant-time lookup, the refusals, no token in `Debug`), the bearer header parsing, the job id derivation (stable, per user, well-formed), the tool set and schemas, the pull request recognition, the summary |
| `tests/auth.rs` | over real HTTP: 401 with `WWW-Authenticate: Bearer` for no header, another scheme and an unknown token, with nothing written and no agent called; the identity header is never an identity (also with `AUTH_DEV_USER` set); `Host` not allowed is 403; the rest of the service still needs the identity; no session id; `GET` is 405 |
| `tests/wait.rs` | the wait loop with no network, on a paused clock (nothing sleeps to synchronise; a test waits on the sink): progress counts up one per event and the last event returns the job; a timeout says where to resume and the next call loses nothing; a timeout of 0 still reports the log; the heartbeat shares the counter; a finished job returns at once and `after_seq: 0` replays it; a blocked job returns and the answer lets the next wait finish; a shutdown ends the call and another replica continues; a client that goes away ends it; another user's job is not found |
| `tests/wait_http.rs` | `wait_for_job` over HTTP with an rmcp client that records the notifications: increasing integer counters and one notification per event, then the finished job; a timeout of 0 and a resume that together report every event once; a blocked job and its answer; a heartbeat while the agent is held; `timeout_secs` cut to the bound; refusals |
| `tests/tools.rs` | an in-process rmcp client on the in-memory stack: `list_agents`; `start_job` (default agent, title, `origin: mcp` in the log, refusals before any write); an idempotent `start_job` (sequential, concurrent, per user); `web_url`; `get_job` to `done` with the pull request; another user's job is "no such job" for every tool; `answer` on a blocked job and on a finished one; `cancel_job` on a running and on a finished job; the `2026-07-28` handshake |

The end-to-end tests over the real dispatcher and the A2A adapter, on the in-memory store and on Postgres, and across
two replicas (a wait that follows a job another replica delivers; a replica killed mid-wait and the call repeated on the
other), are `mcp.rs` in [`orch-e2e`](../e2e/README.md).

| Variable | Meaning |
|---|---|
| none | the tests here run against the in-memory stack and need no database |
