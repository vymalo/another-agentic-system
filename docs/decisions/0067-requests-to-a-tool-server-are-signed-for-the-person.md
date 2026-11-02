# ADR 0067 — Requests to a tool server are signed for the person: MCP `2026-07-28`, one header, the thread's owner

- **Status:** **proposed (2026-11-02).** Asked for by vaam-apps, whose agent will answer a buyer's conversation through this
  orchestrator and call vaam's tools **as the person who owns the thread**; the approach was agreed with its owner on 2026-11-02.
  **Built in this change** (the contract page first, then the code): [`api/signed-tool-requests-v1.md`](../api/signed-tool-requests-v1.md),
  `orch-signed-request`, the MCP client, the relay and `toolServers[].protocol` and `toolServers[].signing`. **Proposed**, for the
  maintainers to confirm: every name, limit and default below. No tool server verifies the signature yet; the first one, vaam's, is
  written against the contract page.
  Builds on [ADR 0024](0024-mcp-tools-attached-per-conversation.md) (the relay and the servers a deployment lists),
  [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md) (the orchestrator knows who the person is) and
  [ADR 0034](0034-one-yaml-configuration-secrets-by-reference.md) (a key is a reference).

## Context

vaam-apps serves its tools on a cratestack 0.17.2 MCP endpoint. Its admission is a signature, not a bearer: a request is admitted when
it carries a `COSE_Sign1` claim object from a key the endpoint pins, and the endpoint then acts for the person the object names.
Two things stopped the relay from calling it. *Verified 2026-11-02 by reading the code at `9afaad3`, `rmcp` 3.5.0 in `~/.cargo/registry`, and
cratestack at the tag `v0.17.2` (`ec92825`):*

| What | Where |
|---|---|
| The relay's client opened every session with the legacy `initialize` (`serve`), and a cratestack 0.17 MCP endpoint speaks only MCP `2026-07-28` and refuses a legacy `initialize` with `-32022` | `orchestrator/crates/tools-mcp/src/lib.rs` (`open`); cratestack `crates/cratestack-mcp/src/server/discovery.rs` (`SUPPORTED`, `initialized`) |
| `rmcp` 3.5.0 **has** the other handshake: `ClientLifecycleMode::Discover { preferred_versions }` and `ClientServiceExt::serve_with_lifecycle` open with `server/discover`, send the protocol version in the `_meta` of every request, and the streamable HTTP transport sends the `MCP-Protocol-Version` and the standard `Mcp-*` headers. So nothing is forked and no upgrade is needed | `rmcp-3.5.0/src/service/client.rs` (`discover_startup`, `ClientLifecycleMode`), `src/transport/streamable_http_client.rs` |
| The relay forwarded `ToolCall::new(tool).with_arguments(args)` and nothing else, with one static bearer and static headers for every thread and person: the server could learn who *the orchestrator* was, never the person | `orchestrator/crates/surface-thread-tools/src/relay.rs` (`relay`), `crates/tools-mcp/src/lib.rs` (`transport_config`) |
| The relay *has* the person: `ToolCtx.owner` is the thread's owner and `ToolCtx.claims.thread` its id | `crates/surface-thread-tools/src/provider.rs` |
| A cratestack MCP guard hands an `AuthProvider` the method, the resource's path, the query and the **exact body bytes**, and, with `CredentialPresence::ask_provider()`, asks the provider whether a request carries its credential before it reads the body | `crates/cratestack-mcp/src/streamable_http/{guard,auth,credential,presence}.rs` |
| `cratestack-cose` 0.17.2 has `rust-version = "1.98.0"`; this workspace builds on 1.94.1 (`rust-toolchain.toml`, `Cargo.toml`, `Dockerfile`, six workflows) | `cargo info cratestack-cose`, `orchestrator/rust-toolchain.toml` |

Invariants (`CLAUDE.md`), checked first. **1, protocols only:** MCP over streamable HTTP and COSE (RFC 9052), both standards; no host SDK.
**2, optional extensions:** a server that is not configured for it is called exactly as before; the new keys are optional. **3, stateless:**
a request is signed when it is sent and nothing is kept; the replay record is the tool server's. **5, the core is pure:** untouched.
**6, swappable implementations:** the port gains data, not a method; the signer is a pure crate and the client its one user.

## Decision

1. **A `toolServers` entry can say which MCP revision the server speaks.** `protocol: legacy` (the default) opens a session with
   `initialize`, as before. `protocol: "2026-07-28"` opens with **`server/discover`**, offers that revision only
   (`ClientLifecycleMode::Discover`) and **does not fall back**: a server that is not one is an error (`Protocol`). The port carries it as a
   closed `ToolProtocol` on the endpoint. *Rejected: the `Auto` mode (probe with `server/discover`, fall back to `initialize`): a failed
   discovery would silently become a different conversation, and every legacy server would cost one extra request for it.*
2. **A `toolServers` entry can say its requests are signed**, in place of a bearer (`signing: { key, audience, ttlSecs }`, refused
   together with `bearer`). **Every HTTP request** to it carries one header, `Tool-Request-Claims`, made just before the bytes leave and
   over those exact bytes: the body is serialised once, hashed, signed and sent as it is, which is why a signed session is always built
   by the crate's own HTTP backend (`wire.rs`) and never by `rmcp`'s, which hands the message to the HTTP client to serialise.
