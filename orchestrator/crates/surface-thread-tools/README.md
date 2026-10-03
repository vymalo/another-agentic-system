# orch-surface-thread-tools

The thread-tools surface ([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md)): **one MCP endpoint per thread**,
at `/thread-tools/{threadId}/mcp`, for the agent that is working on that thread. An agent whose card lists the extension
receives `{url, token, expiresAt}` in the metadata of the A2A message and calls this endpoint with the token as a bearer.
Today it serves two tools, `get_ui_catalog` (the refetch seam of [ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)) and `turn_output` (the agent announces its answer for the turn, [ADR 0031](../../../docs/decisions/0031-working-text-and-the-turns-answer.md));
the tools of the MCP servers attached to the thread (the **relay**, slice 8, [below](#the-relay)) and `ask_agent` (slice 10, [below](#ask_agent)) are added through the provider seam below.

## Where it sits

An adapter over [`orch-app`](../app/README.md)'s `App`, like every inbound surface
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)): it depends on `App`,
[`orch-core`](../core/README.md), `orch-ports` (the `Ports` bound), [`orch-api`](../api/README.md) (the shared
`SurfaceRoutes`, problems and `is_host_authority`) and [`orch-thread-token`](../thread-token/README.md) (the token), and
on [`rmcp`](https://crates.io/crates/rmcp) 3.5 for the protocol. It is a Cargo feature of the binary
([`orchestrator`](../../bin/orchestrator/README.md), `surface-thread-tools`, on by default) and is mounted by
`ORCH_SURFACES` (name `thread-tools`). The route is a [machine route](../api/README.md): outside the identity layer, no
cookie, `X-Auth-Request-Email` never read, and **not under `/mcp`**, which belongs to
[`orch-surface-mcp`](../surface-mcp/README.md) (a path under that mount would collide with it).

## API at a glance

| Item | What |
|---|---|
| `routes::<P>(Arc<App<P>>, ThreadToolsConfig) -> orch_api::SurfaceRoutes` | `ROUTE` (`/thread-tools/{threadId}/mcp`) as a machine route, guarded as below |
| `ThreadToolsConfig::new(keys, allowed_hosts)`, `.with_tool_timeout(d)`, `.with_provider(p)` | the keys the tokens are verified with (`ThreadToolsKeys`), the `Host` values the server accepts (never empty, each a `host` or `host:port`), how long a built-in tool may take (default `DEFAULT_TOOL_TIMEOUT`, 30 s) and the providers of more tools. `ConfigError` says what is wrong |
| `ThreadTool` | the closed set of built-in tools (`GetUiCatalog`): names, schemas and definitions |
| `ThreadToolProvider` (`list(&ToolCtx)`, `call(&ToolCtx, name, args) -> Option<Result<CallToolResult, ErrorData>>`) | the seam later slices add tools through. Not a port: a provider is part of this surface's composition, chosen when the binary is built. Asked **for each request**, with no state kept between calls; `call` answers `None` for a name that is not its own |
| `RelayTools::new(app, client, servers)` (`RelayTools<P, C>`, a `ThreadToolProvider`) | the relay: `servers` are `(ToolServerInfo, ToolServerEndpoint)` pairs (the public part and the endpoint with its credentials), `client` a `ToolServerClient` (`orch-tools-mcp` in the binary, `MemoryToolServers` in tests); `RelayError` for a bad or duplicate id or an endpoint of another id. `RELAY_META_KEY` (`thread-tools/v1`), `RELAY_ERRORS`, `TASK_OVER`, `TIMEOUT_MARGIN`, `MAX_TOOL_NAME_BYTES` |
| `AskTools::new(app)`, `.with_heartbeat(d)` (`AskTools<P>`, a `ThreadToolProvider`), `ASK_TOOL_NAME`, `ASK_HEARTBEAT` (30 s), `ASK_WAIT_MARGIN` (30 s), `ASK_MAX_MESSAGE_CHARS` (16 000) | `ask_agent` ([below](#ask_agent)): the limits and the deadline are the application's (`App::ask_limits`) |
| `ToolCtx { owner, claims, meta, progress, cancel }` | what a provider knows of the call: the thread's owner, the verified `Claims` (thread, job, agent, caller, depth), the request's `_meta`, a `ProgressSink` (`send(progress, message)`; a no-op when the client sent no `progressToken`) and a `CancellationToken` |

### The guard

Every request is checked before any tool runs, in the order of the contract: a bearer token is present; it is a valid
token ([`orch_thread_token::verify`](../thread-token/README.md): size, header, key, signature, claims, issuer and
audience, lifetime); its thread is the path's; the thread exists and the caller is its agent (`main`: the thread's agent
is the token's `agt`; for `ask:<n>`, ask n of the token's job is on the thread's ledger as the token's agent at the token's depth, **running or not**: a call from an ask that ended is told "this task is over" by the tools, not by a dead token). Every failure is the same
`401`, `WWW-Authenticate: Bearer error="invalid_token"`, an RFC 9457 problem with no detail, nothing written and nothing
called. No bearer token (or two `Authorization` headers, or another scheme) is `401` with `WWW-Authenticate: Bearer` and
no error code (RFC 6750). A thread that cannot be read because the store failed is `503` with `Retry-After`, not a
`401`: an agent must not take a transient fault for a dead token. The `Host` header is then checked against the list
(`403`), and a request that carries an `Origin` is refused (`403`: an agent is not a browser).

### The tools

`tools/list` is the built-in tools, then each provider's, in the order they were added, computed for each request. A
provider's tool whose name a built-in or an earlier provider already has is left out, and a provider that is slow to list
leaves its tools out. `tools/call` offers the name to the built-ins, then to the providers in order; a name nobody owns is
the JSON-RPC error `-32602`. A built-in tool is cut off after 30 s (a tool error); a provider bounds its own calls.

| Tool | Arguments | Result (structured content, also as text) |
|---|---|---|
| `get_ui_catalog` | `knownDigest?` (`sha256:` and 64 lowercase hex) | `{catalogId, version, digest, unchanged, catalog?}`: the newest catalog the thread recorded; `catalog` is left out when `knownDigest` is the current digest (`unchanged: true`). A thread with no catalog is `isError: true`, "this thread has no UI catalog; answer in text". Read through `App::thread_ui_catalog` |
| `turn_output` | `text` (Markdown, 1 to 65 536 bytes, no other member) | `{"delivered": true}`. Records the agent's answer through `App::record_answer` (an `agent_message` `purpose: answer, via: turn_output`, id `out-<jti>-<n>`, by the token's agent): accepted only while the thread is `queued` or `working`, for the token's job and, once the turn has an announcement, under the token that made it. An empty (white space alone) or oversize text, and a turn that is over (`this turn is over`: the thread is not working, another job, another token) are `isError: true` and write nothing; a missing or mistyped `text` is `-32602`. Not read-only and not idempotent: a later call replaces the answer ([`thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md#turn_output)) |

### `ask_agent`

`AskTools` ([`thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md#ask_agent), [ADR 0026](../../../docs/decisions/0026-agent-mentions-as-structured-references.md)): the agent the thread is addressed to asks one of the agents the person mentioned and waits for its answer. The rules are the core's and the checks the application's (`App::ask`); this is the tool's face and its wait.

* `tools/list` offers it while the thread runs, the token's job is the thread's, the person mentioned someone in the job and the caller's depth is below `asks.maxDepth`; its description names the agents that may be asked and its `_meta` says `reportsStep: true` and `timeoutSecs` (the ask's timeout plus 30). An asked agent (`ask:<n>`) gets it and the relayed tools, and **none of the built-in tools** (`get_ui_catalog` and `turn_output` are the addressed agent's: they are not on its endpoint, `-32602`). A call that arrives when it is not offered is an `isError` result that says why, never `-32602`.
* `tools/call` validates the arguments (`agent`, `message` of 1 to 16 000 characters, `timeout_secs` 10 to 7200, nothing else: `-32602`), asks through `App::ask` and **waits**: it follows the log from the ask's own event (`thread_events_for_tools`) until `ask_finished` for this ask and gives `{ask, agent, state, text?, artifacts?, question?, error?}` as structured content and the same JSON as text, an error unless the state is `completed`, `input_required` or `auth_required`. A refusal (not mentioned, a cycle, depth, the job's asks, asks running, a task that is over, a call key used for another ask, an agent no role may invoke or the registry dropped, a registry that cannot say) is an `isError` text in the contract's words and writes nothing.
* With a `progressToken` it reports one notification per step under the ask (`<agent>: <label>`: an asked agent's relayed call is a step of `ask-<n>`) and per ask the asked agent started, and, in any case, `waiting for <agent>` every heartbeat (30 s).
* **A dropped call does not cancel the ask.** A client that goes or cancels ends the wait; the ask runs on and a call with the same `callId` re-attaches (a call key is `ask:<thread>:<caller>:<callId>`); one made after the end is answered at once with the recorded result. The wait is bounded by the ask's deadline plus 30 s; the deadline itself is the core's timer (`timed_out`, the asked agent told to stop).
* The relay's steps for an asked agent: its call is a step under `ask-<n>` (whatever `parentStepId` it says), attributed to it with no revision of the thread's task, and a call from an ask that ended is "this task is over" with no step.

### The relay

`RelayTools` serves the servers that are **attached to the thread** and **offered for the caller's agent** ([`thread-tools-v1.md`](../../../docs/api/thread-tools-v1.md#attached-servers-and-the-relay-slice-8), [ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)):

* `tools/list`: each server's upstream `tools/list` (no cache), its allow-list intersected, each tool named `<server id>__<tool>` (left out, with a warning, when the name is over 64 bytes or outside `[A-Za-z0-9_-]`, or the tool's own name starts with `_`), `title` upstream's or `<server name>: <tool>`, `description` cut at 8 KiB, `inputSchema` with `"type": "object"` added when missing, `outputSchema` and `annotations` passed, `icons` the configured `data:` icon only, `_meta["thread-tools/v1"] = {reportsStep: true, timeoutSecs: <server timeout + 5>}`. A server that cannot be listed is left out and logged (its public detail only).
* `tools/call`: the call goes upstream through the `ToolServerClient` with the endpoint's bearer and headers and is **one step** recorded through `App::record_step` (actor the calling agent; `kind: tool`, label `<server name> · <tool>`, icon `mcp-server:<id>`, `input` the arguments, `output` the result's text, both under the ADR 0030 bounds): `running`, then `completed`, `failed` or `canceled`. The step's id is `tool-<callId>` when the request's `_meta["thread-tools/v1"].callId` is usable, so a retried call reports the same step (the call itself is made again, at least once), else `tool-<uuid>`; `parentStepId` nests it under `<task id>/<parentStepId>`. A step that cannot be recorded is logged and never fails the call; a call whose future is dropped (the agent's connection closed) ends its step as `canceled` from a guard, and so does `ToolCtx::cancel`.
* A name that is not on the endpoint (an unknown server, a detached one, a tool left out by the allow-list or the agent filter, a name that does not fit) is not owned by the relay, so the endpoint answers `-32602`; a thread whose task is over (terminal, or another job than the token's) is an `isError` result "this task is over" and no step. The other rows of the error table (`RELAY_ERRORS`) are `isError` results and a `failed` step, except the result over 256 KiB, which is cut with a note and is `completed`.
* The credentials are in the `ToolServerEndpoint` only, which prints none; the relay logs a server's id and the error's public detail, never an argument, a result or a credential.

Stateless: `rmcp`'s `StreamableHttpService` runs with `legacy_session_mode = false` and a session manager that keeps
nothing, so any replica serves any request; a call is one `application/json` response.

## Tests

`cargo test -p orch-surface-thread-tools`, with rmcp's own client over a real TCP port on the in-memory stack:

* `tests/tools.rs`: `tools/list` and its schemas; `get_ui_catalog` with no catalog (an error to read), with version 1
  then 2 recorded (the newest, the same JSON as text), the current catalog still given after 40 older screens were recorded after it, with `knownDigest` (unchanged, no catalog), an older screen (the
  newest stays), one catalog per thread, arguments that do not parse (`-32602`), an unknown name; the provider seam (order
  of the listing, routing of calls, the context a provider gets, a provider that cannot take a built-in's name, the
  listing built for each request).
* `tests/turn_output.rs`: `turn_output` is listed after `get_ui_catalog` with its schemas and description; an announced answer is the agent's `agent_message` (`out-<jti>-1`, `purpose: answer`, `via: turn_output`) and the thread goes on working; a second call is `out-<jti>-2` (the later one replaces the answer by the reader's rule); an empty or oversize text is an error to read and writes nothing (the largest text is accepted); a missing, mistyped or extra argument is `-32602`; the turn is over for a finished thread and for one cancelled after the first announcement; a token of another job, and a token of another message once one announced, are refused.
* `tests/guard.rs`: no credentials, two headers, every kind of bad token (garbage, another key, expired, issued in the
  future, another thread's, another agent's, an `ask` that no ledger holds, a thread nobody created, a changed signature) is the same `401`
  and nothing is called; an asked agent's token must name an ask on the ledger as that agent at that depth in that job (another agent, another depth, another job, another thread's ask: `401`; an ask that ended still opens the endpoint); the previous key still opens the endpoint and a replica that dropped it refuses; the `Host` and
  `Origin` checks; the route is a machine route that is not under `/mcp`; the configuration is checked.
* `tests/ask.rs`: `ask_agent` on the in-memory stack with the real dispatcher and a scripted asked agent: what is listed and for whom (the addressed agent, an asked agent with no built-in tools, depth 2 with none, a job nobody was mentioned in, a thread that is over); an answer (the same JSON as text, `ask_started` by the asker, the asked agent sent once in a context of its own as `ask:1`); a question back and the next ask continuing the task; a failed task; every refusal (not mentioned, a call key used for another agent or question, depth, a cycle, asks running, a thread that is over) writing nothing; the job's asks, another job's token and a finished job; the arguments (`-32602`); a repeat of a call re-attaching to one ask on two connections and answered at once after the end; a dropped client leaving the ask running and a second call re-attaching; the deadline ending `timed_out` with the asked agent cancelled and the job still working; a call lowering the deadline to one timer of a minute; the person's cancel ending the ask and its child, both calls and both asked agents; progress for a step under the ask and a heartbeat. Units in `src/ask.rs`: the arguments, the definition, the refusal words, the results, the mapping of the application's errors.
* `tests/relay.rs`: the relay on `MemoryToolServers` over the in-memory stack: naming, `_meta`, the icon, a schema with no type, names that do not fit; the allow-list; the agent filter (at attach time and in the relay's own list); a server that cannot be listed; a call (the upstream sees the bearer and not the agent's `_meta`, one step with input, output and the server's icon, the credential in no event); `callId` and `parentStepId`; every row of the error table (unreachable, timeout, refused credentials, a JSON-RPC error, `isError`, a result over 256 KiB, a dropped call ends `canceled`); a detached or unknown name is `-32602` with no step and no call upstream; a task that is over; a bad relay configuration. An asked agent's call is a step under `ask-<n>` attributed to it, and its tools end with its ask.
