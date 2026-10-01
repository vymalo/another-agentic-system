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
| `McpConfig::new(TokenTable, allowed_hosts)`, `.with_public_url(origin)`, `.with_allowed_origins(list)`, `.with_wait_max(d)`, `.with_heartbeat(d)`, `.with_wait_limits(per_process, per_user)`, `.with_tool_timeout(d)` | the tokens, the `Host` values the server accepts (never empty, and each a `host` or `host:port`: an empty list would allow every host), the chat's public origin for `web_url`, the browser origins let through (default none), the largest `timeout_secs` of `wait_for_job` (default 3600 s) and its heartbeat interval (default 60 s; tests shorten it), how many waits are open at once (256 per process, 16 per user), how long any other tool may take (30 s). `ConfigError` says what is wrong |
| `TokenTable::new([(UserId, SecretString)])`, `.authenticate(&str)`, `MIN_TOKEN_BYTES` (32) | bearer tokens and their users; tokens are hashed at once (SHA-256) and compared with `subtle` in constant time against every entry; `TokenError` for none, an empty one, one shorter than `MIN_TOKEN_BYTES`, or one shared by two users. `Debug` shows users, never tokens |
| `WaitSlots::new(per_process, per_user)`, `.acquire(&UserId) -> Result<WaitPermit, Busy>` | the caps on open `wait_for_job` calls; a permit is given back when dropped, however the call ends |
| `McpUser` | the identity the guard puts in the request; the only identity a tool has (`X-Auth-Request-Email` is never read) |
| `ToolName` | the closed set of tools, their names, schemas and definitions |
| `StartJobArgs`, `GateArgs`, `GetJobArgs`, `WaitForJobArgs`, `AnswerArgs`, `CancelJobArgs`, `NoArgs` | the arguments of the tools; unknown members are refused |
| `wait::wait_for_job(app, user, id, &WaitRequest, &impl ProgressSink, &CancellationToken) -> Waited` | the wait loop behind `wait_for_job`, apart from the protocol: reads `App::event_stream`, reports through a `ProgressSink`, ends as a `WaitEnd` (`Finished`, `Blocked`, `TimedOut`, `Interrupted`) and says `resume_after_seq`. It keeps no state, so tests drive it on a paused clock |
| `job_id_for(&UserId, client_request_id) -> ThreadId` | the deterministic job id of a `start_job` with a `client_request_id`: UUID version 8, which no other surface may create (`ThreadId::is_derived`) |
| `JobSummary` (and `Branch`, `PullRequest`, `LastCheck`, `Findings`) | what `get_job` says |
| `MCP_PATH` | `/mcp` |
| `is_host_authority(&str)` | whether a string is a `Host` value; now defined in [`orch-api`](../api/README.md), which the thread-tools surface shares, and re-exported here under its old name |

### Authentication

`Authorization: Bearer <token>`. No header, another scheme, or a token nobody has is `401` with
`WWW-Authenticate: Bearer` (an RFC 9457 problem body that does not say which), before any tool runs and with nothing
written. **More than one `Authorization` header is `401`** whatever they carry (which one is the caller is not for an
intermediary to decide). The token's user owns the jobs the caller starts and sees, exactly as an edge identity does. The
server also checks `Host` against the list it was given (DNS rebinding; `403`) and, since a browser always sends
`Origin`, **refuses any request that carries an `Origin` not in `MCP_ALLOWED_ORIGINS`** (`403`; none by default).
Clients that are not browsers send none and do not notice.

Tokens must be at least 32 bytes (generate them with `openssl rand -hex 32`).