3. **The object is the claim object of `cratestack-cose` 0.17.2**, byte for byte (`sign_claims`, which `open_claims` verifies): a tagged
   `COSE_Sign1`, Ed25519, `typ` `application/vnd.vymalo.tool-request.v1+cwt`, the CWT claims `aud`, `exp` (at most 60 seconds after `iat`,
   30 by default), `iat` and a fresh 16-byte `cti`, and a JSON payload of five members: `method`, `path`, `body_sha256`, `thread`,
   `owner`. The issuer is the `kid`, which the tool server pins as a public key: cratestack's profile refuses any other CWT claim, so
   there is no separate `iss`. The wire contract, with a worked example, is [`api/signed-tool-requests-v1.md`](../api/signed-tool-requests-v1.md),
   written before the code; a receiving side is built against it.
4. **The person is the thread's owner, never the caller.** The relay adds `(thread, owner)` to a **copy** of the endpoint for each request
   (`ToolServerEndpoint::with_acting_for`, from `ToolCtx`) when, and only when, the server is signed; any other server is called with the
   endpoint itself and learns neither. A signed endpoint with no thread, a key that is not an Ed25519 PKCS#8 PEM, an owner or a path over
   the limits, a bearer beside it, or a URL with a query is `Misconfigured` and **nothing is sent, and never an unsigned request**. The key is
   checked when the process starts (exit 78, naming `toolServers[i].signing.key`) and again by the client.
5. **The key is a reference** (`{ env }` or `{ file }`, ADR 0034): the seventeenth secret of the schema. One key per tool server; the
   server pins up to two public keys, so a rotation has a step in which both verify (the contract page has the order).
6. **The writer lives in this repository, not behind a dependency on `cratestack-cose`.** `orch-signed-request` writes the bytes
   itself (about 150 lines of deterministic CBOR and an Ed25519 signature through `aws-lc-rs`, already in the tree as rustls' provider) and is
   held to the real crate three ways: a **known-answer vector** produced by the real `sign_claims` (the test fails on one byte), a reference
   verifier behind the feature `verify` that the client's tests use, and `crates/signed-request/interop`, a crate with its own toolchain
   (1.98.1) that runs the real `open_claims` on the orchestrator's bytes and the real `sign_claims` on the same inputs, in its own CI job.
   *Rejected: depend on `cratestack-cose` and move this workspace to Rust 1.98: it is a toolchain change of the Dockerfile, six workflows and
   the adam-rs pin, not part of this one; when the toolchain moves, the writer can be replaced by the crate and the interoperability crate
   becomes an ordinary test.*

## What is not decided here (follow-ups)

- **A listener per surface** (`/agui` and `/thread-tools` on separate ports or hosts, the family of vaam's EXT-19) and, behind it, allowing
  `auth.mode: proxy_header` in production (`config.md`, "Authentication"). The thread's owner is only as good as the identity the
  orchestrator holds for the thread; this ADR makes that identity travel to the tool server, it does not change how it is established.
- **The A2A agents' static bearer and the HMAC thread-tools token** (an agent calling the orchestrator) moving to signatures (EXT-19). They are
  unchanged.
- Signing for the servers **people add** (`u-` ids, ADR 0046): their credentials stay a leased bearer or key; `protocol` and `signing` are keys
  of the deployment's list only.

## Consequences

- **Easier:** a tool server can tell which person a call is for, and that the orchestrator said so for *this* request; a captured header is worth
  one request for at most a minute; no secret of the orchestrator's is held by a tool server, which pins a public key.
- **Required elsewhere:** a tool server that takes signed requests records the `cti` of every accepted request until `exp` plus its skew, and
  authorises the owner itself; its clock agrees with the orchestrator's within 30 seconds. A deployment that turns `signing` on creates a key
  and gives the public half to the tool server's operator.
- **Harder:** a second place writes the bytes of a format whose reference is another repository's. The known-answer vector and the
  interoperability job are what keep the two honest, and the job needs a second toolchain in CI until the workspace moves to 1.98.
- **Risk:** the key holder can name any owner on its server. The contract says so; the key is one per server, held by reference, and rotated
  by the steps on the contract page.

## Verified and unverified (2026-11-02)

| Claim | Status |
|---|---|
| `rmcp` 3.5.0's client opens with `server/discover` in `Discover` mode and works over its streamable HTTP transport with this crate's own HTTP backend | **Verified 2026-11-02** by `orch-tools-mcp`'s `tests/signed.rs`, a real MCP server over HTTP that speaks only `2026-07-28` |
| The bytes are those of the real `sign_claims`, and the real `open_claims` opens them | **Verified 2026-11-02**: the known-answer test, and `crates/signed-request/interop` run locally against `cratestack-cose` 0.17.2 |
| The interoperability job passes in this repository's CI | **Unverified** until the workflow has run |
| A cratestack MCP endpoint on vaam's side admits the header with `ask_provider()` and `open_claims` | **Unverified**: the library calls are verified; vaam's endpoint is not built |
| The first deployment's clocks agree within 30 seconds | **Unverified** (NTP) |
