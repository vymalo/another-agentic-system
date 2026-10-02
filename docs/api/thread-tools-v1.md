# A2A extension: thread tools (v1)

- **URI:** `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`
- **Status:** **built (MVP slice 3, 2026-10-01), apart from the tools of later slices.** Accepted on the owner's
  delegation; the owner may revisit anything here. Built: `orch-thread-token` (the token, with the known-answer vectors
  below), `orch-surface-thread-tools` (the route, the guard, `get_ui_catalog`, the seam for later tools), the binary's
  `thread-tools` surface and `THREAD_TOOLS_*` settings, and the grant in the A2A message (the adapter mints at send
  time, only for an agent whose live card lists the extension). `turn_output` (an agent announces its answer,
  [ADR 0031](../decisions/0031-working-text-and-the-turns-answer.md) amendment of 2026-10-02) is built, below. Slice 8 adds the relayed tools of attached MCP servers
  and the `attached` member of the message; slice 10 adds `ask_agent` and the `ask:<n>` ledger
  ([`mvp.md`](../mvp.md#the-new-build-order)). "Not yet" is marked where it matters below. The adam-rs side (an agent
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
lifetime is long (below). Slice 8 adds a member that names the attached servers (their ids and display names; never a
URL or a credential); its shape is written with that slice.

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
| 8 | `<server>__<tool>`: the tools of each MCP server attached to the thread, relayed. The orchestrator holds the servers' credentials (from its configuration and environment), sees each call and reports it as a tool step with the server's icon. | relay | written with slice 8 ([ADR 0024](../decisions/0024-mcp-tools-attached-per-conversation.md), status note) |
| 10 | `ask_agent`: the addressed agent asks a mentioned agent. The orchestrator runs it as a nested child task on the same thread, its steps under the step of the agent that asked, and returns its result to the call, with progress notifications. The asked agent's own token has `caller = ask:<n>` and a `depth`. | asks | written with slice 10 ([ADR 0026](../decisions/0026-agent-mentions-as-structured-references.md), status note) |

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

## Security notes

- A token is a **capability for one thread** until it expires: whoever holds it can call that thread's tools. With the
  relay and `ask_agent` that is more than reading a catalog (a relayed call uses the server's credentials; an ask runs
  another agent), which is why the token is scoped to one thread and one caller, and why it never leaves the A2A
  message, the agent and the calls it makes.
- The key is deployment configuration, like the webhook secrets: anyone with it can mint a token for any thread. Keep
  it out of images, logs and `Debug` output; rotate it as above.
- What a tool returns goes back into an agent's model: a relayed result or an asked agent's answer is untrusted text
  there, as any tool result is (prompt injection). The orchestrator does not interpret it.
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
