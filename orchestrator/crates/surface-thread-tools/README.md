# orch-surface-thread-tools

The thread-tools surface ([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md)): **one MCP endpoint per thread**,
at `/thread-tools/{threadId}/mcp`, for the agent that is working on that thread. An agent whose card lists the extension
receives `{url, token, expiresAt}` in the metadata of the A2A message and calls this endpoint with the token as a bearer.
Today it serves two tools, `get_ui_catalog` (the refetch seam of [ADR 0023](../../../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md)) and `turn_output` (the agent announces its answer for the turn, [ADR 0031](../../../docs/decisions/0031-working-text-and-the-turns-answer.md));
the tools of attached MCP servers (slice 8) and `ask_agent` (slice 10) are added through the provider seam below.

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
| `ToolCtx { owner, claims, meta, progress, cancel }` | what a provider knows of the call: the thread's owner, the verified `Claims` (thread, job, agent, caller, depth), the request's `_meta`, a `ProgressSink` (`send(progress, message)`; a no-op when the client sent no `progressToken`) and a `CancellationToken` |

### The guard

Every request is checked before any tool runs, in the order of the contract: a bearer token is present; it is a valid
token ([`orch_thread_token::verify`](../thread-token/README.md): size, header, key, signature, claims, issuer and
audience, lifetime); its thread is the path's; the thread exists and the caller is its agent (`main`: the thread's agent
is the token's `agt`; an `ask:<n>` token is refused until a slice builds the ledger of asks). Every failure is the same
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
  future, another thread's, another agent's, an `ask`, a thread nobody created, a changed signature) is the same `401`
  and nothing is called; the previous key still opens the endpoint and a replica that dropped it refuses; the `Host` and
  `Origin` checks; the route is a machine route that is not under `/mcp`; the configuration is checked.
