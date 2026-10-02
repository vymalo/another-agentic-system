# orch-tools-mcp

`ToolServerClient` over MCP: the orchestrator as a **client** of an MCP server, to list its tools and
call one. It is `rmcp`'s client over streamable HTTP behind the port in
[`orch-ports`](../ports/README.md), so the relay of the thread-tools endpoint
([`thread-tools/v1`](../../../docs/api/thread-tools-v1.md), [ADR 0024](../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md))
sees the port's types and never an MCP library type.

## Where it sits

An **adapter** of the `ToolServerClient` port
([ADR 0009](../../../docs/decisions/0009-swappable-implementations-at-build-time.md)): the one
implementation that speaks the network. The in-memory one, `MemoryToolServers`, is in `orch-ports`
(feature `testkit`). Nothing depends on this crate yet: the relay
(`orch-surface-thread-tools`, behind the binary's feature `tool-relay`) is the slice after the port,
and the binary composes it. The client side of `rmcp` is enabled in this crate's manifest only (the
workspace entry has the server side, which the surfaces use). The server a person lists is the
deployment's own (`toolServers` of the configuration, [`config.md`](../../../docs/api/config.md#toolservers)),
never one a person or an agent typed.

## API at a glance

| Item | What |
|---|---|
| `McpToolClient::new() -> Result<McpToolClient, BuildError>` | implements `ToolServerClient`. Builds one HTTP client (no idle connection kept, a connect timeout of 10 s, **no redirect followed**, no proxy of the environment) and installs the `rustls` crypto provider if none is installed. Cheap to clone. `BuildError::Http` is the TLS backend not starting |

The port's `list_tools(&ToolServerEndpoint)` and `call_tool(&ToolServerEndpoint, &ToolCall)` are the whole
interface; the types are in [`orch-ports`](../ports/README.md#api-at-a-glance).

**One request is one session.** Each call connects, runs `initialize`, makes its one request
(`tools/list` with all its pages, or `tools/call` with the arguments and the request's `_meta`) and lets
the session go; nothing is kept between requests and no listing is cached, so any replica serves any
request ([ADR 0001](../../../docs/decisions/0001-rust-state-machine-on-postgres.md)). The transport
accepts a server that keeps no session as well as one that does (rmcp's `allow_stateless`).

**The endpoint's `timeout` bounds the whole request.** A handshake still open at the deadline is
`Unreachable` (nothing answered), a call still open is `TimedOut { after }`. A call is **cancelled by
dropping its future**: the request is abandoned and the session closed in the background; the server may
run the call to its end.

How a server's answer maps to the port's errors (`Classify`):

| The server | `ToolServerError` | Class |
|---|---|---|
| refuses or drops the connection, resets, answers 5xx, 404, 408 or 429, or does not finish the handshake | `Unreachable` (no URL in the text) | transient |
| answers 401 or 403, with `WWW-Authenticate` or without | `Unauthenticated` | unauthenticated |
| has not answered a call in time | `TimedOut` | transient |
| answers a JSON-RPC error (an unknown tool, arguments it cannot read) | `Remote { code, message }`, the message cut at 1 KiB | invalid |
| answers something that is not MCP (HTML, a malformed message), or another 4xx | `Protocol` | transient |
| has a result with `isError: true` | **not an error**: `Ok` with `is_error`, passed through | |
| the endpoint is unusable: a URL that is not `http` or `https`, a header a request cannot carry, one of `Authorization`, `Accept`, `Content-Type`, `Host`, `Mcp-Session-Id`, `Mcp-Protocol-Version`, `Last-Event-ID` | `Misconfigured`, nothing sent | invalid |

**Credentials.** `Authorization: Bearer <bearer>` and the endpoint's headers go on every request to that
server and nowhere else: redirects are not followed (they would replay the headers to another host), the
values are marked sensitive in the HTTP client, and no error holds one (the conformance suite searches
every error, its sources and its `Debug`). The rmcp error is kept as the `source` of `Unreachable` and
`Protocol`, which may hold the server's URL and body; `public_detail()` is what may be shown.

**Bounds.** The content of a result is cut at `MAX_RESULT_BYTES` (256 KiB) before it is returned
(`truncated`). The cut is of what is returned, not of what is read: an event of a streamed answer over
4 MiB is refused (`Protocol`), and a plain JSON answer is read whole, bounded by the endpoint's timeout.
What a server says is untrusted text: the tools' descriptions, the content of a result and an error's
message go back into an agent's model, so they are data to pass on. The icons a server lists are dropped
(open question 38), and so is anything else the port does not carry.

*Verified 2026-10-02, against `rmcp` 3.5.0 (`~/.cargo/registry`):* a 401 with `WWW-Authenticate` is
`StreamableHttpError::AuthRequired`; a 401, 403 or 5xx without it is
`StreamableHttpError::UnexpectedServerResponse("HTTP <status>: <body>")`, from which this crate reads the
status (the tests of both shapes would fail on a change); a refused connection is `Client(reqwest::Error)`.

## Tests

Offline: a real MCP server over HTTP on a loopback port (`orch_testsupport::FakeToolServer`). No
environment variables.

* `tests/mcp.rs`: the `ToolServerClient` conformance suite of `orch-ports` (`tool_server_conformance!`,
  eleven cases: the tools and their schemas, the credentials on the wire, an echo round trip with the
  arguments and `_meta`, `isError` passed through, a wrong or missing bearer is `Unauthenticated`, an
  unreachable server, a slow call is `TimedOut` within the timeout plus a second, a dropped call leaves the
  client usable, an unknown tool is `Remote`, a result over 256 KiB is cut, and no secret in any error or
  `Debug`), run **three times**: against a server that keeps no session, one that keeps a session and
  answers on an event stream, and one that refuses with a bare 401. And what only this client says: a 5xx
  is unreachable, a 403 unauthenticated, an HTML page a protocol error, a server that accepts and says
  nothing is unreachable within the timeout, a redirect is not followed (the target is never reached), an
  endpoint the client cannot use is `Misconfigured` for a listing and a call, every request lists again,
  and non-ASCII arguments and `_meta` arrive as the server can read them.

```sh
cargo test -p orch-tools-mcp
```
