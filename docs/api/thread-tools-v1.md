# A2A extension: thread tools (v1)

- **URI:** `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`
- **Status:** **built (MVP slice 3, 2026-10-01), apart from the tools of later slices.** Accepted on the owner's
  delegation; the owner may revisit anything here. Built: `orch-thread-token` (the token, with the known-answer vectors
  below), `orch-surface-thread-tools` (the route, the guard, `get_ui_catalog`, the seam for later tools), the binary's
  `thread-tools` surface and `THREAD_TOOLS_*` settings, and the grant in the A2A message (the adapter mints at send
  time, only for an agent whose live card lists the extension). `turn_output` (an agent announces its answer,
  [ADR 0031](../decisions/0031-working-text-and-the-turns-answer.md) amendment of 2026-10-02) is built, below.
  **Built 2026-10-02 (slice 8, first half):** the servers a person attaches to a thread (the configuration, the events
  and the API: [below](#what-is-attached-and-by-whom)) and the `attached` member of the message
  ([below](#the-attached-member)). **Built 2026-10-02 (slice 8, second half):** the relay of the attached servers' tools (their `_meta`, the
  step the orchestrator reports for each call, the error table), [below](#attached-servers-and-the-relay-slice-8):
  `RelayTools` in `orch-surface-thread-tools`, composed by the binary behind the Cargo feature `tool-relay` (on by
  default). **Written 2026-10-02 (contract accepted on the owner's delegation, not built):** `ask_agent` with the
  `ask:<n>` ledger (slice 10), [below](#ask_agent). "Not yet" is marked where it matters. The adam-rs side (an agent
  that reads the grant and calls the endpoint) is that repository's slice; the `thread-tools` script of the test
  support's fake agent is the reference of what an agent does.
- **Decided in:** the status notes of [ADR 0023](../decisions/0023-ui-component-catalog-as-an-a2a-extension.md) (the
  endpoint, the token, the refetch), [ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md) (the relay)
  and [ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md) (`ask_agent`); a second MCP endpoint
  beside the one of [ADR 0019](../decisions/0019-mcp-server-over-streamable-http.md); the optional-extension pattern
  is [ADR 0008](../decisions/0008-platform-integration-via-a2a-extension.md).
- **Served by:** the orchestrator. **Used by:** agents whose card lists the extension.
- **Closes:** [open questions](../open-questions.md) 34 (where `ask_agent` lives), 35 (the relay) and 36 (the
  refetch's transport).

## Purpose

Give an agent, for the thread it is working on, **one MCP endpoint** through which it reaches what the orchestrator
holds for that thread: the UI catalog (slice 3), the tools the person attached (slice 8), and the other agents it may
ask (slice 10). The agent needs no other credential, and no credential of a tool server ever travels in an A2A message
or reaches the event log or the outbox ([ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md)). The
orchestrator sees each call, so it can report it as a step ([ADR 0025](../decisions/0025-nested-steps-events-carry-their-source-path.md)).

An agent whose card does not list the extension gets nothing and works as it does today.

Facts about the RFCs and MCP are marked *verified* (with a date and a source) or *unverified*; see
[Verified and unverified](#verified-and-unverified-2026-10-01).

## The flow

```mermaid
sequenceDiagram
  autonumber
  participant D as Dispatcher
  participant X as A2A adapter
  participant A as Agent (card lists thread-tools/v1)
  participant E as Thread tools endpoint (any replica)
  participant S as Store

  D->>X: SendRequest with a grant {thread, job, agent, caller, depth} (no secret)
  X->>A: read the card (live, every send)
  X->>X: mint the token (HS256, key from the configuration), exp = now + TTL
  X->>A: message: metadata thread-tools/v1 {url, token, expiresAt}, extension activated
  Note over D,X: the token is not written to the log, the outbox or a log line
  A->>E: POST /thread-tools/{threadId}/mcp, Authorization: Bearer token, tools/list
  E->>E: verify the token (the checks below, in order)
  E->>S: the thread exists and the caller is its agent
  E-->>A: the tools: built-ins first, then each provider's
  A->>E: tools/call get_ui_catalog {knownDigest?}
  E->>S: read the thread's catalog
  E-->>A: result (structuredContent and the same JSON as text)
  A->>E: a call with a bad, expired or foreign token
  E-->>A: 401, WWW-Authenticate: Bearer error="invalid_token" (nothing written, nothing called)
```

```mermaid
stateDiagram-v2
  [*] --> Minted: the A2A adapter signs it at send time
  Minted --> Sent: put in the message metadata
  Minted --> [*]: the send fails (nothing keeps the token)
  Sent --> Valid: a call passes every check
  Valid --> Valid: later calls pass
  Sent --> Expired: exp passes (plus 30 s of clock skew)
  Valid --> Expired: exp passes (plus 30 s of clock skew)
  Sent --> Rejected: a check fails (a key rotated out, another thread, another agent)
  Valid --> Rejected: a check fails
  Expired --> [*]: 401 on every call
  Rejected --> [*]: 401 on every call
```

A 401 is decided per request; a token that fails a check keeps failing it, so "expired" and "rejected" end the
token's useful life. A new message carries a new token.

## The card

```json
{"capabilities": {"extensions": [
  {"uri": "https://agents.vymalo.com/a2a/extensions/thread-tools/v1", "required": false}
]}}
```

The orchestrator reads the card on **every send**, never caches it, and fails closed: an unreadable card or a card
without the URI gives the agent no grant. The extensions a live card lists are read into one closed set (thread
tools, UI catalog, steps, mentions), and the AG-UI capabilities document lists each under `custom`, so the web can
flag an agent before the person sends ([`agui.md`](agui.md#capabilities-document)).

## The message

Only when **all three** hold: the card lists the extension (read for this very message, exact URI), the request belongs
to a thread (a request to the verifier agent does not), and the orchestrator has a key and a base URL configured
(`THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL`). The message metadata then has:

```json
{"metadata": {"https://agents.vymalo.com/a2a/extensions/thread-tools/v1": {
  "url": "https://orchestrator.example/thread-tools/1b4e28ba-2fa1-11d2-883f-0016d3cca427/mcp",
  "token": "<the JWT of the next section>",
  "expiresAt": "2026-10-01T14:00:00Z"
}}}
```

`url` is `THREAD_TOOLS_URL` plus `/thread-tools/<threadId>/mcp`. `expiresAt` is the token's `exp` as RFC 3339. The URI
is also added to the `A2A-Extensions` header and to `message.extensions`. **Every message gets its own token**, a
follow-up included; an agent keeps using the newest one it was given, and a task that outlives its token is why the
lifetime is long (below). When servers are attached to the thread the metadata also has [`attached`](#the-attached-member).

The orchestrator never writes the token to the event log, to the outbox payload or to any log or trace field. What
travels inside the orchestrator is a non-secret grant (the thread, the job, the agent, the caller and the depth) on the
send request; only the A2A adapter mints the token, at the moment it sends, and the types that hold it print
`[redacted]`. The agent must treat it as a secret: not in the model's context, not in its own logs.

*Built and tested (2026-10-01):* a message to a card without the exact URI (another version, a trailing slash, another
scheme or case), from an adapter with no keys, or for a request with no grant (the verifier's) is exactly the message it
was before the extension existed; the card is read for every message, so an agent that drops the extension gets nothing
from the next one; each message has a token of its own (`jti` is its message id). The token is searched for in
everything the system keeps or says: the adapter's tests capture every log line of a send at the most verbose level and
find neither the token, nor a segment of it, nor the key; the end-to-end test reads the event log, the thread, its
export, every AG-UI frame of the runs and, on Postgres, **every row of every table of the schema**, and finds none; a
test that makes the adapter log the metadata fails.

### The `attached` member

*Built 2026-10-02 (slice 8, first half).* When MCP servers are attached to the thread
([ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md)), the metadata above also has `attached`:

```json
{"https://agents.vymalo.com/a2a/extensions/thread-tools/v1": {
  "url": "https://orchestrator.example/thread-tools/1b4e28ba-2fa1-11d2-883f-0016d3cca427/mcp",
  "token": "<the JWT>",
  "expiresAt": "2026-10-02T14:00:00Z",
  "attached": [
    {"server": "websearch", "name": "Web search", "description": "Search the web."}
  ]
}}
```

| Member | Meaning |
|---|---|
| `server` | The server's id, as the deployment lists it: `^[a-z0-9][a-z0-9-]{0,30}$`. It is the prefix of the server's tools on the endpoint (`websearch__search`). |
| `name` | The display name, 1 to 80 characters. |
| `description` | Optional, at most 500 characters. |

- It is a **hint for the model's instructions** ("a web search is attached"), **not the list of tools**: `tools/list` is the
  truth, computed for each request, so a server attached after the message was sent is listed from the next request on and
  one detached is gone.
- It is the servers attached to the thread **at the moment of the send** that the thread's agent may use (a server whose
  `agents` list leaves the agent out is never attached to its threads, and is left out here too), at most 16, in the order
  of their ids, and **omitted when there are none**. A thread with servers attached and an agent whose card lacks the
  extension gets nothing, and the screen says so before the person sends (ADR 0024).
- It carries **never a URL, a header or a credential**: the agent reaches a server only through the endpoint.
- An asked agent ([`ask_agent`](#ask_agent)) gets `attached` for the servers allowed for **it**.

*Built and tested (2026-10-02):* the dispatcher reads the thread's set when it **sends** (a retry sends what is attached
at the retry, and a server attached or detached between two messages is told or dropped by the next one), keeps those
the deployment lists for the thread's agent (a server it no longer lists is not told) and puts them in the grant; the A2A
adapter writes `attached` only into a message that carries the grant, so a card without the extension, an adapter without
keys and a request without a grant (the verifier's) get exactly the message they got before. The thread's agent in the
orchestrator's end-to-end tests is one whose card lists the extension (told) and one whose card does not (not told, the
message untouched). The relay and its tools are not built: today the endpoint lists `get_ui_catalog` and `turn_output`
whatever is attached, so an agent that reads `attached` knows what the person wants and finds the tools only when the
relay lands.

### Trying it

The fake agent of the test support (`orch-fake-agent`, `FAKE_AGENT_EXTENSIONS=thread-tools`, or `FakeAgentOptions::extensions`
in a Rust test) lists the extension, records the grant of each message (`threadTools` in its `/__control/<agent>/calls`),
and its `thread-tools` script is what an agent does with it: it calls the endpoint with rmcp's own client, lists the tools,
calls `get_ui_catalog` twice (the second time with the digest it was given) and reports one line, for example
`thread-tools: tools=get_ui_catalog,turn_output; catalog=<id> v2 <digest>; again unchanged=true`, or `no catalog: this thread has no UI
catalog; answer in text`, or `no grant`. In the dev stack ([`dev/README.md`](../../dev/README.md#the-thread-tools)) every
orchestrator process has `THREAD_TOOLS_SECRET` and `THREAD_TOOLS_URL=http://orchestrator:8080`, and `orchestrator` serves
the endpoint (the edge does not route it); an agent of the stack that lists the extension receives the grant.

## The token

A JWS in compact form, HS256 (HMAC-SHA-256), three base64url segments without padding.

**Header** (exactly this; any other `alg` or `typ` is refused, `none` included):

```json
{"alg": "HS256", "typ": "JWT", "kid": "<first 16 hex characters of SHA-256(the key)>"}
```

**Claims:**

| Claim | Value | Meaning |
|---|---|---|
| `iss` | `"orch"` | Issuer; checked. |
| `aud` | `"thread-tools"` | Audience; checked. |
| `sub` | the thread id (UUID) | The one thread the token opens; must equal `{threadId}` in the path. |
| `job` | integer, from 1 | The number of the job the message belongs to ([ADR 0020](../decisions/0020-a-thread-is-a-conversation.md)). Tools that attribute work to a job read it; `get_ui_catalog` does not. |
| `agt` | string | The id of the agent the token was minted for. |
| `caller` | `"main"` or `"ask:<n>"` | Who is calling: the thread's addressed agent, or the n-th asked agent of the job, n from 1 ([ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md)). A closed set. |
| `depth` | integer, 0 to 255 | 0 for `main`; for an asked agent, its depth in the chain of asks. |
| `jti` | string | The A2A message id the token was minted for. One token per message. |
| `iat` | integer, seconds since the epoch | When it was minted (the orchestrator's wall clock). |
| `exp` | integer, seconds since the epoch | `iat` plus the lifetime. |

**Lifetime.** `THREAD_TOOLS_TOKEN_TTL_SECS`, default **7200** (two hours), allowed 60 to 86,400. Two hours because a
task can run long, and an ask can last half an hour inside it; a call that fails with 401 in the middle of a task is
hard for an agent to recover from. The token's protection is its **scope** (one thread, one caller) more than its
brevity.

**Minting.** By the A2A adapter, when it sends the message, from a non-secret grant on the send request. The key comes
from the configuration, so **any replica can verify** a token any replica minted (the processes stay stateless,
[ADR 0001](../decisions/0001-rust-state-machine-on-postgres.md)). The key is at least 32 bytes.

**Rotation.** `THREAD_TOOLS_SECRET_PREVIOUS` is verified, never used to sign. The header's `kid` says which key to try
(the current or the previous one). Rotate by moving the current key to `…_PREVIOUS` and setting a new one; remove the
previous key once the longest lifetime in use has passed.

### Verification

All of these, in this order. Any failure is the same answer: `401`, `WWW-Authenticate: Bearer error="invalid_token"`,
no detail in the body, nothing written, nothing called.

1. The token is at most 2 KiB and has three segments.
2. The header is exactly HS256 and JWT, and `kid` names the current or the previous key.
3. The signature matches, compared in constant time.
4. The claims parse: every one present and well formed, `caller` is `main` or `ask:<n>`, a `main` has depth 0.
5. `iss` and `aud` are the expected ones.
6. `exp` is later than now minus 30 seconds, and `iat` is not later than now plus 30 seconds.
7. `sub` equals the thread id in the path.
8. The thread exists, and the caller is its agent: for `main`, the thread's agent is `agt`. For `ask:<n>` (slice 10),
   ask n of the job is on the thread's ledger and is the agent `agt`. *Built:* `main`. Nothing writes an ask ledger
   yet, so an `ask:<n>` token is refused here until slice 10 builds it (the token crate already reads and writes it).

A thread that cannot be read because the store fails is **not** a failed token: the answer is `503` with
`Retry-After`, so that an agent does not take a transient fault for a dead token.

A request without an `Authorization` header (or with two of them, or with another scheme) gets a `401` with
`WWW-Authenticate: Bearer` and no error code, as RFC 6750 says to answer a request that carries no credentials. The
Host header is checked against `THREAD_TOOLS_ALLOWED_HOSTS` (403 when it is not listed), after the token: a stranger
with no token learns nothing of the hosts. A request that carries an `Origin` header is refused with 403 (an agent is
not a browser).

### Known-answer vectors

A fixed key, fixed claims and a fixed time give exactly these bytes. They were computed with an implementation that
shares no code with the orchestrator (Python's `hmac`, `hashlib` and `base64`, 2026-10-01, *verified*), and the token
crate's tests (`crates/thread-token/tests/vectors.rs`) pin the same strings: a change that moves a byte of one of them
is a change of this contract, a `v2` URI, not an edit. The key is the bytes of the string as written (here 64
characters of hexadecimal, which is what `openssl rand -hex 32` gives); `kid` is the first 16 hexadecimal characters of
the SHA-256 of those bytes; the header and the claims are compact JSON in the order shown (no spaces); the signature is
HMAC-SHA-256 over the first two segments with the dot, as written.

**These are test vectors, not credentials.** Each token below is signed with the made-up test key printed beside it (a counting
sequence, a descending one), names a made-up thread, agent and message, and expired on the day it was written; no
deployment holds these keys, so the tokens open nothing. (A secret scanner reports each as a JWT: that is what a
known-answer vector is.) Never use these keys in a deployment: `openssl rand -hex 32` gives a real one.

**Vector 1: the thread's addressed agent**, signed with key 1, minted at 2026-10-01T12:00:00Z (`iat` 1790856000) for
two hours.

```text
key 1   000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
kid     6c86c6aac5fb24bc
header  {"alg":"HS256","typ":"JWT","kid":"6c86c6aac5fb24bc"}
claims  {"iss":"orch","aud":"thread-tools","sub":"01927a4e-3b00-7000-8000-000000000001","job":3,"agt":"coder","caller":"main","depth":0,"jti":"5b0b9c2e-7f61-4d1c-9a43-2f3f6d0f9c11","iat":1790856000,"exp":1790863200}
token   eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6IjZjODZjNmFhYzVmYjI0YmMifQ.eyJpc3MiOiJvcmNoIiwiYXVkIjoidGhyZWFkLXRvb2xzIiwic3ViIjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiam9iIjozLCJhZ3QiOiJjb2RlciIsImNhbGxlciI6Im1haW4iLCJkZXB0aCI6MCwianRpIjoiNWIwYjljMmUtN2Y2MS00ZDFjLTlhNDMtMmYzZjZkMGY5YzExIiwiaWF0IjoxNzkwODU2MDAwLCJleHAiOjE3OTA4NjMyMDB9.jloITlRjG0bpM62IXeLGZq-mUH1badzKsCjKn0E5vvM
```

**Vector 2: an asked agent**, signed with key 2 (a replica holding key 2 as its current key, or as its previous key,
verifies it; one holding neither refuses it), valid for 60 seconds.

```text
key 2   ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100
kid     8588cdfcd6d2b0d5
header  {"alg":"HS256","typ":"JWT","kid":"8588cdfcd6d2b0d5"}
claims  {"iss":"orch","aud":"thread-tools","sub":"01927a4e-3b00-7000-8000-000000000001","job":3,"agt":"researcher","caller":"ask:2","depth":1,"jti":"a7d2e1c0-0b3f-4e55-8d7a-9c1e4f6b2a30","iat":1790856000,"exp":1790856060}
token   eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6Ijg1ODhjZGZjZDZkMmIwZDUifQ.eyJpc3MiOiJvcmNoIiwiYXVkIjoidGhyZWFkLXRvb2xzIiwic3ViIjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiam9iIjozLCJhZ3QiOiJyZXNlYXJjaGVyIiwiY2FsbGVyIjoiYXNrOjIiLCJkZXB0aCI6MSwianRpIjoiYTdkMmUxYzAtMGIzZi00ZTU1LThkN2EtOWMxZTRmNmIyYTMwIiwiaWF0IjoxNzkwODU2MDAwLCJleHAiOjE3OTA4NTYwNjB9.aIdaJXz3PRX8xbeV4LHMn9V3bH1tpaoa_ReeOq5STmg
```

Reading rules the tests pin as well: base64url is the strict, unpadded alphabet (a padded token, the standard alphabet
or non-canonical trailing bits is refused, so no two spellings stand for one token); the header may hold exactly `alg`,
`typ` and `kid`, and the claims exactly the ten above (an extra member is refused); `caller` is `main` or `ask:<n>` with
n from 1 and no leading zero; a `main` caller has `depth` 0 and an `ask` at least 1; `job` is at least 1; `agt` and
`jti` are non-empty and at most 256 bytes; `exp` is not before `iat`. At the boundaries: a token is read from 30
seconds before its `iat` to 29 seconds after its `exp`, and refused at 30.

## The endpoint

- **Route:** `/thread-tools/{threadId}/mcp`. It is **not** under `/mcp`: that mount belongs to the MCP server of
  [ADR 0019](../decisions/0019-mcp-server-over-streamable-http.md) and would swallow it.
- **A machine route**, outside the edge identity (no cookie; `X-Auth-Request-Email` is never read), like the webhooks
  ([`webhooks.md`](webhooks.md#routes)). Agents call the orchestrator directly, and the edge does not expose it to
  browsers.
- **MCP streamable HTTP, stateless.** No session id (the specification lets a server assign one, it does not require
  it, see [Verified](#verified-and-unverified-2026-10-01)), so any replica serves any request. A call is answered with
  one JSON response unless the tool streams progress (`ask_agent`, slice 10, as `wait_for_job` does in ADR 0019).
- **The surface** is named `thread-tools` in `ORCH_SURFACES` and is behind the Cargo feature `surface-thread-tools`
  (on by default).

### Configuration

| Variable | Meaning |
|---|---|
| `THREAD_TOOLS_SECRET` | The HMAC key, at least 32 bytes (`openssl rand -hex 32`). Never logged. |
| `THREAD_TOOLS_SECRET_PREVIOUS` | The previous key, for verifying only (rotation). |
| `THREAD_TOOLS_URL` | The base URL under which agents reach the orchestrator, http or https. |
| `THREAD_TOOLS_TOKEN_TTL_SECS` | The token lifetime, default 7200, 60 to 86,400. |
| `THREAD_TOOLS_ALLOWED_HOSTS` | The Host values accepted; default the host and port of `THREAD_TOOLS_URL`. |

The process exits with 78 (as for other configuration errors) when the surface is mounted without a key; when the URL
is set without a key or the key without a URL; when the previous key is set without a current one, is the current one,
or is too short; and for a bad URL, lifetime or host. The A2A adapter mints whenever the key and the URL are set, so
every role gets the same variables: **workers mint, the control plane serves**. That is why naming the surface
`thread-tools` in `ORCH_SURFACES` requires the key and the URL in **every** role (a worker mounts no routes, but it is
the one that mints), and why a process that does not name the surface still reads and checks them when they are set
(another replica serves the endpoint). The default for `THREAD_TOOLS_ALLOWED_HOSTS` is the host of the URL, with the
port when the URL names one; a name without a port matches any port. A build without the Cargo feature
`surface-thread-tools` does not read these variables and naming the surface is refused.

## The tools

`tools/list` answers the **built-in** tools first and then those of each **provider**, in a fixed order, computed per
request (the endpoint holds no state, so a server attached a moment ago is listed on the next request).
`tools/call` offers the name to the built-ins and then to the providers in that order; the first that owns it answers,
and a name nobody owns is the JSON-RPC error `-32602` (unknown tool). The built-in set is a closed enum
([ADR 0004](../decisions/0004-closed-enums-over-dyn-registry.md)); providers are chosen when the binary is built and by
configuration ([ADR 0009](../decisions/0009-swappable-implementations-at-build-time.md)), never loaded at run time.

| Slice | Tools | Provider | Contract |
|---|---|---|---|
| 3 (built) | `get_ui_catalog` | built in | below |
| built (2026-10-02) | `turn_output`: the agent announces its answer for the turn. | built in | [below](#turn_output) |
| 8 (built) | `<server>__<tool>`: the tools of each MCP server attached to the thread, relayed. The orchestrator holds the servers' credentials (from its configuration), sees each call and reports it as a tool step with the server's icon. | relay | [below](#attached-servers-and-the-relay-slice-8), [ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md) |
| 10 (written, not built) | `ask_agent`: the addressed agent asks a mentioned agent. The orchestrator runs it as a nested child task on the same thread, its steps under the step of the agent that asked, and returns its result to the call, with progress notifications. The asked agent's own token has `caller = ask:<n>` and a `depth`. | asks | [below](#ask_agent), [ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md) |

An agent should expose to its model **every tool the endpoint lists, under the listed name**, and re-read the list at
each model turn: it is not hard-wired to `get_ui_catalog`.

The seam is built (`ThreadToolProvider` in `orch-surface-thread-tools`, added to the surface's configuration by the
binary): a provider gets, for each request, a context with the thread's **owner**, the verified **claims** (thread, job,
agent, caller, depth), the request's `_meta`, a sink for progress notifications and a cancellation token. A provider
that offers a name a built-in tool, or an earlier provider, already has is left out of the listing (the first owner
keeps it), and a provider that is slow to list leaves its tools out rather than failing the listing. The built-in tools
are cut off after 30 seconds; a provider bounds its own calls, because a tool such as `ask_agent` may run much longer.

### `get_ui_catalog`

Returns the thread's current UI catalog ([`ui-catalog-v1.md`](ui-catalog-v1.md)): the newest version the thread
recorded. The agent calls it when its copy may be stale (a message whose digest it does not hold, a restart) and when
it was told `inline: false`.

Input:

```json
{"type": "object",
 "properties": {"knownDigest": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"}},
 "additionalProperties": false}
```

Output, as `structuredContent` and as the same JSON in one text content:

```json
{"type": "object",
 "properties": {
   "catalogId": {"type": "string"},
   "version": {"type": "integer", "minimum": 1},
   "digest": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"},
   "unchanged": {"type": "boolean"},
   "catalog": {"type": "object"}},
 "required": ["catalogId", "version", "digest", "unchanged"],
 "additionalProperties": false}
```

`catalog` is left out when `unchanged` is true, that is, when `knownDigest` is the current digest. A thread with no
catalog (the web that opened it sent none) answers `isError: true` with the text "this thread has no UI catalog;
answer in text". The call times out after 30 seconds. It reads the thread's catalog ledger and the matching
`ui_catalog` event; the token has already authorised the thread, so there is no further check of a user.

### `turn_output`

An agent says "this is my answer" before it is done ([ADR 0031](../decisions/0031-working-text-and-the-turns-answer.md),
the amendment of 2026-10-02). It serves what the rule of that ADR (the words that end the turn are the answer) misses: an
agent that shows its answer and **keeps working** (it commits, cleans up), and one whose last words are not the answer (the
answer, then a surface drawn, then "there it is"). An agent whose card does not list the extension is unchanged: its last
words are its answer.

Input:

```json
{"type": "object",
 "properties": {"text": {"type": "string", "minLength": 1,
   "description": "Your answer for this turn, as Markdown: 1 to 65536 bytes."}},
 "required": ["text"],
 "additionalProperties": false}
```

Output, as `structuredContent` and as the same JSON in one text content: `{"delivered": true}`
(`{"type": "object", "properties": {"delivered": {"type": "boolean", "const": true}}, "required": ["delivered"],
"additionalProperties": false}`).

The tool's description tells the model: call it once the answer is ready, with the whole answer as Markdown; the person is
shown it as your answer and everything else you said in the turn is kept as working notes; it must be complete on its own
with the result first; then finish with one short line; a later call in the same turn replaces the answer; a call after the
turn is over is an error. Annotations: not read-only, not destructive, **not idempotent**, closed world.

**What it does.** The orchestrator records one `agent_message` by the token's agent (with the revision its binding says):

```json
{"kind": "agent_message", "data": {"messageId": "out-<jti>-<n>", "text": "...", "final": true,
                                    "purpose": "answer", "via": "turn_output"}}
```

`<jti>` is the token's `jti` (the A2A message id) and `<n>` counts the announcements of that token in the turn, from 1.
The AG-UI projection puts `purpose` and `via` on the message's `TEXT_MESSAGE_START` as `vymalo.purpose` and `vymalo.via`
([`agui.md`](agui.md#the-agents-words)).

**When it is accepted.** Only while the thread is `queued` or `working`, for the job the token names (`job` is the
current job), and, once the turn has an announcement, under the token that made it. Otherwise it is a result with
`isError: true` and the text **`this turn is over`**, and nothing is written. The other tool errors say what is wrong:
`text must not be empty` (white space alone is empty), `text must be at most 65536 bytes`, and, for a caller that is not the
thread's addressed agent (`ask:<n>`, slice 10), that only the addressed agent announces its answer. A missing `text`, one
that is not a string and any other member are a protocol error (`-32602`) like any invalid argument. Like every built-in
tool it is cut off after 30 seconds (`temporarily unavailable; try again` when the store is busy: call again).

**A later call replaces the answer.** The log is append-only, so the earlier announcement stays and the rule is the
reader's: **the answer of a turn is the last message marked `answer` in it, and an earlier one is working text**
([`agui.md`](agui.md#the-agents-words)).

**What the rest of the turn becomes.** Once a turn has an announced answer nothing else it says is the answer
(a core rule, ADR 0031): a message of the turn is `purpose: working` whatever the adapter read off its status, and the words
of a `completed`, `input_required` or `auth_required` status that no message said become a `working` message
(`out-<jti>-words-<n>`) ahead of the status, which keeps them as its `detail`. Words that repeat the last words said are
not said again, so an agent whose `completed` carries the answer it announced says it once. A new message, a card's action
or a rework starts a new turn.

**Order.** The call comes by HTTP, the agent's other words by its A2A stream: a sentence stated just before the call may be
logged after the announcement (it is working text either way). An agent finishes only after the tool has answered, so the
announcement is before its closing words.

*Built and tested (2026-10-02):* `crates/surface-thread-tools/tests/turn_output.rs` (accepted, replaced by a second call,
empty and oversize refused, refused once the thread is done or cancelled, refused for a token of another job and for another
token once one announced), `crates/core/tests/turn_output.rs` and the property test of `properties.rs` (the rule over any
sequence), `crates/app/tests/answers.rs`, and the whole loop in `crates/e2e/tests/thread_tools.rs` on both stores with the
fake agent's `turn-output` scripts (the `turn-output` golden of [`examples/`](examples/README.md)).

## Attached servers and the relay (slice 8)

*Written 2026-10-02 on the owner's decisions of plan 11 (the servers come from the YAML configuration, who may attach, the
icons, no doubled steps); contract accepted on the owner's delegation. **Built (2026-10-02):** what is attached and by
whom (the configuration, the events, the API, the message's `attached`) and the relay (the tools on the endpoint, the step
of a call, the error table: `RelayTools`, behind the binary's feature `tool-relay`). It amends the plan of
[ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md) as that ADR's status note of 2026-10-02 says.*

### What is attached, and by whom

- **The deployment lists the servers a person may attach**, in the `toolServers` section of the orchestrator's YAML
  ([`config.md`](config.md), [ADR 0034](../decisions/0034-one-yaml-configuration-secrets-by-reference.md)): per server
  `id`, `name`, `description`, `url` (`http(s)`, no credentials in it), `icon` (a `data:` URI of at most 8 KiB),
  `bearer` (a **secret reference**), `headers` (header name to a **secret reference**; `Authorization`, `Accept`,
  `Content-Type`, `Host`, `Mcp-Session-Id`, `Mcp-Protocol-Version` and `Last-Event-ID` are refused), `tools` (an allow-list
  of upstream tool names, default all), `agents` (the agent ids it may be attached for, default every agent) and
  `timeoutSecs` (1 to 600, default 120). The keys are in [`config.md`](config.md#toolservers) and its schema; the
  loader refuses a bad list at startup (exit 78). A person cannot enter a URL: the list is the deployment's.
- **Who may attach:** anyone with `thread.write` on the thread, for the servers whose `agents` list includes the thread's
  agent. There is no per-role filter yet. The set is at most 16 servers; the thread records `tools_attached` and
  `tools_detached` ([ADR 0004](../decisions/0004-closed-enums-over-dyn-registry.md)) and carries the set from job to job.
  An unknown server, one not allowed for the agent, or more than 16 is a **422**. *Built (2026-10-02):* a person attaches
  when the thread is created (`forwardedProps["vymalo.tools"]`, [`agui.md`](agui.md#attaching-mcp-servers)) and afterwards
  with [`PUT /api/threads/{threadId}/tools`](chat-api.yaml) (`putThreadTools`); the picker reads
  [`GET /api/tool-servers`](chat-api.yaml) (`listToolServers`): id, name, description, icon and the agents it is for, never
  a URL or a credential. A person who may read a thread and not change it (an administrator on another's) gets a 403
  `read_only`; one who may not read it a 404. A server a deployment stops listing stays attached to the threads that have
  it, is not told to the agent and can be detached.
- **Icons are `data:` URIs from the configuration only.** The relay never fetches an icon from a URL and drops the icons an
  upstream server offers (open question 38); the screen draws a `data:` icon and a generic one otherwise.
- **Credentials** live in the orchestrator's configuration and environment, by reference, and appear in no A2A message, no
  event, no outbox row and no log line. The upstream sees `Authorization: Bearer …` only on the orchestrator's own request.

### The tools on the endpoint

The relay is a **provider** of the endpoint (`ThreadToolProvider`). For a request from a caller, it serves the attached
servers allowed for that caller's agent.

- **`tools/list`.** For each such server, the upstream `tools/list` (no cache: each request lists again), the allow-list
  intersected with what the upstream says. A tool is named **`<server>__<tool>`** and left out (with a warning) when that
  name does not fit `^[A-Za-z0-9_-]{1,64}$` (the longest name a model API takes; *unverified* here, the limit of the
  providers the orchestrator's agents use) or the tool's own name starts with `_`. The id has no `_`, so the first `__` is
  the split. `title` is the upstream's, else `"<server name>: <tool>"`; `description` is the upstream's, cut at 8 KiB;
  `inputSchema` is the upstream's (with `"type": "object"` when missing); `outputSchema` and `annotations` are passed on
  (the deployment trusts the servers it lists); `icons` is the configured icon as `[{"src": "data:…"}]`, or none. A server
  that cannot be listed is left out of the answer and logged; it never fails the whole list.
- **Each tool's `_meta`** says what the orchestrator does with a call:

  ```json
  {"_meta": {"thread-tools/v1": {"reportsStep": true, "timeoutSecs": 125}}}
  ```

  | Member | Meaning |
  |---|---|
  | `reportsStep` | `true`: **the orchestrator reports each call of this tool as a step**, with the server's icon ([`steps-v1.md`](steps-v1.md#6-steps-the-orchestrator-reports-itself)). The agent SHOULD NOT report a step of its own for the call: it would be drawn twice. Absent or `false`: the orchestrator reports nothing for this tool, and the agent reports as it likes. Every relayed tool and `ask_agent` say `true`; `get_ui_catalog` and `turn_output` say nothing |
  | `timeoutSecs` | The longest the orchestrator lets a call of this tool run: the server's `timeoutSecs` plus 5 for a relayed tool, the ask's timeout plus 30 for `ask_agent`. The agent SHOULD use it as the time it waits for the call, in place of a fixed default, **capped by its own limit** (adam-rs: `THREAD_TOOLS_MAX_CALL_SECS`, default 3600; 60 seconds today for every call). Absent: the agent's default |

  **The key.** MCP reserves `_meta` for protocol metadata and gives its keys a format: an optional prefix of dot-separated
  labels followed by a slash, then a name (*verified 2026-10-02*, [below](#verified-and-unverified-2026-10-02)). The full URI
  of this extension is not a key of that format, so the key here is **`thread-tools/v1`** (prefix `thread-tools`, name `v1`,
  not a reserved prefix). The A2A message's metadata, which A2A leaves to extensions, keeps the full URI.
- **`tools/call`.** The request's `_meta["thread-tools/v1"]` may carry, both optional and at most 256 bytes each:

  | Member | Meaning |
  |---|---|
  | `callId` | The agent's own id for this call, **stable across a retry of the same call** (adam-rs journals it, so a step retried after its lease expired sends the same one). The step the orchestrator reports takes its id from it, so a retry reports the same step again, not a second one. For `ask_agent` it is the [dedupe key](#dedupe-by-call-key) |
  | `parentStepId` | The [`steps/v1`](steps-v1.md) id (the agent's own, as it reported it) of the step this call runs under. The orchestrator's step nests under it. Absent: under the caller's top, the agent's invocation (or the ask's step, for an asked agent) |

  A relayed call is **at least once** on a retry: the orchestrator does not hold a result, so a retried call of a tool with
  side effects is made again upstream. Whether that is safe is the tool's, as for any MCP client that retries.

### The step of a call

For a tool that says `reportsStep`, the orchestrator records a step for each call through the same input an agent's step
takes (`Input::Step`, [steps-v1 section 6](steps-v1.md#6-steps-the-orchestrator-reports-itself)), attributed to the calling
agent: `kind: tool`, `label` `"<server name> · <tool>"`, `icon: "mcp-server:<id>"` (an agent cannot claim that icon),
`state: running` when the call starts, and the same id again with `completed`, `failed` or `canceled` when it ends, with
`detail` the public error (at most 200 characters) when it failed. **The step carries its `input` and `output`**
([ADR 0030](../decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md), which overrides the earlier
plan that a relayed call's arguments and results are not logged): the `input` is the call's arguments and the `output` the
result's text, under the same bounds and redaction, and **no credential is ever in either**. The agent receives the
**whole** result (up to 256 KiB); the step keeps its 8 KiB. A step that cannot be committed (a busy store) is logged and does
not fail the call. Exactly one step is reported per call.

This is what makes the existing rule of [`steps-v1.md`](steps-v1.md#3-the-report) enforceable: an agent that sees
`reportsStep: true` knows the orchestrator reports the call, and one that sees nothing knows it does not. The orchestrator
does not merge or drop a step an agent reports (steps are data from the agent, steps-v1 section 3): an agent that reports
one anyway has two lines on screen.

### Errors of a call

| Situation | The agent gets | Step |
|---|---|---|
| The name is not on the endpoint: unknown, detached, left out by the allow-list or the agent filter | JSON-RPC `-32602` (unknown tool) | none |
| The caller's task is over (the thread is terminal, the job is not the token's, the ask finished) | a result with `isError: true`, "this task is over" | none |
| The server cannot be reached, or the connection times out | `isError`: "the MCP server '<name>' could not be reached; the call did not run" | `failed` |
| No answer within the server's timeout | `isError`: "no answer within N s; it may still be running on the server" | `failed` |
| The server answers 401 or 403 | `isError`: "the MCP server refused the orchestrator's credentials" (and the operator's log has a warning) | `failed` |
| The server answers with a JSON-RPC error | `isError` with its message, at most 1 KiB | `failed` |
| The server's result has `isError: true` | passed through as it is | `failed`, `detail` its first line |
| The result is over 256 KiB | the content cut, with a note appended | `completed` |
| The agent cancels the call or drops the connection | the upstream call is dropped | `canceled` |

A server that the deployment no longer lists, or whose credentials were rotated, fails like the rows above: the thread keeps
the attachment, and the failure names the server, never a credential.

### As built

What the contract left open, as `RelayTools` does it (*verified 2026-10-02, by this repository's tests*,
`crates/surface-thread-tools/tests/relay.rs` over `MemoryToolServers` and `crates/e2e/tests/tool_relay.rs` over the real
MCP client and `FakeToolServer`s, on both stores):

- **What is the relay's to answer.** A name is the relay's when it is `<server>__<tool>` of a server the deployment lists,
  offered for the caller's agent, **attached to the thread**, whose allow-list (if any) names the tool, and that fits the
  name rule. Anything else is not owned by any provider, so the endpoint answers `-32602`: a detached server, one not
  offered for the agent, a tool left out by the allow-list, `<server>___x` (a tool name starting with `_`). Nothing reaches
  a server and no step is written. A tool that **no allow-list names and the server does not have** is the server's to
  refuse: the call goes upstream and its JSON-RPC error is the row "a JSON-RPC error" (a failed step), because the relay
  does not list before it calls.
- **The step's id** is `tool-<callId>` when the request's `callId` is usable (a string of 1 to 256 bytes with no control
  character, and short enough for a step id), so a retried call is the same step again (the ledger coalesces it; the call is
  made again upstream, as said above); otherwise `tool-<uuid>`. `parentStepId` is the agent's own id, which the adapter
  prefixed with the task id when it logged the agent's steps, so the relay's step nests under `<task id>/<parentStepId>`
  (the thread's binding names the task); without a task or a parent the step is at the top, which is where the agent's own
  top steps are (the log has no step for the agent's invocation).
- **The step is attributed** to the calling agent with the revision that serves its task, and is one `start` and one `end`
  (a step that cannot be recorded is logged, never fails the call). In `blocked` and `verifying` the core drops a step, so a
  call made then has none; in a finished thread the call is not made at all (the "task is over" row).
- **A call the agent drops.** A connection that closes drops the call's future: a guard records the `canceled` end from a
  task of its own. A cancellation notification ends the call the same way. The upstream request is dropped either way (the
  server may run it to the end).
- **Wording.** Beyond the table: a server that does not answer as MCP is "the MCP server '<name>' did not answer as MCP", an
  endpoint that cannot be used "... is not set up for use" (both `failed`). A result over 256 KiB has a text block appended:
  `[the result is longer than 256 KiB and was cut here]`; the step's `output` is the text, cut to 8 KiB with `truncated`.
- **Bounds.** A call is also bounded by the server's `timeoutSecs` plus 5 s, whatever the client does, so the
  `timeoutSecs` of the tool's `_meta` (the server's plus 5) is an upper bound the agent can wait for.
- **No credential** is in a step, an event, a frame, the export, any table of the database or a log line: the tests search
  all of them for the bearer and a header value (`tables_mentioning`), and for the thread token.

```mermaid
sequenceDiagram
  autonumber
  participant A as Agent
  participant E as Thread tools endpoint (relay)
  participant L as Event log
  participant S as MCP server (credentials held here)

  A->>E: tools/list
  E->>S: tools/list (the server's bearer)
  S-->>E: search
  E-->>A: websearch__search, _meta {reportsStep: true, timeoutSecs: 125}
  A->>E: tools/call websearch__search {query}, _meta {callId, parentStepId}
  E->>L: step running {icon: mcp-server:websearch, input: {query}} (agent_step start)
  E->>S: tools/call search {query}
  S-->>E: result
  E->>L: step completed {output: the result's text, cut to 8 KiB} (agent_step end)
  E-->>A: the whole result (up to 256 KiB)
  Note over A,E: the agent reports no step of its own for this call
```

```mermaid
stateDiagram-v2
  [*] --> Running: tools/call accepted, the step starts
  Running --> Completed: the server answered
  Running --> Failed: unreachable, timeout, 401/403, a JSON-RPC error or isError
  Running --> Canceled: the agent cancelled or dropped the call
  Completed --> [*]
  Failed --> [*]
  Canceled --> [*]
```

(An unknown name or a task that is over gets no step: the first is `-32602`, the second a result with `isError`.)

## `ask_agent`

*Slice 10. Written 2026-10-02; contract accepted on the owner's delegation; **not built**. Decided in the status note of
[ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md) (option A: the addressed agent coordinates); the
references it works from are [`mentions-v1.md`](mentions-v1.md).*

The addressed agent asks an agent the person **mentioned** to do part of the work and waits for its answer. The orchestrator
runs the asked agent as a **child task of the same thread**, in a context of its own, with its steps nested under the step of
the agent that asked, and returns its result to the tool call with progress notifications.

The provider owns the name `ask_agent` for every caller. `tools/list` includes it only when the job has mentioned agents
and the caller may still ask (its depth is below the limit); a call that arrives when it is not offered (a list that went
stale during a turn) is a **result with `isError`** that says why, not `-32602`, so the model reads the reason.

### Input

```json
{"name": "ask_agent",
 "title": "Ask a mentioned agent",
 "description": "Ask one of the agents the person mentioned to do part of the work and wait for its answer. The agent does not see this conversation: put everything it needs in message. Asking the same agent again continues its conversation, and answers its question if it asked one.",
 "inputSchema": {"type": "object", "additionalProperties": false, "required": ["agent", "message"], "properties": {
   "agent": {"type": "string", "description": "the agentId of a mention"},
   "message": {"type": "string", "minLength": 1, "maxLength": 16000},
   "timeout_secs": {"type": "integer", "minimum": 10, "maximum": 7200,
                    "description": "Lowers the ask's timeout; it never raises the deployment's"}}},
 "_meta": {"thread-tools/v1": {"reportsStep": true, "timeoutSecs": 1830}}}
```

Annotations: not read-only, not destructive, **not idempotent** (the same call key is, below), open world. A missing or
mistyped argument or an unknown member is `-32602` like any invalid argument.

### Result

As `structuredContent` and as the same JSON in one text content:

```json
{"ask": 2, "agent": "mock-browser", "state": "completed",
 "text": "Pictures: …", "artifacts": [{"name": "pitch.png", "uri": "…", "mimeType": "image/png"}]}
```

| Member | Meaning |
|---|---|
| `ask` | The number of the ask in the job, from 1 |
| `agent` | The `agentId` asked |
| `state` | `completed`, `input_required`, `auth_required`, `failed`, `rejected`, `canceled` or `timed_out` |
| `text` | The asked agent's last words, at most 64 KiB, **untrusted text** in the asker's model |
| `artifacts` | At most 20 `{name, uri?, mimeType?}`; a file is a reference to the thread's artifact store ([ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md)), never its bytes |
| `question` | With `input_required` or `auth_required`: what the asked agent asks. Asking the same agent again **continues that task** with the answer |
| `error` | With a failure: the public reason |

`isError` is true unless the state is `completed`, `input_required` or `auth_required`.

### Refusals

A refusal is a result with `isError: true`, **nothing written**:

| Situation | Text |
|---|---|
| The agent is not mentioned in this job | "you can ask only the agents the person mentioned: a, b" |
| The agent is the asker or one of its own ask chain | "an agent cannot ask itself or an agent already in its chain" |
| The depth limit is reached | "asks are nested at most N deep" |
| The job's limit of asks is reached | "this job has used its N asks" |
| The thread's limit of running asks is reached | "N asks are already running; wait for one to finish" |
| The agent is no longer listed | "agent '<id>' is no longer listed" |
| The person may not invoke the agent (`agent.invoke`, checked again here) | "the person may not use '<id>'" |
| The registry cannot answer | "the agent list is unavailable; try again" |
| The caller's task is over | "this task is over" |
| The call key was used for another ask | "this callId was used for another ask" |

### Limits

| Limit | Default | Range | Configuration |
|---|---|---|---|
| Depth: the asked agent's depth is its asker's plus 1; the addressed agent is 0 | **2** | 1 to 4 | `asks.maxDepth` |
| Asks per job | **16** | 1 to 64 | `asks.maxPerJob` |
| Asks running at once per thread | **4** | 1 to 16 | `asks.maxRunning` |
| Timeout of an ask | **1800 s** | 10 to 7200 | `asks.timeoutSecs` |
| `message` | 16,000 characters | fixed | |

An ask whose depth would pass `asks.maxDepth` is refused: with the default, the addressed agent asks A (depth 1), A may ask B
(depth 2), and B cannot ask. The keys belong to the `asks` section of the orchestrator's YAML; `config.md` and its schema
are written by the pull request that builds them. The core never reads a configuration: the limits are passed in the input
that records the ask ([ADR 0004](../decisions/0004-closed-enums-over-dyn-registry.md)).

### Dedupe by call key

The **call key** is `ask:<thread>:<caller>:<callId>`, from the request's `_meta["thread-tools/v1"].callId`. A second call with
the same key **re-attaches to the ask** instead of starting another: a running ask is waited for again, a finished one is
answered at once with its recorded result. This is why adam-rs sends a stable `callId`: a step retried after its lease
expired calls again, and a **dropped connection does not cancel the ask**, so the agent re-attaches by calling again. A call
with no `callId` has no key and is a new ask every time; an agent SHOULD send one. A second call with the same key and a
different `agent` or `message` is refused (the last row above).

### The child task

- **Ledger.** The job keeps `asks`, one entry per ask: its number, the agent, who asked (`main` or `ask:<m>`), its depth, its
  context `<thread>-ask-<agent>`, its A2A task id once it has one, its state, and the call key. It is reset when the next job
  starts. What the asked agent said lives in events, not in the ledger.
- **Events.** `ask_started {ask, agent, by, depth, text (at most 16 KiB, untrusted), stepId: "ask-<n>", parentStepId?}`,
  attributed to the asking agent; `ask_finished {ask, state, text?, question?, artifacts?, error?}`, attributed to the
  asked agent (the system for `timed_out`, a cancel or a delivery failure). Their schemas are added to
  [`chat-api.yaml`](chat-api.yaml) by the pull request that builds them. The asked agent's progress is `agent_step`
  events with ids `ask-<n>/<its id>` under the top step `ask-<n>`; partial messages are not logged.
- **The asked agent's token** has `caller = ask:<n>` and a `depth`. It gets the attached servers allowed for **it**, and
  `ask_agent` only while its depth is below the limit; it gets **no** `get_ui_catalog`, no `turn_output` (only the addressed
  agent announces the turn's answer) and no mentions metadata.
- **Continuation.** Asking again is a new ask (a new number). An ask to an agent whose last ask in this job ended `input_required` or `auth_required` continues that
  task. Otherwise it is a new task in the same context with `referenceTaskIds` naming the earlier ask tasks to that agent in
  this job ([ADR 0021](../decisions/0021-context-across-a2a-tasks.md)).
- **The gate never sees an asked agent.** Its `branch`, `checks` and pushed commits do not reach the job's gate
  ([ADR 0018](../decisions/0018-verification-gate-and-rework-loop.md)): the addressed agent's work is what is verified. An
  asked agent cannot show a surface: an A2UI part from it is refused with an `error` step.
- **Nesting.** The ask is a **subagent** step `ask-<n>` under the step of the agent that asked (`parentStepId` when given), and
  its own steps nest under it ([ADR 0025](../decisions/0025-nested-steps-events-carry-their-source-path.md)). In AG-UI it is
  a `SUBAGENT_STARTED` named `sub-ask-<n>` whose parent is the asker's invocation, with a `vymalo.ask` activity; the rows
  are written into [`agui.md`](agui.md) by the pull request that builds them.
- **Waiting.** After the ask is recorded (or found, for a duplicate), the call follows the thread's events until
  `ask_finished` for this ask. With a `progressToken` it sends a progress notification for each step of the ask, and a
  heartbeat every 30 seconds in any case; the HTTP wait is bounded by the ask's deadline plus 30 seconds.
- **Ending.** At the deadline a running ask ends `timed_out` and the asked agent's task is cancelled. **The person's Cancel and
  a Stop & send** ([ADR 0036](../decisions/0036-sending-while-an-agent-works.md)) cancel every running ask, which ends
  `canceled` ("the person stopped the job"); the addressed agent's task reaching a terminal state does the same ("the asking
  task ended"). An update from an asked agent after its `ask_finished` is dropped like any late input.

```mermaid
sequenceDiagram
  autonumber
  participant A as Addressed agent
  participant E as Thread tools endpoint (asks provider)
  participant C as Core and event log
  participant D as Dispatcher (ask row)
  participant B as Asked agent

  A->>E: tools/call ask_agent {agent, message}, _meta {callId, parentStepId}
  E->>E: checks: mentioned, depth, limits, may invoke, call key
  E->>C: Input::Ask: ask_started {ask 1, by main, depth 1}, an outbox row of kind ask
  Note over E,A: the call waits and sends progress and a heartbeat
  D->>B: new task in context <thread>-ask-<agent>, token caller ask:1
  B-->>D: working, steps nested under ask-1
  D->>C: agent_step events (ask-1/...)
  B-->>D: completed, "Pictures: ..."
  D->>C: ask_finished {ask 1, completed, text}
  E-->>A: result {ask 1, state completed, text, artifacts}
  A->>E: the same callId again (a retry)
  E-->>A: the recorded result at once, no second ask
```

```mermaid
stateDiagram-v2
  [*] --> Running: ask_started (the checks passed)
  Running --> Running: the asked agent works: steps, progress to the call
  Running --> Finished: the asked agent's task completed, failed, was rejected or asked
  Running --> Finished: the deadline passed: timed_out, the task is cancelled
  Running --> Finished: the person stopped the job, or the asking task ended: canceled
  Running --> Finished: the delegation could not be delivered: failed
  Finished --> [*]: ask_finished, written once
```

## Security notes

- A token is a **capability for one thread** until it expires: whoever holds it can call that thread's tools. With the
  relay and `ask_agent` that is more than reading a catalog (a relayed call uses the server's credentials; an ask runs
  another agent), which is why the token is scoped to one thread and one caller, and why it never leaves the A2A
  message, the agent and the calls it makes.
- The key is deployment configuration, like the webhook secrets: anyone with it can mint a token for any thread. Keep
  it out of images, logs and `Debug` output; rotate it as above.
- What a tool returns goes back into an agent's model: a relayed result or an asked agent's answer is untrusted text
  there, as any tool result is (prompt injection). The orchestrator does not interpret it.
- **The relay is an outbound surface.** The servers come only from the deployment's configuration, never from a person or
  an agent; a response is cut at 256 KiB; a credential is never logged or stored (the relay's tests search every table of the database, the event log, the export and
  the frames for the bearer); a server's result and an asked agent's answer are untrusted text in the agent's model.
- **An ask runs another agent** on the person's behalf, so each ask is checked against what the person may invoke
  (`agent.invoke`) when it is made, not only when the agent was mentioned, and an asked agent never reaches the gate.
- The endpoint has no browser use, so it needs no cookie and no CORS; it must not be routed from the public edge.

## Verified and unverified (2026-10-01)

*Verified 2026-10-01:*

- HS256 needs a key "of the same size as the hash output (for instance, 256 bits for HS256) or larger", hence 32 bytes
  (RFC 7518 section 3.2, <https://www.rfc-editor.org/rfc/rfc7518.html#section-3.2>).
- A bearer token that is "expired, revoked, malformed, or invalid for other reasons" is answered with 401 and the
  error `invalid_token`, and a request that "lacks any authentication information" gets no error code (RFC 6750
  section 3.1, <https://www.rfc-editor.org/rfc/rfc6750.html#section-3.1>).
- `jti` "provides a unique identifier for the JWT" (RFC 7519 section 4.1.7); a message id, one token per message, serves.
- MCP streamable HTTP: a server "MAY assign a session ID", so a stateless server is allowed; servers "MUST validate
  the Origin header" on all incoming connections (MCP specification 2025-11-25, transports,
  <https://modelcontextprotocol.io/specification/2025-11-25/basic/transports>). The Host check named in
  `THREAD_TOOLS_ALLOWED_HOSTS` is the orchestrator's reading of that rule for agent-to-server calls (the library's
  own `Origin` check is on, with no origin allowed: see below).
- MCP tools: `structuredContent` should be accompanied by the same JSON as text; an unknown tool is a protocol error
  (`-32602`); a tool's own failure is a result with `isError: true` (MCP specification 2025-11-25, tools,
  <https://modelcontextprotocol.io/specification/2025-11-25/server/tools>).

*Verified 2026-10-01, by this repository's tests* (`crates/surface-thread-tools/tests/`, rmcp 3.5.0): the MCP server
library serves a stateless service on the exact parametrised path `/thread-tools/{threadId}/mcp` (it ignores the path
it is called at; the guard reads the thread from it); its Host check refuses a host that is not listed with `403`; with
the list of allowed origins empty, a request that carries an `Origin` header is refused with `403` (so the library's
check does cover `Origin`, as configured here); a request with no `Authorization` header, a bad token and a good token
for another thread are the two `401` answers above, through the library's own middleware order (the guard runs first).

## Verified and unverified (2026-10-02)

*Verified 2026-10-02* (MCP specification 2025-11-25, <https://modelcontextprotocol.io/specification/2025-11-25/basic>,
"General fields"):

- `_meta` "is reserved by MCP to allow clients and servers to attach additional metadata to their interactions". A key has
  an optional **prefix**, "a series of labels separated by dots (`.`), followed by a slash (`/`)", and a **name** that
  begins and ends with an alphanumeric character and may hold hyphens, underscores and dots. A prefix whose second label is
  `modelcontextprotocol` or `mcp` is reserved. So `thread-tools/v1` is a valid key and the URI of the extension is not.
- A tool definition may carry `icons` whose `src` is an HTTP(S) URL or a base64 `data:` URI, and consumers "MUST" treat
  icon bytes as untrusted, fetch without credentials and keep a strict allow-list of image types; a client that renders
  icons must support PNG and JPEG and should support SVG and WebP (MCP specification 2025-11-25, "icons"). The relay uses
  only a configured `data:` icon.
- Tool names "SHOULD" be 1 to 128 characters of letters, digits, `_`, `-` and `.`
  (<https://modelcontextprotocol.io/specification/2025-11-25/server/tools>); the relay's names are stricter (64
  characters, no dot).
- An unknown tool is a protocol error (`-32602`) and a tool's own failure is a result with `isError: true`; clients
  "SHOULD" give tool execution errors to the model, and "MAY" give protocol errors (same page).

*Unverified:* that 64 characters is the limit of every model API an agent here uses; that an `ask_agent` call of half an
hour survives every proxy between an agent and the endpoint (the heartbeat every 30 seconds is the mitigation, and the
agent re-attaches by its `callId`).