Behind [oauth2-proxy](https://oauth2-proxy.github.io/oauth2-proxy/) the route has to bypass the session check: list it in
`skip_auth_routes` (for example `^/mcp`), and keep `skip_jwt_bearer_tokens` off for it so that the proxy does not try to
read the bearer token as its own. *Unverified:* no oauth2-proxy was run against this; check both settings, and that the
`Authorization` header arrives unchanged, before relying on it.

### The tools

| Tool | Arguments | Result (structured content, also as text) |
|---|---|---|
| `list_agents` | none | `{agents: [{id, name, description}]}`, in display order, the deployment's own agents first; the first is the default. Read from the agent registry now ([ADR 0022](../../../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md)), so an agent the platform added is there without a restart, and a registry that cannot be read leaves only the others, which the result says: `unavailable_sources: [name]` (only when a source could not be read, so a client that does not know the key sees what it always saw) |
| `start_job` | `text`, `agent?`, `title?`, `client_request_id?`, `gate?` | `{job_id, state, created, web_url?}` at once. With a `client_request_id`, a retry (on any replica, even at the same time) returns the same `job_id` with `created: false`; a retry that says something else (another text, agent, title or gate) is refused, naming what differs; without a `client_request_id`, every call starts a job. `gate` is `{require?, maxAttempts?}`, the per-job layer of the verification gate: one the deployment cannot honour (`ci` is not built in this version; `verifier` needs a verifier agent other than the job's own) is a tool error naming why, and so is a `client_request_id` whose job id another user's thread already has |
| `get_job` | `job_id` | `{job_id, title, agent, job, state, finished, hold?, attempt?, max_attempts?, branch?, pull_request?, ci?, findings?, last_seq, created_at, updated_at}` |
| `answer` | `job_id`, `text` | `{job_id, state, job}`. A message to a finished job (`done`, `failed`, `cancelled`) starts the thread's next job on the same agent, under the same `job_id`; `job` is its number ([ADR 0020](../../../docs/decisions/0020-a-thread-is-a-conversation.md)). The summary's `job` says which job of the thread the rest is about, and `pull_request` only looks at what came after the last `job_started` |
| `wait_for_job` | `job_id`, `after_seq?`, `timeout_secs` | the `get_job` summary plus `outcome` (`finished`, `blocked`, `timed_out`, `interrupted`) and `resume_after_seq`; see below |
| `cancel_job` | `job_id` | `{job_id, state, finished}`. Cancelling a finished job changes nothing |

A job that is not the caller's, that does not exist, or whose id is not an id is the same tool error, `no such job`
(never "forbidden"), for `get_job`, `wait_for_job`, `answer` and `cancel_job` alike (`tests/tools.rs` runs the same
foreign and unknown ids through each of them and compares the answers); `start_job` has no job id to refuse, and a
derived id that is taken is its own error (above). Failures the caller can act on are tool results with `isError: true`;
a malformed call (unknown tool, arguments that do not fit, a `gate` of the wrong shape) and a fault of the server are
protocol errors. Every tool but `wait_for_job` is cut off after 30 s (a tool error). Text that came from an agent or the
log (titles, pull request fields, findings) is cut to a size in the results, and only an `https` pull request URL is
reported.

### `wait_for_job`

Follows a job through the event log, with no state in the process: it reads `App::event_stream` from a cursor and reports
through `notifications/progress` when the request carries a `progressToken`.

* **Cursor.** `after_seq` omitted: only what happens from now on (a job that is already finished or blocked answers at
  once). `0`: the whole log. A value past the end is the end. `resume_after_seq` in every answer is the last event the
  call read: call again with it as `after_seq` and nothing is lost or repeated, on any replica.
* **Progress.** One notification per event (`#3 artifact: Pull request`; `#9 forked from thread <id> at #4`; partial agent messages are skipped, and so is the `ui_catalog` event, which is bookkeeping about the person's screen, and the updates of an `agent_step`: a step's start and end are one line each, `step: <label> [<state>]`; and a `thread_titled`, a label of the conversation and not progress of the job) and a
  heartbeat every 60 s (`still waiting (job working, last event #7)`), with an integer `progress` counter that starts at
  1 and only increases. Agent text in a message is untrusted and cut to one line of 200 characters.
* **End.** `finished` (`done`, `failed`, `cancelled`) or `blocked` (waiting for an `answer`) when the event that says so is
  the last of the log; `timed_out` after `timeout_secs` (0 to `MCP_WAIT_MAX_SECS`; a larger value is cut to the bound);
  `interrupted` when the process is shutting down (the answer then also has `retry_after_secs`, 2) or the client is
  gone. Events that were already in the log when the call began are always reported, even with a timeout of 0.
* **No `progressToken`: short.** A call that sent none gets nothing while it waits, and an intermediary may take the
  silence for a dead connection, so it returns `timed_out` with `resume_after_seq` after at most one heartbeat interval
  (60 s), however large `timeout_secs` is. To wait longer, send a token, or call again with the cursor.
* **Caps.** At most `MCP_WAIT_MAX_CONCURRENT` waits per process (256) and `MCP_WAIT_MAX_PER_USER` per user (16); a call
  over either is a tool error starting `too many waits`. A slot is given back when the call ends, also when the client
  hangs up.

### Stateless

`StreamableHttpService` runs with `legacy_session_mode = false` and a `NeverSessionManager`: a fresh handler per
request, no `Mcp-Session-Id`, no state in the process, so any replica serves any request. Both handshakes work with
rmcp's own client: the `initialize` handshake of the older protocol versions and the `server/discover` lifecycle of
`2026-07-28`. *Unverified:* Claude Code and opencode against it (not tried).

### Response framing

A call is answered with **one `application/json` response** (rmcp's `json_response`), sent once the tool has answered,
the answer in the same write as the head. Only `wait_for_job` with a `progressToken` is a `text/event-stream`, and its
head goes out with the first notification. The head is never sent before the tool has said anything, because a Go
reverse proxy (Caddy; oauth2-proxy is *unverified*) can drop the upstream connection right after writing the head to its client: when
the server answers before the proxy has finished reading the request body it forwarded, the proxy's own server closes
that body and its transport takes the failed read for a failed request. Whatever came after the head is lost; with an
event stream opened before the answer, that was the answer (coder-e2e's `FAIL start_job: {}`, with rmcp logging "failed
to send pending response during drain"). Reproduced on 2026-09-30 through Caddy 2.11.4 and Go 1.24's
`httputil.ReverseProxy` over a veth pair under CPU load. An answer larger than the proxy's first read (about 4 KiB,
`tools/list` is about 6 KiB) can still be cut, so the dev edge also buffers request bodies (`dev/Caddyfile`), which stops
the proxy from dropping the connection at all.

## Features and environment

No Cargo features. The crate reads no environment: the binary reads these and builds an `McpConfig` from them.

| Variable | Meaning |
|---|---|
| `ORCH_SURFACES` | include `mcp` to mount this crate |
| `MCP_TOKENS_FILE` | YAML list of `{user, tokenEnv}`; required with `mcp` on a role that serves HTTP |
| `MCP_TOKEN_<NAME>` | whatever `tokenEnv` names; the bearer token, secret, at least 32 bytes; unset, empty or shorter is a startup error |
| `MCP_ALLOWED_HOSTS` | the `Host` values accepted, comma separated, each `host` or `host:port` (a URL or `*` is a startup error); required with `mcp` |
| `MCP_ALLOWED_ORIGINS` | browser origins let through (`https://app.example.com`, comma separated); default none |
| `ORCH_PUBLIC_URL` | the chat's public origin; without it `start_job` has no `web_url` |
| `MCP_WAIT_MAX_SECS` | the largest `timeout_secs` of `wait_for_job`; 1 to 86400, default 3600 |
| `MCP_WAIT_MAX_CONCURRENT`, `MCP_WAIT_MAX_PER_USER` | open waits per process (default 256) and per user (16); at least 1 |

## Tests

| File | Covers |
|---|---|
| `src/*` (unit) | the token table (users, rotation, constant-time lookup, the refusals, no token in `Debug`), the bearer header parsing (several `Authorization` headers are none), the wait slots and their caps, the host and origin syntax, the capped wait time of a call without a token, the job id derivation (stable, per user, well-formed), the tool set and schemas, the summary (the pull request recognition itself is `orch_core::pull_request_url`, tested in `orch-core`) |
| `tests/auth.rs` | over real HTTP: 401 with `WWW-Authenticate: Bearer` for no header, another scheme and an unknown token, with nothing written and no agent called; two `Authorization` headers are 401; a browser `Origin` is 403 unless listed and a request without one is not affected; the identity header is never an identity (also with `AUTH_DEV_USER` set); `Host` not allowed is 403; the rest of the service still needs the identity; no session id; `GET` is 405 |
| `tests/wait.rs` | the wait loop with no network, on a paused clock (nothing sleeps to synchronise; a test waits on the sink): progress counts up one per event and the last event returns the job; a timeout says where to resume and the next call loses nothing; a timeout of 0 still reports the log; the heartbeat shares the counter; a finished job returns at once and `after_seq: 0` replays it; a blocked job returns and the answer lets the next wait finish; a shutdown ends the call and another replica continues; a client that goes away ends it; another user's job is not found |
| `tests/wait_http.rs` | `wait_for_job` over HTTP with an rmcp client that records the notifications: increasing integer counters and one notification per event, then the finished job; a timeout of 0 and a resume that together report every event once; a blocked job and its answer; a heartbeat while the agent is held; `timeout_secs` cut to the bound; refusals |
| `tests/wait_limits.rs` | the per-user and per-process caps (and that another user's share is their own), a client that hangs up with and without a progress token gives its slot back, a wait without a token returns within a heartbeat with where to resume (and with a token it stays open), a replica that is shutting down says `interrupted` with `retry_after_secs` |
| `tests/framing.rs` | over raw TCP and HTTP: a client that drops the connection as soon as it has the response head (as a Go reverse proxy can) still has the whole answer, for a tool that takes a while (a held `wait_for_job`) and for `start_job`; a tool answer, `tools/list` included, is one `application/json` response with a `Content-Length`; `wait_for_job` with a progress token is still an event stream with its notifications |
| `tests/tools.rs` | an in-process rmcp client on the in-memory stack: `list_agents` (also `list_agents_reads_the_registry_now_and_says_when_a_source_could_not_be_read`: an agent the registry adds is there at once after the deployment's own, one that is down leaves only those and the result says `unavailable_sources`, the default agent of `start_job` does not change, and naming an agent only the registry could list is "temporarily unavailable" while it is down); `start_job` (default agent, title, `origin: mcp` in the log, refusals before any write); an idempotent `start_job` (sequential, concurrent, per user), a retry that says something else is refused; `start_job.gate` (taken, refused with the reason, a retry with another gate refused); a derived job id that another user's thread holds; `web_url`; `get_job` to `done` with the pull request; another user's job is "no such job" for `get_job`, `wait_for_job`, `answer` and `cancel_job`; `answer` on a blocked job and on a finished one; `cancel_job` on a running and on a finished job; the `2026-07-28` handshake |

The end-to-end tests over the real dispatcher and the A2A adapter, on the in-memory store and on Postgres, and across
two replicas (a wait that follows a job another replica delivers; a replica killed mid-wait and the call repeated on the
other, and concurrent `start_job` calls with one `client_request_id` across replicas making one job), are `mcp.rs` in
[`orch-e2e`](../e2e/README.md).

| Variable | Meaning |
|---|---|
| none | the tests here run against the in-memory stack and need no database |
