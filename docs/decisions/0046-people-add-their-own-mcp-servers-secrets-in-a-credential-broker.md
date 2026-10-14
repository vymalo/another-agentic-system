# ADR 0046 — People add their own MCP servers from the UI; their secrets live in a credential broker, never here

- **Status:** **accepted (2026-10-11)**, on the owner's words of that day: "ADR 0046 should be accepted and built." Every name, default and
  shape marked *proposed* below is **accepted as written**, except what the amendment of 2026-10-11 amends (decision 3: the broker is in the
  orchestrator, behind two ports) and the few points it keeps open (its last section). **Nothing of this is built yet** *(as written on 2026-10-11; slice 1 is built since, see the amendment of 2026-10-14 at the end)*; the owner asked for it to
  be built. The amendment also reads the title's "never here" as *never in the log, an export, a frame, a file or in clear*: a sealed secret is
  now kept here (amendment, and invariant 3's note in `CLAUDE.md`).
  *Amended 2026-10-14: slice 1 of the amendment's list is built (the ports, their testkits, the in-memory stores and the egress guard); see the last section.*
  *Amended 2026-10-14: slice 2 is built too (`orch-credential-broker` and `credential_broker_conformance!`); see the section "Amended 2026-10-14, slice 2" at the end.*
  *Amended 2026-10-14: slice 3 is built too (`orch-secrets-pg`, the `secrets` configuration section, `orchestrator secrets rewrap` and the chart's key, off by default); see the section "Amended 2026-10-14, slice 3" at the end.*

  *Earlier status (2026-10-06, kept as written):* proposed (2026-10-06). **Accepted** on the owner's decisions of 2026-10-06: decisions 1 to 4 below, in
  substance (an agent's own servers are fixed; people add their own, with four kinds of authentication, stored per person, "we'll mimic how LibreChat is
  doing their stuffs"; secrets go to a credential broker, never into the log; a user server is relayed like today's web search and fails closed; a person's
  URL is untrusted). **Proposed**, for the owner to confirm: every name, default and shape marked *proposed*, which are the ports, the permission, the
  configuration, the endpoints and the lifecycle. **Nothing of this is built.** Extends [ADR 0024](0024-mcp-tools-attached-per-conversation.md) (the relay)
  and [ADR 0045](0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md) (permissions); amends invariant 3 of `CLAUDE.md` for the
  definitions of decision 2. *Amended 2026-10-06:* the link to the platform's broker (decision 3) now names its section.

## Context

Today a person attaches only what the deployment lists in `toolServers` ([`config.md`](../api/config.md#toolservers)), with
credentials from the orchestrator's own configuration, and the orchestrator relays the server's tools to the agent on the
thread's endpoint, `thread-tools/v1` ([`thread-tools-v1.md`](../api/thread-tools-v1.md#attached-servers-and-the-relay-slice-8),
`dev/tools-e2e.sh`). "A person cannot enter a URL: the list is the deployment's." The owner wants people to bring their own
servers, with their own credentials, as LibreChat does.

What LibreChat does, as far as this ADR copies it (*verified 2026-10-06*, page fetches; the tool summarised the pages, so a
wording below is a paraphrase unless quoted):

- Servers can be added from the UI, **per-user credentials** are asked of the person ("User provides key"), and "each user gets
  their own isolated connection" and "will be prompted to authenticate with their own OAuth login"
  (<https://www.librechat.ai/docs/features/mcp>).
- Authentication of a UI-added server is an **API key** (Bearer, Basic or a custom header) or **OAuth 2.0**; an OAuth server is
  "registered on save but starts out disconnected" until the person authorizes; a UI-created server resolves only the person's
  own variables, never the server's environment or the person's identity token (same page).
- OAuth is the **authorization code flow with PKCE**, with automatic renewal when a refresh token exists
  (same page); the settings are `authorization_url`, `token_url`, `client_id`, `client_secret`, `scope`, `redirect_uri`
  (`/api/mcp/<server>/oauth/callback`) (<https://www.librechat.ai/docs/configuration/librechat_yaml/object_structure/mcp_servers>).
- Who may create is a flag, `interface.mcpServers.create` (beside `USE`, `SHARE`, `SHARE_PUBLIC`), and a server can be shared with
  users, groups, roles or all (same pages); LibreChat's network guard is `mcpSettings.allowedDomains` and `allowedAddresses`,
  private IP literals blocked unless allowed (configuration page).
- *Unverified:* how LibreChat encrypts what it stores (its pages say only "stored securely"), and whether its callback binds
  `state` to the signed-in person.

What the protocol requires of a client (*verified 2026-10-06*, MCP specification 2025-11-25,
<https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization>): discovery by the server's Protected Resource
Metadata (RFC 9728) and the authorization server's metadata; **PKCE with `S256`**, refusing a server whose metadata lacks
`code_challenge_methods_supported`; the `resource` parameter (RFC 8707) on the authorization and the token request; client
registration by pre-registration, Client ID Metadata Documents or dynamic registration; tokens only in the `Authorization`
header, never in a URL; all authorization-server endpoints over HTTPS and redirect URIs `localhost` or HTTPS; a client must
not send a token to a server other than the one its authorization server issued it for. Every authorization is optional for
a server, so **"none"** is a real kind.

## Decision

1. **An agent's own MCP servers are fixed** *(accepted)*. They are the agent's files or custom resource (adam-rs `mcp.json`), and nothing here edits them. A user server is only ever
   **added on top**, as a relayed tool `<server>__<tool>`, so it cannot shadow or replace one of the agent's. The deployment's
   `toolServers` stay as they are.
2. **People add their own servers, kept per person** *(accepted; the rest proposed)*.
   - **Kinds** (a closed enum, ADR 0004, *proposed* name `UserAuth`): `none`; `bearer` (a token); `api_key` (a value in a header
     the person names, with LibreChat's "custom header" shape; the names `Host`, `Accept`, `Content-Type`, `Mcp-*` and
     `Last-Event-ID` are refused, as for `toolServers[].headers`); `oauth2` (authorization code with PKCE; scopes, and an
     optional pre-registered client id).
   - **Owner and limits:** keyed by the token's subject, as threads are ([ADR 0039](0039-nobody-reads-another-persons-thread.md)):
     nobody lists, reads or attaches another's server (a 404). *Proposed* at most 8 per person; the existing 16 per thread
     stand. **Org sharing is later** (open question 53): nothing here shares a server.
   - **What is stored here** is the definition and its state, never a secret: id, name, URL, kind, header name, scopes, state,
     a `CredentialRef` (an opaque id the broker gave). Postgres, a table of its own like the thread's organisation in
     [ADR 0042](0042-the-thread-list-is-the-owners.md); **no event** records it (it is the person's, not the thread's).
     The log keeps recording `tools_attached` with an id only, as now.
   - **The id** is `u-` plus 10 lowercase letters or digits, made here; a deployment `toolServers[].id` may not start with `u-`
     when the feature is on (a startup error), so the two never collide.
3. **Secrets go to a credential broker** *(accepted)*. The broker, its storage and its OAuth work are specified in the platform
   ([architecture §39a, `08-security.md`](https://github.com/vymalo/another-agentic-platform/blob/main/docs/architecture/08-security.md#39a-connections-user-mcp-servers-and-the-credential-broker)). This repository's side is two ports in `orch-ports`
   ([ADR 0009](0009-swappable-implementations-at-build-time.md); no driver type in a signature), each with a testkit, in the
   style of `ToolServerClient`. **Not implemented here.**
   - `UserToolServerStore` (*proposed*): `put`, `get`, `list(owner)`, `set_state` and `delete` of a definition, by
     `(owner, id)`; a Postgres adapter and an in-memory one, with a conformance testkit.
   - `CredentialBroker` (*proposed*): `store(owner, server, kind, material) -> CredentialRef` (material is a `ToolSecret`),
     `begin_oauth(owner, server, redirect) -> {authorize_url, state}`, `complete_oauth(owner, state, code)`,
     `lease(owner, server) -> Lease { header, value: ToolSecret, expires_at }`, and `revoke(owner, server)`. Discovery, PKCE
     `S256`, the `resource` parameter, client registration, storage and refresh are the broker's, as the spec requires of a
     client; this side never holds a verifier, a refresh token or a client secret. Its errors carry a class
     (`Classify`, as every port's): `NeedsReauth`, `Unavailable`, `Unknown`.
   - **A secret appears only** on the person's request body to the add or key endpoint, in memory, on its way to `store`, and
     on the relay's request to the server in a `ToolSecret`. It is in **no** event, outbox row, export, AG-UI frame, log line,
     step input or output ([ADR 0030](0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)), error or agent file;
     the request body of those endpoints is never logged. A test that plants a sentinel secret and greps every output is the
     acceptance of the slice, as `dev/tools-e2e.sh` does for configured ones.
   - **Fail closed**: with no broker configured, `bearer`, `api_key` and `oauth2` cannot be added (only `none`); the feature
     itself is off unless `userToolServers.enabled: true` (*proposed*).
4. **A user server is relayed like web search, and fails closed** *(accepted; the names proposed)*.
   - **Added** with `POST /api/user-tool-servers` (*proposed*; with `PATCH`, `DELETE`, `POST …/authorize`, and the OAuth
     callback `GET /api/user-tool-servers/oauth/callback`, `state` bound to the person who started the flow and used once; see the amendment below for desktop and mobile).
     `GET /api/tool-servers` lists the person's own beside the deployment's, each with `status`
     (`ready`, `needs_auth`, `unavailable`) and a `reason` the web shows; never a URL's credential.
   - **Attached** by the existing `PUT /api/threads/{id}/tools`, which needs `thread.write` as today. Only the thread owner's own
     servers are accepted (a 422 otherwise); a fork carries those its new owner owns and detaches the rest in its first
     events, as it does for an agent that may not use a server.
   - **Relayed** by `RelayTools` through the same `ToolServerClient`: per `tools/list` and `tools/call` it asks the broker for a
     **fresh** `lease`, so a token is never cached by the relay or older than the broker says; the agent sees `<id>__<tool>` and
     the orchestrator reports the step with the server's icon (a generic one: a user supplies none).
   - **A server whose credential cannot be obtained is not offered**: not `ready` in the picker (with its reason), a 422 on
     attach, left out of `tools/list`, and a call that was already attached fails as a refused credential does today
     (`isError`, "the MCP server needs the person to sign in again", a `failed` step), never a silent skip. The person's own
     `needs_auth` is a call to action in the web, not an error of the agent.
   - **What a user server says is untrusted**: its tool `annotations` are dropped (ADR 0024: untrusted unless from a trusted
     server), its `description` is cut at 2 KiB, and nothing it returns is read as an instruction here.
   - **Permission** (*proposed*, ADR 0045 style, an orchestrator permission mapped in `auth.roles`, and in Keycloak a client
     role that a composite can bundle): **`tools.user.add`** gates adding, editing, authorizing and re-keying a server.
     Listing one's own, attaching it (`thread.write`) and **removing** it are not gated by it, so a person whose permission is
     taken away can still remove what they added; but with it off a person's servers are not offered (`status:
     unavailable`, reason "not permitted"). It is **off by default**: the chart grants it to no role, and a deployment gives it to
     the roles it chooses. `admin` (ADR 0039) does not imply it and reads nobody's servers.
5. **A person's URL is untrusted** *(accepted; the rules proposed)*. One function, in `orch-ports` so that the broker's side
   applies the same, decides whether a URL may be added **and** whether a connection may be made:
   1. **`https` only**, a host, no user information, query or fragment; no redirect is followed (the client follows none today).
   2. **Private ranges are denied by default**: loopback, RFC 1918, link-local (the metadata address among them),
      carrier-grade NAT, multicast, reserved and the IPv6 equivalents, judged on **the address the connection is made to**, not
      on the name, so DNS rebinding has no gap. This is the guard of `dev/searxng-mcp/server.mjs` (`fetch`), which is the
      model, ported rather than shared.
   3. **The deployment's lists win** (`userToolServers.egress.deny` and `.allow`, host patterns, *proposed*): a **deny** match
      refuses, whatever else; a non-empty **allow** list refuses every host it does not match; a host that an allow entry names
      may resolve to a private address (an operator's explicit choice, as LibreChat's `allowedAddresses`); the default of the
      allow list is empty, meaning every public host.
   4. **The same rules apply to every URL the broker fetches** for the server (its resource metadata, authorization-server
      metadata, token endpoint and a client-metadata document), because those come from a stranger too (the spec itself warns
      of SSRF in the last).
   5. The check runs when a server is added or edited, again at each connection, and a refusal says which rule, never a
      resolved address.

## The flow

```mermaid
sequenceDiagram
  participant P as Person (web)
  participant O as Orchestrator
  participant B as Credential broker
  participant S as User's MCP server
  participant A as Its authorization server
  participant G as Agent
  P->>O: POST /api/user-tool-servers (url, oauth2) [tools.user.add]
  O->>O: egress check, store definition (state added)
  O->>B: begin_oauth(owner, server)
  B->>S: discover (resource metadata), AS metadata
  B-->>O: authorize_url (PKCE S256, resource), state
  O-->>P: authorize_url
  P->>A: consent
  A-->>P: redirect to the callback with code and state
  P->>O: GET /api/user-tool-servers/oauth/callback
  O->>B: complete_oauth(owner, state, code)
  B->>A: token request (verifier, resource)
  B-->>O: CredentialRef, connected
  P->>O: PUT /api/threads/{id}/tools (attach u-…)
  G->>O: tools/call u-…__search (thread-tools/v1)
  O->>B: lease(owner, server)
  B-->>O: fresh token (refreshed if expired)
  O->>S: call with Authorization Bearer
  S-->>O: result
  O-->>G: result (step reported)
```

```mermaid
stateDiagram-v2
  [*] --> added: POST (definition saved, no credential)
  added --> connected: consent done, key stored, or kind none
  connected --> needs_reauth: refresh fails, 401, scope step-up
  needs_reauth --> connected: person authorizes again
  connected --> revoked: person disconnects, or upstream revokes
  needs_reauth --> revoked: person disconnects
  revoked --> connected: person reconnects
  added --> [*]: deleted
  connected --> [*]: deleted (broker revokes)
  needs_reauth --> [*]: deleted
  revoked --> [*]: deleted
```

Only `connected` is offered. `unavailable` (server down, egress refused, permission off) is a reason shown, not a stored
state: it clears when the cause does. The state is the definition's, so it is the same for every thread of the person.

## Consequences

- One agent, with a chat's web search, can also have the person's own GitHub or Notion server for that chat only; the agent's
  own tools are untouched, and nobody else's thread sees the server.
- **Invariant 3 is amended** (note in `CLAUDE.md`): Postgres also keeps a person's definitions of servers, which is not the log
  and holds no secret. The credentials are the broker's, outside this repository's database.
- **A new required port pair**: the composition root needs a broker adapter, and one for the platform's service is a slice of
  its own. With none, only `none` servers work.
- **The relay gains a dependency on the broker on its critical path**: a broker that is down makes the person's `oauth2`,
  `bearer` and `api_key` servers `unavailable`, never the deployment's.
- **A revoked permission** does not detach a server from a running thread; it stops it being offered and leased at the next
  attach, and `userToolServers.enabled: false` stops every lease at once (the chart's switch).
- `config.md`, its schema, `chat-api.yaml`, `thread-tools-v1.md` and the web's picker change in the slices that build it;
  `docs/open-questions.md` question 11 (authentication to MCP servers) is partly answered here and stays open for the
  deployment's own servers.
- Slices (*proposed*): ports and testkits with an in-memory broker; the egress function and its tests; the Postgres store and
  endpoints; the relay's lease; the web (add, authorize, status); the platform's broker adapter.

## Alternatives rejected

- **Secrets in this database, encrypted** (as the orchestrator holds the configured ones in its environment). It makes the
  orchestrator the vault for strangers' tokens, with refresh and rotation here, and the owner chose a broker.
- **The agent connects to the person's server directly** ("direct", ADR 0024). The agent would hold the credential and the log
  would not see the calls; the relay is what exists and shows steps.
- **Let a person edit an agent's own servers.** They are the agent's contract, versioned with it; users add on top.
- **No egress guard, trusting the person.** The orchestrator would fetch any URL, the cloud metadata address among them.

## Amended 2026-10-06: the callback on desktop and mobile

[ADR 0047](0047-one-ui-for-web-desktop-and-mobile-each-signs-in-as-a-public-oauth-client.md) adds Tauri clients that call
the API with a bearer token. The browser that comes back to the OAuth callback (the system browser on desktop, the in-app
tab on mobile) carries neither that token nor, on native, the web's cookie. So the callback does not need the person to be
signed in: when the signed-in person starts the flow, `begin_oauth` returns the `state` and the orchestrator keeps only
`state → (who, which server, an expiry of minutes)`; the PKCE verifier stays in the broker, as decision 3 says. The callback
finds the person through that `state` alone, used once, and passes `code` and `state` to `complete_oauth`. The page
it answers says "you can return to the app"; the app learns the new status from `GET /api/tool-servers`.

## Amended 2026-10-11: accepted; the broker is the orchestrator's, behind two ports

The owner, 2026-10-11: "ADR 0046 should be accepted and built." Asked where the broker lives, he answered: "The orchestrator, but we keep it modular
enough, so that another company can decide to go with K8s secrets or Bitwarden or openbao if they want."

This **replaces decision 3's wording** (the platform's broker, the two ports, "this side never holds a verifier, a refresh token or a client secret",
"not implemented here"). Decisions 1, 2, 4 and 5, the flow and the callback amendment of 2026-10-06 stand. The old text above is kept as it was; read it
with this section. *Facts about third parties are in [Verified and unverified](#verified-and-unverified-2026-10-11).* **Nothing is built** *(as of 2026-10-11; slice 1 is built since, see the amendment of 2026-10-14)*.

```mermaid
sequenceDiagram
  autonumber
  participant R as Relay (orchestrator)
  participant B as CredentialBroker
  participant K as SecretStore (the adapter)
  participant A as Authorization server
  participant S as User's MCP server

  R->>B: lease(owner, server)
  B->>K: get(owner, server)
  K-->>B: version v, material (access token, expiry, refresh token)
  alt the access token is valid for 30 more seconds
    B-->>R: Lease {header, value, expires_at}
  else expired or about to be
    B->>A: token request, refresh_token and resource (RFC 8707)
    A-->>B: a new access token, and a new refresh token if it rotates
    B->>K: rotate(owner, server, expected v, new material)
    alt another replica refreshed first, Conflict
      B->>K: get(owner, server)
      K-->>B: version v+1, material
    end
    B-->>R: Lease
  end
  R->>S: the call, Authorization Bearer (a ToolSecret)
```

```mermaid
stateDiagram-v2
  [*] --> Current: named as secrets.pg.current, new secrets are sealed under it
  Current --> Previous: a new key is made current, rows sealed under this one still open
  Previous --> Retired: rewrap has re-sealed every row under the current key, the key is removed from the file
  Retired --> [*]
  Previous --> Previous: nothing new is sealed under it
```

1. **Where.** The broker is in the orchestrator: no separate service, no platform dependency. The platform's broker
   ([another-agentic-platform §39a](https://github.com/vymalo/another-agentic-platform/blob/main/docs/architecture/08-security.md#39a-connections-user-mcp-servers-and-the-credential-broker),
   *unverified here*: not re-read on 2026-10-11) stays a **possible future adapter of `CredentialBroker` itself** (an implementation that asks the platform for a lease), not of `SecretStore`.
2. **Two ports** in `orch-ports` ([ADR 0009](0009-swappable-implementations-at-build-time.md); no driver type in a signature), each with a conformance testkit in the style of
   `artifact_store_conformance!`. `UserToolServerStore` (definitions, decision 3's first bullet) is unchanged.
   - **`CredentialBroker`**: `store`, `begin_oauth`, `complete_oauth`, `lease`, `revoke`, as decision 3 listed them. **One implementation, written once** (`orch-credential-broker`): OAuth
     discovery (the protected resource metadata of RFC 9728, then the authorization server's), **PKCE `S256`** (refusing a server whose metadata lacks
     `code_challenge_methods_supported`), the **`resource` parameter** (RFC 8707) on the authorization and the token request, **client registration** (pre-registered, a Client ID Metadata
     Document, or dynamic registration, in that order), **refresh**, and **short leases**. Its URL guard is decision 5's. Errors are classed as before (`NeedsReauth`, `Unavailable`, `Unknown`).
   - **`SecretStore`**: storage of **opaque material** by `(owner, server)`; the adapter keeps it sealed at rest, and none parses it (the broker serialises the token set, the PKCE verifier of a pending flow
     under `(owner, "<server>#flow")`, a client secret).

     | Call | Does |
     |---|---|
     | `put(owner, server, material) -> version` | Creates or replaces. |
     | `get(owner, server) -> Option<(version, material)>` | Absent is `None`, not an error. |
     | `delete(owner, server)` | Idempotent. |
     | `rotate(owner, server, expected, material) -> version` | **Compare and swap** (*the owner's word "rotate", read this way*): replaces only when `expected` is the stored version, else `Conflict`. It is how a refreshed, rotated token set is stored without a lost update between replicas. |

     `material` is a `SecretMaterial`: `Debug` prints `<redacted>`, no `Display`, zeroized on drop (as `ToolSecret`, `orchestrator/crates/ports/src/tools.rs`), at most 16 KiB. Errors are classed
     (`Classify`): `Conflict`, `Unavailable`, `KeyUnknown`, `Corrupt`.
3. **Adapters are separate crates, chosen at build time** (invariant 6): a Cargo feature each, on `secrets.store` in the configuration, a store not compiled in is exit 78 naming the feature
   (as `artifacts.store`). With no `secrets` section only `none` servers can be added (decision 3's fail-closed rule, restated).
   - **`orch-secrets-pg`, the default.** One table in the orchestrator's own database (`user_secrets`: owner, server, version, envelope, updated). An **AES-256-GCM envelope**
     `{v, kid, nonce, ciphertext+tag}` with a random 96-bit nonce and the associated data `owner ‖ server ‖ version ‖ kid`, so a row moved to another person, server or version does not open.
     **The key comes from the configuration by reference, an AWS secret through an ExternalSecret** (chart property `user_secrets_key`, 32 random bytes, as `openssl rand -hex 32`):
     `secrets.pg.keys: [{id, key: {file: …}}]` and `secrets.pg.current: <id>`. **Rotation by key id**: add a key, make it current, run
     `orchestrator secrets rewrap` (it re-seals each row under the current key through `rotate` *(amended 2026-10-14, slice 3: in place at the same version, not through `rotate`; departure 47)*), then remove the old one. A row sealed under an id the file does not hold is `KeyUnknown`:
     that server is `unavailable`, never a crash.
   - **Later and optional**, each its own crate and its own dated amendment when it is built: **`orch-secrets-k8s`** (Kubernetes Secrets), **`orch-secrets-openbao`**, **`orch-secrets-bitwarden`**
     (Bitwarden Secrets Manager). A name an adapter derives for an object is a hash of `(owner, server)`, so no string a person chose becomes an object name (as `ArtifactKey`). What each
     can and cannot do is in the facts below: the Bitwarden adapter is **not promised**.
4. **Every adapter passes the testkit**: `secret_store_conformance!` (put then get, `get` of nothing, idempotent delete, `rotate` conflict, a second owner never sees the first's material, material that
   does not read back byte for byte is a failure, no secret in an error or a `Debug`); the broker has `credential_broker_conformance!` over an in-memory store and a WireMock authorization server.
5. **Refresh is the broker's and lazy**: at `lease`, inside the call, one refresh at a time per `(owner, server)` in a process (single flight); across replicas `rotate` settles it, and the loser
   re-reads. **This closes open question 54** (the orchestrator refreshes; it did not ask the platform).
6. **A secret appears** where decision 3 said (the person's request body, memory, the relay's call in a `ToolSecret`) **and at rest in the `SecretStore`, sealed**, nowhere else. The
   broker now holds a verifier, a refresh token and a client secret, in memory only while it uses them.
7. **Invariant 3.** A sealed secret in Postgres is new stored state, so its note in `CLAUDE.md` is amended today (and ADR 0001 has a status note). It is not the log, it is read only to lease a credential, and
   nothing reasons on it. **A consequence to plan for:** the database holds only envelopes, so **a backup of the database does not restore the secrets without the key**, which lives in the AWS secret;
   losing the key loses every stored credential (people authorize again), losing the database loses the definitions as well.

**Consequences, replacing decision 3's.** No new infrastructure for the default: the same database. The composition root needs one `SecretStore` adapter, `pg` unless a deployment chooses another at build time.
The relay's critical path now depends on the database and, at a refresh, on the person's authorization server, not on a second service. A company that wants Kubernetes Secrets, OpenBao or Bitwarden writes or
picks an adapter crate and sets `secrets.store`; nothing else changes. **Slices, replacing the list above:** the two ports and their testkits with an in-memory store; `orch-credential-broker` against a mock
authorization server; the egress function; `orch-secrets-pg` with `rewrap` and the chart's key; the definitions' table and the endpoints; the relay's lease; the web (add, authorize, status); then the
optional adapters. **The first alternative rejected below ("Secrets in this database, encrypted") is reversed for the default adapter**, on the owner's words: the broker is the orchestrator's and the vault is
behind a port, one crate away.

**Kept open (the owner to confirm, or a later amendment):**
1. The **envelope layout and the associated data** above, which are this amendment's, the owner having named AES-256-GCM, the key by reference and rotation by key id only.
2. **`rotate` as compare and swap.** If the owner meant key rotation only, the port loses `rotate` and the broker takes a lock instead.
3. **A refresh token presented twice.** `rotate` prevents a lost update, not a double spend: two replicas that refresh at the same instant present one refresh token twice, and an authorization server that
   treats reuse as theft revokes the grant (the server goes `needs_auth`). Single flight makes it rare. A cross-replica lock is not in the port; add one if it happens.
   *(Closed 2026-10-14, slice 2 review: the broker claims the refresh in the store with `rotate` before it asks, so no lock is added to the port; see departure 37.)*
4. **The three later adapters**, and **the platform's adapter of `CredentialBroker`**: nothing is decided beyond their place.

### Verified and unverified (2026-10-11)

*Verified 2026-10-11*, page reads (the tool summarised the pages, so wording is a paraphrase unless quoted):
- **Kubernetes Secrets** (<https://kubernetes.io/docs/concepts/configuration/secret/>): "Kubernetes Secrets are, by default, stored unencrypted in the API server's underlying data store (etcd)", and the page
  recommends encryption at rest. A Secret is created with `POST /api/v1/namespaces/{namespace}/secrets` and patched at `…/secrets/{name}`
  (<https://kubernetes.io/docs/reference/kubernetes-api/config-and-storage-resources/secret-v1/>); the size limit is a constant, `MaxSecretSize`, whose number was not read. So the adapter would rely on the cluster's
  encryption at rest and on RBAC for the orchestrator's ServiceAccount; it adds no sealing of its own unless the owner wants one (*open*).
- **OpenBao KV v2** (<https://openbao.org/docs/secrets/kv/kv-v2/>): a secret is written and read at `secret/data/<path>` (the mount is configurable), deleting the metadata path `secret/metadata/<path>` removes the key with
  all versions, every write makes a version, and a write may carry a `cas` value that must match the current version. That fits `put`, `get`, `delete` and `rotate`. The HTTP methods and request bodies were not read
  on the API page (its fetch returned nothing); the adapter's author reads them first.
- **Bitwarden Secrets Manager** (<https://bitwarden.com/help/secrets-manager-overview/>, <https://bitwarden.com/help/secrets-manager-sdk/>): its objects are secrets, projects, machine accounts and access tokens; there is a CLI and
  an SDK (a Rust core with wrappers for several languages). The SDK page lists authenticating, retrieving a secret and listing secrets and projects; it does **not** mention creating, updating or deleting, and no stand-alone
  REST API appeared on the pages read. **Whether an adapter can `put`, `delete` and `rotate` is unverified**, which is why it is not promised.

*Unverified:* the safe volume of random 96-bit nonces under one AES-GCM key (NIST SP 800-38D sets a limit near 2^32 invocations; from memory, the volume here is tiny) *(read and verified 2026-10-14, slice 3: section 8.3 says 2^32; see the facts of that section)*; the platform's section (above).

## Amended 2026-10-14, slice 1: the ports, their testkits, the in-memory stores and the egress guard are built

All in `orchestrator/crates/ports` (`orch-ports`); `orch-core` is untouched, and **nothing is wired into the binary**: no `config`, no endpoint, no relay change, no adapter beyond the in-memory ones.
Decisions 1, 2, 4 and 5, the flow and the amendments above stand. What exists, by file under `orchestrator/crates/ports/src/`:

| File | Public names | Pinned by |
|---|---|---|
| `egress.rs` | `EgressPolicy` (`new`, `allowing_http`, `with_allow`, `with_deny`, `from_lists`, `check_url`, `check_resolved`), `EgressTarget`, `CheckedAddrs`, `EgressRefusal` (`rule()` for the operator's log; `public_reason()`, `public_rule()`, `public_class()` for a person), `HostPattern`, `EgressHost`, `AddressClass`, `classify_address`, `is_public_address`, `MAX_URL_BYTES` | `tests/egress.rs` |
| `secrets.rs` | `SecretStore` (`put`, `get`, `delete`, `rotate`), `SecretMaterial`, `MAX_SECRET_BYTES`, `MaterialTooLarge`, `SecretVersion`, `SecretServer`, `SecretError` | `secret_store_conformance!` (with an optional `breaker`), `tests/secrets.rs` |
| `user_tools.rs` | `UserToolServerStore` (`insert`, `get`, `list`, `replace`, `set_state`, `delete`), `StoredServer`, `ServerRevision`, `UserToolServer` (`new` from an `&EgressTarget`, `from_stored`), `UserToolServerId`, `UserAuth`, `UserToolServerState`, `ToolServerStatus`, `UnavailableReason`, `DefinitionError`, `UserToolServerStoreError`, `MAX_USER_TOOL_SERVERS` | `user_tool_server_store_conformance!`, `tests/user_tool_servers.rs` |
| `credentials.rs` | `CredentialBroker` (`store`, `begin_oauth`, `complete_oauth`, `lease`, `revoke`), `CredentialLease` (a closed enum), `CredentialRef`, `OAuthStart`, `OAuthState`, `RedirectUri`, `CredentialError` | `tests/credentials.rs` (a stub broker that follows the trait's docs; **no conformance macro**, it is slice 2's) |
| `tools.rs` | `is_reserved_header`, `RESERVED_HEADERS`; `ToolSecret` is now wiped on drop | `tests/user_tool_servers.rs`, `tests/secrets.rs`; `orch-tools-mcp` uses it |
| `memory/` | `MemorySecretStore`, `MemoryUserToolServers` (feature `testkit`) | the two macros |

```mermaid
sequenceDiagram
  participant C as Caller, the broker or the relay
  participant G as EgressPolicy
  participant D as Resolver of the caller
  participant S as User MCP server

  C->>G: check_url(stored url)
  G-->>C: EgressTarget (normal form, host, port), or the rule that refuses
  C->>D: resolve the host, once
  D-->>C: the addresses
  C->>G: check_resolved(target, addresses)
  G-->>C: CheckedAddrs, or the rule that refuses
  C->>S: connect to the checked addresses only, no redirect, no proxy
```

```mermaid
stateDiagram-v2
  [*] --> Classified: an address the host resolved to
  Classified --> Public: global unicast
  Classified --> Private: loopback, RFC 1918, carrier-grade NAT, unique local
  Classified --> Special: link-local, multicast, reserved, documentation, outside the global unicast block
  Public --> Admitted
  Private --> Admitted: an allow entry names the host
  Private --> Refused: none does
  Special --> Refused: whatever the lists say
  Admitted --> [*]
  Refused --> [*]
```

**Departures from the text above, and clarifications the ports needed** (the owner to confirm; each is one line to change). *Numbered as the pull request's body numbers 1 to 10; 11 and 12 were in this list and not in the body; 13 to 19 are the review's, below.*

1. **`allow` lifts private, for what the operator named exactly; never for a suffix, never special** (decision 5.3). A host named by an **exact `allow` entry** (a host written in full, or an address) may resolve to a private-use address (loopback, RFC 1918, carrier-grade NAT, unique local): the address entry for that address only. A **`*.` suffix entry** only *admits* the hosts it matches: it does **not** lift the ban on private addresses, because it names a family of names that a stranger can make (`*.vercel.app` would let `evil.vercel.app` resolve to `10.0.0.1`); to reach a private network, name the hosts or the addresses. Nothing in `allow` lifts a link-local one (the metadata address among them), nor an unspecified, multicast, broadcast, reserved or documentation one: a pattern such as `*.internal` would otherwise name `metadata.google.internal`. An IP literal written in `allow` is held to the same. *(Amended 2026-10-14, review: the suffix case; the text before said any `allow` match lifted the private ban.)*
2. **The guard refuses more than the ADR lists:** `localhost` and `*.localhost` (always loopback, RFC 6761) at `check_url` unless `allow` names them **exactly**; **one** refused address among those a name resolved to refuses the whole answer (the model's `guardedLookup` does the same); a URL whose normal form is over 2 KiB. *(Amended 2026-10-14, review, also:)* the lists are applied to **the addresses a name resolved to and to IP literals, in any spelling** (an IPv6 address that carries an IPv4 one is compared as that IPv4 address), so `deny: ["8.8.8.8"]` stops a name that resolves to it and `https://[::ffff:808:808]/`; an `allow` address entry admits a name whose every resolved address it names; a numeric pattern is read strictly (`010.0.0.1`, `127.1`, `0x7f.1` are refused); `192.88.99.0/24` is a special range.
3. **`CredentialLease`, not `Lease`**: `orch_ports::Lease` is the thread store's. For `Authorization` the lease's token is the **bare token** (the relay gives it to `ToolServerEndpoint::with_bearer`); any other header carries the value as it is. *(Amended 2026-10-14, review: it is a closed enum, `Bearer` and `Header`, with no `PartialEq`, `Eq` or `Clone`.)*
4. **`complete_oauth(owner, server, state, code)`** takes the server, which the list of decision 3 did not: the pending flow is kept under `(owner, server)` (amendment, 2), so the broker cannot find the verifier without it. The callback knows the server from the table that maps `state` to the person (amendment of 2026-10-06). *(Amended 2026-10-14, slice 2 review: it takes the callback's `iss` as well, `complete_oauth(owner, server, state, code, iss)`; departure 36.)*
5. **`begin_oauth`, `lease` and `store` carry only `(owner, server id)`** (and the secret, and the redirect), not the URL, the kind, the scopes or the client id: `orch-credential-broker` is composed over a `UserToolServerStore` and reads them from the definition, which stays the one place that says what a server is. *(Amended 2026-10-14, review: `store` no longer takes the kind, which could disagree with the definition.)*
6. **The `SecretStore` key's second half is a `SecretServer`**, not a free string: `u-abcdefghij` (a server's credential) or `u-abcdefghij#flow` (its pending flow, the amendment's `"<server>#flow"`), validated, so no string a person chose becomes an object name.
7. **Stricter than the text, in `SecretStore`**: a version is **never handed out twice for one key, not even across a `delete`**. Without it a replica that read version 3, then saw the grant revoked and made again, could `rotate` from 3 and overwrite the new grant with a refresh of the old one. `rotate` of a secret that is not there is a `Conflict`. Material may be empty. `KeyUnknown` and `Corrupt` have the error class `Corrupt` (nobody retries, an operator is told); the server is unavailable either way. *(Amended 2026-10-14, review: the contract is what `rotate` needs and no more. A version is opaque and **callers compare it for equality only**; the port no longer asks for "one counter for the whole store", which an adapter over OpenBao's per-key versions or a Kubernetes `resourceVersion` could not give, only that a version is never given twice for a key, across a delete included, and that the writers of one key are serialized and numbered in the order they land, so the version a key holds is the greatest it gave, which the concurrent case asserts. The memory store keeps its store-wide counter. A `put` of identical material is still a new version.)* `orch-secrets-pg` is to take versions from a sequence; an adapter that cannot do the same fails the case `a_version_is_never_reused_after_a_delete`.
8. **The `api_key` header refuses `Host`, `Accept`, `Content-Type`, `Last-Event-ID`, every `Mcp-*` and `Authorization`**: the last because it is the `bearer` kind's and `orch-tools-mcp` refuses it in `headers` (it is also what `toolServers[].headers` refuses today). A person who needs `Authorization: Basic …` cannot yet; see open question 2 below. *(Amended 2026-10-14, review: also `Content-Length`, `Transfer-Encoding`, `Connection`, `Upgrade`, `TE`, `Expect`, `Keep-Alive`, `Trailer`, `Cookie` and every `Proxy-*`, in any case: headers the HTTP client frames or routes the request with.)*
9. **The state is the diagram's, `added`, `connected`, `needs_reauth`, `revoked`** (`UserToolServerState`), because the text says `unavailable` is a reason shown and not a stored state. `ready`, `needs_auth` and `unavailable` with a reason are the derived `ToolServerStatus` and a closed `UnavailableReason` (`not_permitted`, `egress_refused`, `server_down`, `broker_down`). *(Amended 2026-10-14, review: no port signature uses these two types. They stay in `orch-ports`, and not in the API's crate, because the API's listing and the relay's refusal to offer a server both write them and this is the crate both depend on, and because `UserToolServerState::status()` is the one table from a stored state to what a person sees. They move to the crate that owns the endpoints if it becomes the only one that needs them.)*
10. **The reserved-header check is one function now**, `orch_ports::is_reserved_header`: `orch-tools-mcp` uses it in place of its private copy. `orch-config` still has its own list (it depends on no other crate of the workspace), unchanged.
11. **The definition's "optional header name, scopes"** are in the variants of `UserAuth` (`api_key { header }`, `oauth2 { scopes, client_id }`), not fields beside it.
12. **The limit of eight is the store's**: `UserToolServerStoreError::TooMany { limit }` (class `Rejected`), an edit at the limit is not a ninth, and the testkit races sixteen adds to see eight succeed. A definition's name is held to 80 characters, at most 32 scopes. *(Amended 2026-10-14, review: the testkit repeats the race twenty times, each with an owner of its own, because a count that is read and then written passes the limit only when the writers interleave. The trait's docs name the technique a SQL adapter must use, a lock per owner, taken before the count: a transaction-scoped advisory lock keyed by the owner, or `SELECT … FOR UPDATE` on a row of the owner. That is the definitions table's, **slice 4**; `orch-secrets-pg` keeps no limit.)*
13. **`UserToolServerStore` has no upsert** *(review)*. `put` was an unconditional write: an edit that read a definition and then wrote it back reverted a state or a credential reference the broker wrote in between, and brought back a server that was deleted in between. It is `insert` (`Exists`, `TooMany`), `replace(owner, server, expected)` and `set_state(owner, id, state, expected)`, both **compare and swap on a `ServerRevision`** the store keeps and a read returns (`StoredServer`): a stale revision is `Conflict`, an absent definition is `NotFound` and nothing is created. The credential reference is set by `replace` (no third call). A revision is new for every write and never given twice for a key, across a delete included; callers compare for equality. `delete` is unconditional and idempotent. Chosen over "set_state without a revision, never resurrecting": a state computed for a URL that was edited since must not be written onto the edited definition.
14. **A definition's URL is one the guard accepted** *(review)*. `UserToolServer::new` takes an `&EgressTarget`, which only `EgressPolicy::check_url` makes (and an adapter's `UserToolServer::from_stored`), so an unchecked string cannot be stored. A row read back goes through `from_stored`, which checks the URL again as the **normal form** of a URL that passes the rules of the URL itself (scheme, user information, host, query, fragment, port, length), **not** the deployment's lists or the address: the lists change, and a person must still be able to list a server that today's lists refuse, see `unavailable` (`egress_refused`) and delete it; the connection applies the lists every time. `Debug` of a definition and of an `EgressTarget` prints the scheme, host and port, **never the path**, which can carry a secret.
15. **A credential is bound to the URL it was issued for** *(review; the contract of the broker, written in the trait's docs)*. The broker keeps the definition's normal-form URL (the OAuth `resource`) inside the sealed material when it stores a credential, and `lease` compares it with the definition's now: a mismatch is `NeedsReauth`. Deleting a server is **"revoke, then delete"**: `revoke` reads the definition to find the authorization server, so it runs first, and a `revoke` that fails aborts the delete. A revoked or never-given credential is `NeedsReauth`; `Unknown` is "no such server" (and a kind that has no credential: `lease` of a `none` server is `Unknown`, and the relay does not ask).
16. **The OAuth inputs are values, not strings** *(review)*. `begin_oauth` takes a `RedirectUri` (`https`, or `http` on the loopback literals `127.0.0.1` and `[::1]`, no fragment, no user information: the MCP and native-app rules; `localhost` and custom schemes are not accepted), `OAuthStart` says when the pending flow ends (`expires_at`), and `OAuthState` is checked when it is made (16 to 128 unreserved characters, also for a callback's) and compared in constant time. **An abandoned flow, or a callback with `error=`, needs no call**: the pending flow expires, the broker forgets it the next time it meets it, and the `state` cannot be completed after `expires_at`.
17. **A person is not shown the precise refusal** *(review)*. `private-address` and `no-address` together tell which internal names exist. `EgressRefusal::rule()` stays precise, for the operator's log; the API shows `public_reason()`, `public_rule()` and `public_class()`, which are one for both.
18. **`ToolSecret` is wiped on drop** *(review)*, as `SecretMaterial` is, and both **derive** `ZeroizeOnDrop` (the derive refuses a field that is not wiped), so the promise is checked by the compiler. `tests/secrets.rs` pins the declaration and does not claim to prove that memory is cleared.
19. **The testkit looks into the errors that can hold text** *(review)*. `no_error_or_debug_text_shows_a_secret` only provoked `Conflict`, which holds nothing. `secret_store_conformance!(make, breaker = path)` takes an `async fn(&S)` that breaks the store (a Postgres adapter closes its pool; the memory store is taken down) and runs `put`, `get`, `rotate` and `delete` with a sentinel in flight, looking for it in the `Display`, `Debug` and sources of each error. Without a breaker the case is skipped and says so. Four leaky stores in the tests show it can fail.

**Facts.** *Verified 2026-10-14, by `tests/egress.rs` against `url` 2.5.8:* the URL parser reads `2130706433`, `017700000001`, `0x7f000001`, `0x7f.0.0.1`, `0177.0.0.1`, `127.1`, fullwidth digits, a trailing dot and bracketed IPv6 forms as the address they name, normalises `https:///mcp`
to `https://mcp/` and drops an empty `@`; no address that `std` calls private, loopback, link-local, multicast, broadcast, documentation, unspecified, unique local or unicast link-local is judged public, over every /16 of IPv4 and every first group of IPv6 (`tests/egress.rs`).
*Verified 2026-10-14, review, by tests and a probe:* the URL parser decodes `127%2e0%2e0%2e1` to `127.0.0.1` and refuses a zone id in a bracketed literal; `std`'s `Ipv4Addr::from_str` refuses `010.0.0.1` (a leading zero), `127.1` and `0x7f.1`; the `zeroize_derive` 1.5.0 derive of `ZeroizeOnDrop` fails to compile for a field that is neither `Zeroize` nor `ZeroizeOnDrop` (checked with a `std::fs::File` field, not kept).
*Unverified (from memory of the IANA special-purpose registries, not re-read today):* `192.88.99.0/24` as the deprecated 6to4 relay anycast prefix (RFC 7526), the carrier-grade NAT, benchmarking, reserved and `2001::/23` ranges, `64:ff9b:1::/48`, `5f00::/16`, and `3fff::/20` as the second documentation prefix (RFC 9637). *Unverified:* that `reqwest` 0.13's `dns::Resolve` and
`ClientBuilder::resolve_to_addrs` do what the egress docs say a caller uses them for; the slice that writes the broker reads them first *(read and verified in slice 2, 2026-10-14: see its facts)*.

**Open for the next slices.**
1. **`orch-credential-broker`** *(built in slice 2, 2026-10-14: see the section at the end)*: its conformance macro (`credential_broker_conformance!`), over `MemorySecretStore`, `MemoryUserToolServers` and a mock authorization server; how it takes an `EgressPolicy`; a resolver hook for `reqwest`. **The macro must have a case for departure 15**: a credential stored for one URL is not lent after the definition's URL changes (`NeedsReauth`), nor for a definition made again under the same id; a revoked or never-given credential is `NeedsReauth`, a missing server and a `none` kind are `Unknown`; an expired `state` is `Unknown` and a used one too; `revoke` is idempotent and runs before the definition is deleted.
2. **`Authorization` in an `api_key` header** (departure 8): LibreChat offers Basic. Allow it by letting the broker lease `Authorization` with a whole value (`Basic …`), which `ToolServerEndpoint` cannot carry today (it sets `Authorization` from `bearer` only), or keep it out.
3. ~~`ToolSecret` is not zeroized~~ *(amended 2026-10-14, review: it is, departure 18)*. What stays: a lease's value sits in memory for the length of a call, and a request makes copies while it is sent, which the wipe does not cover.
4. **A deployment with no `secrets` section** needs a `SecretStore` and a `CredentialBroker` that refuse every call (`NoArtifacts` is the model); not built, the composition root's slice.
5. **`orch-secrets-pg`** *(built in slice 3, 2026-10-14: see the section at the end; `rewrap` is not through `rotate`, departure 47)*: versions from a sequence (never given twice for a key, across a delete included, in the order the writes land), `rewrap` through `rotate`, and a `breaker` for the testkit that closes its pool. *(Amended 2026-10-14, review: "the limit under a lock per owner" is no longer here; it is the definitions table's, slice 4, below.)*
6. **The definitions table (slice 4)**: revisions from a sequence, the limit of eight under a lock per owner taken before the count (an advisory lock keyed by the owner or `SELECT … FOR UPDATE` on an owner row; the testkit races it twenty times), `replace` and `set_state` as one statement with the revision in its `WHERE`, rows read back through `UserToolServer::from_stored` (a failure is `Corrupt`), and the API showing `EgressRefusal::public_reason()` and never `rule()`.
7. The IPv6 table treats anything outside `2000::/3` as special: a new allocation is added to `egress.rs` when a server lives in it.

## Amended 2026-10-14, review of the slice 1 pull request

The review of [pull request 250](https://github.com/vymalo/another-agentic-system/pull/250) found five major points and twelve minor ones, all fixed in the same pull request before it merged. The departures above carry each change where it belongs, marked *(review)* or *(Amended 2026-10-14, review)*; in short: the lists judge addresses in any spelling and a suffix never lifts the private ban (1, 2); the two checks that could not fail now can (18, 19); a credential is bound to its URL, which is the broker's contract (15); the definitions store has no upsert (13). Also: the lease is a closed enum (3), the version contract is what `rotate` needs (7), the redirect and the state are values (16), the headers a client frames a request with are refused (8), a URL never prints its path (14), the refusal a person sees is one for two causes (17), the limit race is repeated (12), and `ToolServerStatus` and `UnavailableReason` stay in `orch-ports`, with a reason (9). Decisions 1 to 5 stand.

## Amended 2026-10-14, slice 2: the broker is built

`orchestrator/crates/credential-broker` (package `orch-credential-broker`) is the one implementation of `CredentialBroker`, and `credential_broker_conformance!` (with its fixture trait
`BrokerFixture`) is in `orch-ports`' testkit. **Nothing is wired into the binary**: no `config`, no endpoint, no relay change, no secret adapter beyond the in-memory store (slices 3 to 5). `orch-core`
is untouched; `orch-ports` gained the testkit module `credential_broker` and doc changes only (no signature changed). Decisions 1 to 5, the slice 1 departures and the amendments above stand.
Process diagrams (the OAuth flow, the lease and its refresh, the states of a credential and of a pending flow) are in the crate's `README.md`; every node and edge is a call or a branch in the code.

| Where | What |
|---|---|
| `crates/credential-broker/src/broker.rs` | `Broker<S, D, C = SystemClock>` (generic over `S: SecretStore`, `D: UserToolServerStore`, `C: Clock`; the stores are held by value, as the adapters are cheap to clone), `BrokerBuilder`, `egress_refusal(&CredentialError)` |
| `.../src/http.rs` | the one HTTP client and the guard in front of it: `check_url`, the `reqwest::dns::Resolve` hook, `HostLookup`/`SystemLookup`, bounds |
| `.../src/discovery.rs`, `oauth.rs`, `material.rs`, `flights.rs`, `config.rs` | RFC 9728 and RFC 8414 discovery, PKCE and the token endpoint, the sealed records, single flight, `BrokerConfig` |
| `crates/ports/src/testkit/credential_broker.rs` | the 27 cases, `BrokerFixture`, `RefreshOutcome`, `AsRequests`, `Consent`; macro in `testkit/mod.rs` |
| `crates/credential-broker/tests/` | `conformance.rs` (the macro, over `MemorySecretStore`, `MemoryUserToolServers` and an axum authorization server on `127.0.0.1`), `egress.rs`, `registration.rs`, `behaviour.rs`, `hygiene.rs`, `support/` (the mock server, a tripwire that counts connections, a secret store that yields after every call so that two replicas interleave) |

The new third-party dependencies of the workspace: **none** (`Cargo.lock` gains only the workspace package). `reqwest`, `url`, `base64`, `sha2`, `getrandom`, `serde_json`, `jiff`, `tokio`, `tracing` and
`zeroize` were already in it; `axum`, `tracing-subscriber` and `futures` are dev-dependencies of the new crate.

**Departures from the text above, and choices the build needed** (the owner to confirm; each is small to change). Numbered after slice 1's 19.

20. **`store` is for `bearer` and `api_key`; `oauth2` is `Unknown`.** The trait's docs also let `store` keep "an OAuth client secret of a client they registered themselves"; this broker does not: a pre-registered client
    (`oauth2 { client_id }`) is a **public** client with PKCE, and a client secret is only ever one that dynamic registration returned, kept inside the grant. The conformance macro does not pin this (an implementation that keeps
    client secrets still conforms); this crate's `tests/behaviour.rs` does. A confidential pre-registered client is an open question (below).
21. **A grant keeps the endpoints its authorization server named, and `revoke` does not read the definition.** The token and revocation endpoints (and the client) are sealed with the tokens, so a refresh and a revocation go
    where the grant was issued, with no second discovery, and a `revoke` works for a definition that is already gone (the trait's docs say `revoke` reads the definition to find the authorization server; it needs not, and
    "revoke, then delete" still holds). Each endpoint is judged by the guard **when it is used**, so a list tightened since is applied (a refresh under a deny list is refused with nothing sent: a test).
22. **The broker writes one state of the definition: `needs_reauth`**, with `set_state` and the revision it read, when it learns the grant is over (a refresh refused with `400` or `401`; an access token that ran out with no
    refresh token). A definition edited since is a `Conflict` or `NotFound` and nothing is written (a test renames a server while a refresh is in flight). `connected`, `revoked` and the credential reference are the API's
    `replace`, because the reference must be written in the same compare and swap. This is the "if the port requires it" of the slice's brief: the trait's docs on `set_state` call it the broker's write, and name no other.
23. **Discovery is by the well-known URIs only**: the path-inserted form, then the root (RFC 9728). The MCP specification (2025-11-25) says a client MUST also support the `WWW-Authenticate: Bearer resource_metadata=…` of a `401`
    from the server and use that URL when present; this broker makes no unauthenticated request to the person's server, so a server that publishes its metadata elsewhere is `Unavailable` (`NotFound`). Adding the probe is
    one function and an unauthenticated `POST` through the guard; not done.
24. **The broker never fetches a Client ID Metadata Document**: the authorization server does. The broker only *names* it: `BrokerConfig::with_client_metadata_url` (an `https` URL with a path, checked when the broker is
    built; the orchestrator serves the document, a later slice), used as the `client_id` when the server says `client_id_metadata_document_supported` and neither the definition nor the broker has a better one. The ADR's
    rule 5.4 ("a client-metadata document" among the URLs the broker fetches) therefore applies to nothing here; the SSRF risk of that fetch is the authorization server's (the draft says so).
25. **Dynamic registration happens at each `begin_oauth`**, a public client (`token_endpoint_auth_method: none`) for that redirect URI. A person who authorizes ten times leaves ten registrations at the authorization server.
    No RFC 7592 management, no cache of the client by `(owner, server, redirect)`. If the server returns a client secret it is kept in the grant and used (`client_secret_basic`, or `client_secret_post`).
    *(Amended 2026-10-14, slice 2 review: a client the broker registered is used again for an hour by the next `begin_oauth` for the same issuer and redirect URI; departure 40. There is still no RFC 7592 management.)*
26. **`complete_oauth` claims the flow before it exchanges the code**: it swaps the flow for a marker with `rotate` (the version it read), so of two callbacks for one `state` (two replicas, a double click) one wins and the
    code is spent once (a test races two). The flow is deleted whatever came of the exchange: **a failed exchange spends the `state`**, and the person begins again. *(Amended 2026-10-14, slice 2 review: the marker stays and nothing deletes it, departure 38; a callback whose `iss` is refused claims nothing.)* A wrong `state` is `Unknown` and leaves the flow (so a
    stranger's guess cannot end the person's flow); an expired one is `Unknown` and is forgotten.
27. **A refresh runs in a spawned task** (`tokio::spawn`), awaited by `lease`: a caller that gives up (a timeout in the relay) does not interrupt it between the authorization server's answer and the `rotate`, which would
    lose a refresh token the server has rotated. `lease` therefore needs a Tokio runtime (a test drops a lease at 80 ms and finds the refresh done and kept).
28. **Only `invalid_grant` drops the tokens.** Any `400` or `401` at the token endpoint is `NeedsReauth` and marks the definition (as the brief said), but only `invalid_grant` (RFC 6749, 5.2: the refresh token is invalid,
    expired or revoked) is certain enough to throw the tokens away (a compare and swap, so a grant made since is untouched); after that `lease` is `NeedsReauth` with no request. An `invalid_client` (the registered client was
    deleted) leaves the tokens, the definition is marked, and the person's next authorization registers again.
29. **The lifetime of an access token**: `expires_in` seconds (read as a number, a float or a numeric string; at most a year); when the server says nothing, **an hour if a refresh token was given**, else **no known end**
    (lent until the server says otherwise). *(Amended 2026-10-14, slice 2 review: `expires_in: 0` counts as nothing said; departure 43.)* A lease ends at `min(token end, now + lease TTL)`; the lease TTL (300 s) and the 30 s margin are `BrokerConfig`.
30. *(Revised 2026-10-14, slice 2 review: **the `resource` parameter is the definition's own normal-form URL**, once `same_resource` shows the document's `resource` names it; the document's spelling is never sent, and a person who named no scope asks for none; departure 39. The original text follows.)* **The `resource` parameter is the `resource` the server's own metadata declares**, after `same_resource` shows it names the definition's URL (`https://h` and `https://h/` are one; a different path, port, scheme, a
    query or a fragment are not, and then the document is not used: RFC 9728, 3.3). The MCP specification prefers the form without a trailing slash, which the URL parser does not write for a root path. The credential is
    bound to the definition's **normal form** (departure 15), not to this string.
31. **Two endpoints are judged at `begin_oauth`**, before the person is sent anywhere: the token endpoint (`check_url` and, for an address, `check_resolved`) and the authorization endpoint (by its shape only: scheme,
    host, user information and the lists, with the query removed, because the endpoint is a place to send the person and may carry a query that the guard refuses for a fetched URL). The registration and revocation
    endpoints are judged when used.
32. **`CredentialRef` is `cred-<server id>-v<secret version>`**; **the sealed records** are JSON `{"v":1,"d":…}` (the credential: a `bearer`, an `api_key` or an `oauth2` grant with its URL, `resource`, client, endpoints and
    tokens; the pending flow: the `state`, the verifier, its `expires_at`, the URL, the redirect URI, the client and the endpoints, or the marker `used`). A record that cannot be read (garbage, another version) is
    `Unavailable`, never `NeedsReauth`: the person's credential is not "gone", the store is wrong.
33. **`CredentialError` has no "invalid input" class**: a secret too large for the store (over 16 KiB with the record) is `Unavailable` with a fixed phrase, and a malformed token (empty, a newline in it) is kept as it is. The
    API must validate what a person types before `store` (length, characters); see the open points.
34. **`HostLookup` is public** (`SystemLookup` by default): how a name becomes addresses is a parameter of the builder, so that a test can map a name to a loopback address (DNS rebinding, and a name admitted by `allow` and
    connected to only through the hook) and a deployment can put its own resolver in. The guard judges whatever it answers.
35. **The testkit's authorization server must enforce** PKCE `S256`, the `resource` parameter, the redirect URI and the client's authentication (the module's docs list them), so that a case that gets a token has proved the
    request was right; and the fixture gives a second broker over the same stores (`replica()`) and a clock it reads, which the replica and expiry cases need. The suite is 27 cases (listed in `orch-ports`' README) *(36 since the slice 2 review)*.

**How the suite was checked to be able to fail.** Each of these changes to the broker was made in turn and made at least one test fail, then reverted: no URL binding (3 cases), `put` in place of `rotate` at a refresh,
no single flight, `S256` not required, no claim of the `state` (found only after the secret store was made to yield after every call: with a store whose calls never yield, two callers of one `join` run one after the
other and never race), the flow's expiry ignored, no `resource` at the token endpoint (17 cases), no 30-second margin, `invalid_grant` as `Unavailable`, no spawn, a state written with a fresh read instead of the revision
read, no `check_url`, no check of the resolved addresses, redirects followed.

**Facts, verified 2026-10-14.**
- *By reading the source* in `~/.cargo/registry/src` at the versions of `Cargo.lock`: `reqwest` 0.13.5 has `ClientBuilder::dns_resolver<R: IntoResolve>` (not behind a feature) and the trait `reqwest::dns::Resolve`
  (`fn resolve(&self, Name) -> Resolving`, `Addrs = Box<dyn Iterator<Item = SocketAddr> + Send>`), whose docs say "Explicitly specified port in the URL will override any port in the resolved `SocketAddr`s. Otherwise,
  port `0` will be replaced by the conventional port for the given scheme"; `ClientBuilder::resolve_to_addrs` overrides per domain and the resolver still serves the rest; `hyper-util` 0.1.21's `HttpConnector` does not call
  the resolver for a host that is an IP literal (`SocketAddrs::try_parse`), so the broker judges a literal itself before sending; `reqwest::Error::without_url`, `Response::chunk` exist; `form` is a separate feature that
  the workspace does not enable, so form bodies are built with `url::form_urlencoded`. The egress tests exercise the hook (a name resolved to a private address, to a mix, to nothing, to an error, and an admitted name
  connected to only through the hook) and fail when the hook does not judge the answer.
- *By page reads* (the tool summarised the pages, so wording is a paraphrase unless quoted): the **MCP authorization specification 2025-11-25** (<https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization>):
  clients "MUST support both" discovery mechanisms (the `WWW-Authenticate` header and the well-known URIs, in that order), the well-known fallback is the path-inserted URI then the root; the authorization-server metadata
  URLs for an issuer with and without a path are tried in the order the crate's `README.md` lists (OAuth 2.0 metadata, then OpenID Connect with the path inserted, then appended); registration priority is pre-registered,
  Client ID Metadata Documents when `client_id_metadata_document_supported`, dynamic registration, then ask the user; clients "MUST refuse to proceed" when `code_challenge_methods_supported` is absent; `resource` "MUST be
  included in both authorization requests and token requests", and `https://mcp.example.com` is preferred to the form with a trailing slash; "All redirect URIs MUST be either `localhost` or use HTTPS" and all authorization-server
  endpoints HTTPS. **RFC 9728** (<https://www.rfc-editor.org/rfc/rfc9728.html>), 3.1 (the well-known URI is formed by inserting the suffix between the host and the path; a terminating slash after the host is removed) and 3.3 ("The `resource`
  value returned MUST be identical to the protected resource's resource identifier value … If these values are not identical, the data contained in the response MUST NOT be used"). **RFC 8414** (<https://www.rfc-editor.org/rfc/rfc8414.html>), 3 (the
  suffix is inserted between host and path, a terminating slash of the path removed first), 3.3 (the `issuer` returned "MUST be identical" to the one used), and the defaults: `token_endpoint_auth_methods_supported`
  omitted is `client_secret_basic`, `code_challenge_methods_supported` omitted means no PKCE. **RFC 7636** (<https://www.rfc-editor.org/rfc/rfc7636.html>): a verifier of 43 to 128 unreserved characters with at least 256 bits of
  entropy, `S256` is `BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))` without padding (the crate's unit test reproduces the vector of RFC 7636, appendix B, which was written from memory and passes only if the transform is
  right). **RFC 8707** (<https://www.rfc-editor.org/rfc/rfc8707.html>), 2: "Its value MUST be an absolute URI", no fragment; the same parameter on the token request "for all grant types", and a refresh request in its figure carries it.
  **RFC 7591** (<https://www.rfc-editor.org/rfc/rfc7591.html>): registration is a JSON `POST`; `token_endpoint_auth_method` `none`, `client_secret_post`, `client_secret_basic` (default); `client_secret_expires_at` is required if a secret is
  issued. **RFC 7009** (<https://www.rfc-editor.org/rfc/rfc7009.html>), 2.1 and 2.2: a form `POST` with `token` and `token_type_hint`, client authentication as in RFC 6749, `200` also for an invalid token, `503` meaning "assume the token
  still exists"; revoking a refresh token "SHOULD also invalidate all access tokens based on the same authorization grant". **The Client ID Metadata Document draft**: the page now shows
  `draft-ietf-oauth-client-id-metadata-document-02`, dated 6 July 2026 and expiring 7 January 2027 (the MCP specification cites `-00`): the client identifier URL "MUST use the https URL scheme" and "MUST contain a path component",
  the document's `client_id` must match the URL, and the authorization server "MUST NOT" fetch a document URL (or one inside it) that resolves to a special-use address nor follow redirects automatically.
- *By tests of this crate, against a mock authorization server of ours*: everything the suite lists. No live authorization server, GitHub, Notion or other product was used.

*Unverified:* RFC 6749, 2.3.1 (the Basic credentials are form-encoded before the base64 step: from memory, followed in `BasicAuth`) and 5.2 (the error codes; the response is read as JSON with an `error` member, and `400`/`401`
as the refusals) *(5.2 read in the slice 2 review, 2026-10-14, below)*; RFC 9207 (`iss` on the redirect) is not implemented and was not read *(read and implemented in the slice 2 review, below)*; which real authorization servers omit `expires_in`, send it as a string or a float, return `200` with an `error`, or require
`client_secret_post` (all handled defensively); whether the platform's TLS verifier (`rustls` with the system roots) accepts a real authorization server's certificate in the image (every test connects over `http` to the
loopback); how an authorization server that detects refresh-token reuse behaves when two replicas present one token at the same instant (the mock has no reuse detection: "kept open" point 3 stands) *(the mock has it since the slice 2 review, and the claim of departure 37 keeps the token from being presented twice)*.

**Open for the next slices.**
1. **`orch-secrets-pg`** *(built in slice 3, 2026-10-14: it passes `secret_store_conformance!` with a breaker, and each of the three guarantees below has a case of its own in its tests)* must pass `secret_store_conformance!`, and the broker relies on three of its guarantees: `rotate` of a missing secret is a `Conflict` (a revoke racing a refresh), a version is never reused for a key,
   and a `get` that returns `KeyUnknown` or `Corrupt` is an error the broker maps to `Unavailable` (and, in `revoke`, still deletes the row).
2. **The API (slice 4)** orders its writes around the broker: `store` or `complete_oauth`, then `replace` of the definition with the returned `CredentialRef` and state `connected`; an edit of the URL or the kind is `revoke`,
   then `replace` as `added`; a delete is `revoke`, then `delete`. It validates what a person types before `store` (departure 33), shows `public_reason()` found through `egress_refusal` and never an `Unavailable`'s
   `Debug`, keeps the table `state` to (owner, server) with the expiry `OAuthStart::expires_at`, passes the callback's `iss` to `complete_oauth` as it came and shows `OAuthStart::issuer` to the person, and rate-limits `begin_oauth` *(amended 2026-10-14, slice 2 review: the broker checks `iss`, the API does not interpret it; departure 36)*. A server whose `lease` is `NeedsReauth` while the definition says
   `connected` (a time-based end the broker marks, or a URL mismatch) is `needs_auth` in the listing.
3. **The relay (slice 5)** calls `lease` per `tools/list` and `tools/call`, and sets `needs_reauth` itself on a `401` from the server (the broker learns only of a refresh); `Unavailable` maps to `broker_down` or, with an
   `egress_refusal`, `egress_refused`. A refresh holds the single-flight lock for up to the request timeout (10 s), so a slow authorization server delays the other leases of that one server.
4. **The composition root (slice 4)** builds the broker with the `userToolServers.egress` policy (`allowing_http` only outside production), the clock, a `client_metadata_url` and a `client_name` from the configuration, and
   serves the Client ID Metadata Document at that URL (its `redirect_uris` must contain the callback and the desktop's loopback listener); with no `secrets` section it needs a broker that refuses every call.
5. **Decisions for the owner**: a confidential pre-registered client (a client secret typed by the person; departure 20), the `WWW-Authenticate` probe (23), caching or deleting dynamic registrations (25), an invalid-input
   class on `CredentialError` (33), and `Authorization: Basic` for an `api_key` (slice 1's open point 2, unchanged).

## Amended 2026-10-14, slice 2 review

The review of [pull request 251](https://github.com/vymalo/another-agentic-system/pull/251) (security) found two major points and seven minor ones, all fixed in the same pull request. Nothing is wired into the binary; decisions 1 to 5 and the amendments above stand, with the departures marked *(Amended)* or *(Revised)* where they stood. What it changed in the port is two lines (`complete_oauth` takes `iss`, `OAuthStart` names the `issuer`) and in the testkit's fixture four required methods (below). Departures continue the numbering, 36 to 46.

**Major 1: the mix-up defence could not be applied by anybody.** A person's server names its authorization server in its own metadata, so a person with several servers talks to several, some a stranger's: the situation in which RFC 9700 requires a defence against mix-up and names the `iss` parameter of RFC 9207. Discovery checked the issuer and dropped it, the pending flow had none, and the port carried none.

```mermaid
sequenceDiagram
  autonumber
  participant P as Person's browser
  participant B as Broker
  participant H as Authorization server of the flow
  participant X as Another authorization server
  B-->>P: authorize_url at H, state, issuer H (kept sealed with the flow)
  P->>X: tricked into the request of the flow
  X-->>P: redirect with code of X, the state, iss = X
  P->>B: complete_oauth(state, code of X, iss = X)
  Note over B: iss is not the issuer of the flow, refused, nothing spent
  B-->>P: Unknown
  Note over B,H: the code of X is sent to nobody, and the flow of H is as it was
```

```mermaid
stateDiagram-v2
  [*] --> Pending: begin_oauth keeps the issuer and whether it advertised iss
  Pending --> Pending: complete_oauth with an iss that is refused, nothing is spent
  Pending --> Used: complete_oauth with the state and an acceptable iss
  Used --> [*]: revoke
  Pending --> [*]: revoke
```

36. **`complete_oauth(owner, server, state, code, iss: Option<&str>)` and `OAuthStart { .., issuer }`.** The broker, not the API, applies RFC 9207, section 2.4, because it is the one that holds what the comparison needs: the issuer of the authorization server it sent the person to (the `issuer` of the metadata, which discovery showed is the one the resource named) and whether that server advertised `authorization_response_iss_parameter_supported` (only a JSON `true` counts). The comparison is `==` on the strings (simple string comparison, RFC 3986, 6.2.1). The API passes the `iss` of the redirect as it came, decoded, and shows `issuer` to the person. The rule, in the trait's module docs and the broker's `PendingFlow::check_issuer`:

    | The server advertised | `iss` of the callback | Result |
    |---|---|---|
    | yes | equal | accepted |
    | yes | another | refused (MUST) |
    | yes | none | refused (MUST) |
    | no | one | refused (the RFC says SHOULD discard and leaves the policy local; this broker refuses) |
    | no | none | accepted (the server takes no part; the defence is the `state` alone) |

    A refusal is `Unknown` (it is not a callback of this flow), a warning with a fixed reason and the server's id and never the value (a stranger chose it), and **spends nothing**: the check is before the claim, so the code is sent to nobody and the person's own callback still completes the flow. The testkit has the fixture's server take part or not (`IssParameter`), `Consent::iss`, `issuer()`, and `other_authorization_server_consent` (the browser tricked into a second, honest authorization server, a second mock on its own port); six cases pin the table and the mix-up. *Residual:* a server that does not advertise `iss` and an attacker who is that server get no protection from this check; for them the `state` and the issuer-bound endpoints (all taken from the one metadata document whose `issuer` matched) are what there is, which is RFC 9700, 4.4.2's own limit: "just storing the authorization server URL is not sufficient". An `error=` callback is not a call; its text is shown as unverified (RFC 9207, 2.4).

**Major 2: a rotating refresh token was presented twice, and a lost race was read as a revoked grant.** (a) After `invalid_grant`, `end_grant`'s compare and swap lost when another replica had rotated, the loss was ignored, and the code went on to mark the definition `needs_reauth` and return `NeedsReauth`. (b) `rotate` prevents a lost update, not a second request: two replicas that read the same refresh token both asked.

```mermaid
sequenceDiagram
  autonumber
  participant A as Replica A
  participant B as Replica B
  participant S as SecretStore
  participant X as Authorization server (rotates, detects reuse)
  A->>S: get(credential) at v1, stale
  B->>S: get(credential) at v1, stale
  A->>S: rotate(v1, grant with refreshing_until): v2
  B->>S: rotate(v1, ...): Conflict
  A->>X: refresh_token R1, once
  B->>S: get(credential) at v2: claimed, token expired, wait with a backoff
  X-->>A: access token, R2
  A->>S: rotate(v2, tokens with R2 and no claim): v3
  B->>S: get(credential) at v3: fresh
  Note over B: lends A's token, asks nobody
```

```mermaid
stateDiagram-v2
  [*] --> Unclaimed: tokens kept
  Unclaimed --> Claimed: rotate with refreshing_until, one replica wins
  Claimed --> Unclaimed: tokens kept, or the claim released after a failure
  Claimed --> Unclaimed: the claim runs out, another replica takes over
  Claimed --> Ended: invalid_grant, and the credential is still the claim's version
  Claimed --> Unclaimed: invalid_grant, and the credential is not (the other replica's tokens are used)
```

37. **The refresh is claimed before it is asked for; closes "kept open" point 3.** The grant gains `refreshing_until`. A replica that needs a refresh writes it with `rotate` on the version it read, for one request timeout and 10 s (so at most 70 s), and only the replica whose swap lands asks. A replica that finds a claim lends the access token if it has not expired (the 30 s margin is skipped, an expired token is not lent) and otherwise reads the store again, 20 ms doubling to 250 ms, until the version moves or the claim ends, and then claims itself; after one claim's length of *real* time it gives up with `Unavailable`, in case a clock stands still. The claim ends with the tokens that answer it (written with `rotate` on the claim's version, three tries if the store stumbles), is released after a failure, and a refusal first reads the credential again so that a refresh that is no longer the grant's writes nothing: the swap that ends the grant is also checked, and a lost one is no longer ignored. **In a process** the first caller of a key leads and the others follow it (a `watch` of the outcome): they are told how it ended, with the egress refusal if that was why (and that the tokens could not be kept, if the store failed three times after the server rotated them), and ask nobody, so eight leases against a failing or hanging server are one request and one timeout. No lock is added to `SecretStore`: the claim is the broker's record and `rotate` is its only primitive. *Residual:* a replica that stalls longer than the claim (a stopped process, a clock that jumps) can be overtaken, and its rotated token is then presented twice by the replica that took over; the claim's length is the request timeout, which bounds the request, plus 10 s for keeping what came back. The testkit's authorization server rotates with reuse detection (`set_reuse_detection`), three replicas and six leases make one request, and a hang or a failure makes one.

**The minor points.**

38. **A used flow's marker stays; nothing deletes a flow** *(departure 26 amended)*. `complete_oauth`'s last step deleted the flow unconditionally, which could delete a flow begun since, and the credential it wrote could follow a `revoke` that ran beside it. `SecretStore` has no compare-and-delete and adding one is a change to a merged port, so the flow is swapped for the marker `used` (it holds nothing), which stays until the next `begin_oauth` or a `revoke`; an expired flow is ended by a swap on the version read. The write of the credential is under the flight of the key and between two looks at the flow: it must still be the marker this callback wrote, before and after the `put`. A flow that is gone or another means a `revoke` or a `begin_oauth` ran beside it, the tokens are revoked upstream, and after the `put` the credential is taken back by a swap on the version written (not a delete, which could take another authorization's), for the grant without its tokens: **a revoke wins**, in the process and in another (tested with the `put` held at a gate). The happy-path case of the testkit no longer asserts the row is gone; it asserts the state opens nothing a second time.
39. **The `resource` is the definition's URL; no scope unless the person named one** *(departure 30 revised)*. The metadata is a stranger's, and what is sent to the authorization server is chosen from the definition. `scopes_supported` is not read. The MCP specification's scope selection would use it; this is a departure for the owner to confirm (below), and a server that needs a scope shows the person which to name.
40. **A client the broker registered is used again for an hour** *(departure 25 amended)*, for the same issuer and redirect URI, from the pending flow or the grant, so that a caller that begins in a loop registers once; never at another issuer (it could carry a secret), and not longer than an hour (a registration the server forgot would otherwise be tried for ever). The randomness and the size of the sealed flow (with the largest client a registration can give: a 2 KiB id and a 1 KiB secret) are checked before anything is registered. The API rate-limits `begin_oauth`.
41. **A grant is for the scopes and the `client_id` the person authorized with**, as it is for the URL (departure 15): an edit of either is `NeedsReauth`, with no refresh (a conformance case). The scopes compare as a set.
42. **Bounds on what a stranger sets and a deployment tunes.** A URL in the metadata over 2 KiB, an issuer over 2 KiB, an authorization request over 8 KiB, a client id over 2 KiB, a client secret over 1 KiB: refused as `Unavailable` with a fixed phrase. `BrokerConfig` is checked against upper bounds: lease 1 h, flow 1 h, refresh margin 10 min, request and connect 60 s, response 1 MiB, the document URL 2 KiB. The bounds are the review's numbers where it gave them; the margin's and the flow's are the author's.
43. **`expires_in: 0` is read as nothing said**: some servers send it for a token that does not expire, and "ends now" would make every lease a refresh.
44. **Only the error codes the RFCs define are logged**: RFC 6749, 5.2 (`invalid_request`, `invalid_client`, `invalid_grant`, `unauthorized_client`, `unsupported_grant_type`, `invalid_scope`) and RFC 8707 (`invalid_target`); anything else, however well formed, is `unknown`. The text `error` may hold is "%x20-21 / %x23-5B / %x5D-7E" (RFC 6749, 5.2), so a refresh token fits; the hygiene test plants a sentinel in it.
45. **The refusal warnings name the server's id** (not a secret); the dead `set_fragment` of the authorization endpoint is gone (a fragment is refused by the guard: RFC 6749, 3.1).
46. **The tests** now show which endpoint is refused at which address (the metadata address with an allow list that names it, so that the class of the address decides, for the token, registration and revocation endpoints), an oversize body with no `Content-Length` and a body that drips, and the tripwire says on stderr that it proved less when `127.0.0.2` cannot be bound (`ORCH_TEST_REQUIRE_TRIPWIRE=1` makes it a failure).

**How the new tests were checked to be able to fail.** Each of these changes was made in turn and made at least one test fail, then reverted: a missing `iss` accepted, an `iss` accepted from a server that did not advertise it, any `iss` accepted, a comparison that ignores case, no claim written, followers that do not follow (every caller leads), no re-read and no checked swap at a refusal, one try of `rotate` instead of three, a release that does nothing, a valid token not lent while another replica refreshes, an error code logged as the server wrote it, the document's `resource` sent, no reuse of a client, a reuse that ignores the issuer, no size check of the flow before the registration, no 2 KiB limit, no scope and client binding, zero seconds as a lifetime, no limit on a chunked body, no request timeout (the dripping body), no look after the write of the credential, no look before it, a taken-back credential left as it was, followers told the tokens were kept when they were not.

**Facts, verified 2026-10-14** (by fetching <https://www.rfc-editor.org/rfc/rfcNNNN.txt>; quotes are exact):
- **RFC 9207**, 2.4: "If the value does not match the expected issuer identifier, clients MUST reject the authorization response and MUST NOT proceed with the authorization grant"; the comparison "MUST use simple string comparison as defined in Section 6.2.1 of [RFC3986]"; "Clients MUST reject authorization responses without the iss parameter from authorization servers that do support the parameter according to the client's configuration. Clients SHOULD discard authorization responses with the iss parameter from authorization servers that do not indicate their support for the parameter. However, there might be legitimate authorization servers that provide the iss parameter without indicating their support in their metadata. Local policy or configuration can determine whether to accept such responses"; "For error responses, clients MUST NOT assume that the error originates from the intended authorization server." 3: `authorization_response_iss_parameter_supported` is a boolean, "If omitted, the default value is false." 2: the value "MUST be a URL that uses the "https" scheme without any query or fragment components."
- **RFC 9700**, 2.1: "When an OAuth client can interact with more than one authorization server, a defense against mix-up attacks (see Section 4.4) is REQUIRED", with the `iss` parameter of RFC 9207 as the first option; 4.4.2: "clients MUST store, for each authorization request, the issuer they sent the authorization request to and bind this information to the user agent" (the binding to the user agent is the API's table of `state`), and "just storing the authorization server URL is not sufficient"; 4.4.2.1: "the client MUST compare the received issuer identifier to the stored issuer identifier. If there is a mismatch, the client MUST abort the interaction." 2.2.2: "Refresh tokens for public clients MUST be sender-constrained or use refresh token rotation"; 4.14.2: under rotation "The previous refresh token is invalidated ... If a refresh token is compromised and subsequently used by both the attacker and the legitimate client, one of them will present an invalidated refresh token ... it will revoke the active refresh token."
- **RFC 6749**, 5.2: the six error codes above, and "Values for the "error" parameter MUST NOT include characters outside the set %x20-21 / %x23-5B / %x5D-7E." **RFC 8707**: `invalid_target` is registered (an error code of the resource parameter).
- *By tests of this crate, against mock authorization servers of ours* (one that rotates refresh tokens with reuse detection, one that is a stranger's for the mix-up): everything above. No live authorization server was used.

*Unverified:* which real authorization servers advertise `authorization_response_iss_parameter_supported` or send `iss` unadvertised (the policy of 36 refuses the second, which RFC 9207 allows a deployment to accept: a deployment that meets such a server decides, and the change is one arm of `check_issuer`); the length of time real authorization servers keep a dynamic registration (the hour of 40 is a guess to be kept short); that a refresh in 70 s is enough for the replicas' clocks and a loaded store (37's residual).

**Decisions for the owner**, added to the slice 2 list: no scope when the person named none, against the MCP specification's preference for `scopes_supported` (39); an `iss` from a server that did not advertise it is refused, which RFC 9207 allows either way (36); an hour for a registration's reuse (40); the bounds of 42.

## Amended 2026-10-14, slice 3: `orch-secrets-pg`, the `secrets` section, `orchestrator secrets rewrap` and the chart's key are built

The default `SecretStore`, its configuration, its one operator command and the chart's key. **Nothing in the running service uses the store yet**: the service reads and checks the `secrets` section (every role does) and refuses a store its build lacks, and
`orchestrator secrets rewrap` is the one caller (the composition root, the definitions' table, the endpoints and the relay are slices 4 and 5). `orch-core` and the `SecretStore` port are untouched (no signature changed); the broker is untouched.
Decisions 1 to 5, the slice 1 and 2 departures and the amendments above stand, except decision 3's `rewrap` through `rotate` (departure 47). Departures continue the numbering, 47 onward.

| Where | What |
|---|---|
| `orchestrator/crates/secrets-pg` (`orch-secrets-pg`) | `PgSecretStore` (`impl SecretStore`, `migrate`, `rewrap`, `close`, `connect`, `from_pool`), `Keyring`, `KeyId`, `KeyBytes`, `RewrapReport`, `advisory_lock_key`; `migrations/0001_user_secrets.sql`; 62 tests (20 without a database, 16 of `secret_store_conformance!`, 16 and 10 of its own) |
| `orchestrator/crates/config` | `secrets` (`UserSecrets`, `SecretStoreKind`, `UserSecretsPg`, `UserSecretKey`), `Config::validate_user_secrets`, `Secrets::user_secret_keys`; `docs/api/config.md` and `config.schema.json` |
| `orchestrator/bin/orchestrator` | the feature `secrets-pg` (on by default), `src/secrets.rs` (the command), `load_user_secrets`; `tests/secrets.rs` runs the binary as a process against Postgres |
| `deploy/chart` | `orchestrator.userSecrets.{enabled, keys, current}` (off), the property `user_secrets_key`, the mount, the section of the rendered configuration; `render-check.sh` (37 new checks); `deploy.yml` lints, kubeconforms and reads the render through the pinned image once its tag has the section |
| `.github/workflows/orchestrator.yml` | the guard that **an applied migration is never edited** covers `crates/secrets-pg/migrations` as well as the store's |

```mermaid
sequenceDiagram
  participant O as Operator
  participant K as kubectl exec, in the orchestrator's pod
  participant R as orchestrator secrets rewrap
  participant D as Postgres
  O->>O: add property user_secrets_key_2 to the AWS secret
  O->>O: values: keys k1 and k2, current k2, then sync (the pod restarts holding both)
  O->>K: /orchestrator secrets rewrap
  K->>R: reads database.url and secrets (not the agents, not the thread tools' key)
  R->>D: apply the crate's migrations
  loop each secret not under k2
    R->>D: lock the key, open with k1, seal again at the same version under k2
  end
  R-->>O: one line of JSON, exit 0 (65 failed, 69 database, 70 internal, 75 changed meanwhile, 78 configuration)
  O->>O: run it again, every secret alreadyCurrent, then remove k1 from the values and sync
```

```mermaid
stateDiagram-v2
  [*] --> Current: put or rotate seals under the current key
  Current --> Previous: a new key is made current
  Previous --> Current: rewrap, at the same version
  Previous --> Unknown: its key is removed from the file
  Unknown --> Previous: the key is put back
  Unknown --> Current: a put or a rotate replaces the row
  Current --> [*]: delete
  Previous --> [*]: delete
  Unknown --> [*]: delete
```

**Departures from the text above, and choices the build needed** (the owner to confirm; each is small to change).

47. **`rewrap` is not a `rotate`: it re-seals in place at the same version.** Decision 3 said it re-seals "through `rotate`". The broker refreshes by **claiming** the grant with `rotate(read version)` and ending the claim with `rotate(claim's version)`
    (`crates/credential-broker/src/broker.rs`, `claim` and `keep`); a `Conflict` at the second is read as "another replica took over" (`Asked::Changed`) and the refresh token the authorization server has just rotated to is dropped. A `rewrap` that made a new version
    between the two would turn that into a lost grant. So it opens the row with the key it names and seals the same material again, **new nonce, current key, same version**, under the key's lock, and only if the row still has the version the run saw (a row written or
    deleted since is left alone and counted). The port's contract holds: callers compare versions for equality only, and the material behind a version has not changed. The test `a_refresh_that_is_claimed_before_a_rewrap_can_still_keep_its_tokens_after_it` is the scenario, and fails (with two others) if `rewrap` takes a new version. `updated` says when the material last changed and is not touched.
48. **The crate has its own history of migrations.** `orch-secrets-pg` runs `crates/secrets-pg/migrations` through a `Migrator` whose table is renamed (`Migrator::dangerous_set_table_name`, `_orch_secrets_pg_migrations`), so it depends on none of `orch-store-postgres`' schema and the two histories never see each other's versions
    (a shared `_sqlx_migrations` would make each refuse the other's as `VersionMissing`, and both number from 1). A test runs both migrators in one schema, twice each, in both orders and at the same moment. The CI guard that an applied migration is never edited now covers both directories.
49. **Versions are numbered under a per-key advisory lock, from one sequence for the store.** Every write of a key (`put`, `rotate`, `delete`, one row of `rewrap`) is a transaction that takes `pg_advisory_xact_lock` of 64 bits of the SHA-256 of a domain string and the owner and server (each with its length) first, and only then reads, takes
    `nextval('user_secret_versions')`, seals and writes. The version is in the associated data, so it must be known before sealing, and a number taken outside the writers' order could commit after a greater one; under the lock the writers of a key are serialized from lock to commit, take their numbers in the order they land,
    and a key holds the greatest it was given (the testkit's concurrent case asserts it, and fails in two of three runs when the lock is taken out). A failed write leaves a gap and never a repeat. `delete` takes the lock too (so a `rewrap` and a `delete` are ordered), `get` takes none, and a `rotate`
    never opens the old row, so it can replace one whose key is gone. A refused `rotate` takes no version. `advisory_lock_key` is public: anything else that writes the table by hand takes the same lock.
50. **The envelope is four columns, and the associated data has a format byte and eight-byte lengths.** `v`, `kid`, `nonce` (12 bytes) and `ciphertext` (with the 16-byte tag), beside `owner`, `server`, `version` and `updated`, with checks on the lengths and an index on `kid` (so `rewrap` and an operator's
    `SELECT kid, count(*) ... GROUP BY kid` look rows up by key id). The associated data is `v (1 byte) ‖ len(owner) ‖ owner ‖ len(server) ‖ server ‖ version ‖ len(kid) ‖ kid`, every length and the version eight bytes big-endian: the text above said `owner ‖ server ‖ version ‖ kid` and asked for an unambiguous
    encoding; this is one, and it binds the format too. A unit test pins the bytes and another pins the cipher against OpenSSL, so changing either is a migration, not an edit. A row moved to another owner, server, pending flow or version, or whose `kid` is edited (even to a key with the same bytes), does not open.
51. **A key is 64 hexadecimal characters, an id is 1 to 64 of `A-Z a-z 0-9 . _ -`, and a ring has 1 to 16 keys.** The text said "32 random bytes, as `openssl rand -hex 32`"; the configuration refuses anything else by key path and never quotes what it read (`secrets.pg.keys[0].key`), white space around the hex is cut (an `{ env }` value is trimmed, a `{ file }` loses a newline),
    `current` must name a listed key, ids are unique. Two ids may carry one key's bytes in the configuration (the crate's tests need it to show the `kid` is authenticated); the chart refuses it (departure 55). `UserSecretKey.key` is the **fifteenth secret** of the schema (`tests/schema.rs` pins the list; ADR 0034's "fourteen" stands for the time it was written).
    `KeyBytes` is wiped when dropped, its `Debug` is `<redacted>`, and no error of the crate or of the configuration holds a key, a part of one or the text it was read from. The key schedule of the cipher is wiped with the `zeroize` feature of `aes-gcm`.
52. **The command's exit codes.** `0` every secret is under the current key (none failed, none changed under the run); `65` at least one could not be re-sealed (`keyUnknown`: put the key back, or `corrupt`: investigate; the others are done); `69` the database failed (run it again; the rows done count as current); `70` the migrations' history does not match this build, or another internal error;
    `75` nothing failed, but a secret was written or deleted while the run was under way and was left alone; `78` the configuration (no file, no `secrets` section, a key that does not resolve) or a build without `secrets-pg`; `2` a usage error. **`75` is not `0`** because a writer on a replica that still runs the old configuration seals under the old key, so a run that saw a row change
    proves nothing about that row: the operator removes no key until a run exits `0`, and the second run of the procedure is the proof. The report is one line of JSON on stdout, names a failure by server and key id and never the person, and the logs are on stderr. The command reads the database URL and the section and nothing else, applies the crate's migrations first (so it can run before the service has
    ever started), and takes no flag of its own.
53. **The service reads the section in every role, and refuses a store its build lacks.** `secrets.store: pg` without the feature `secrets-pg` is exit 78 naming it (as `artifacts.store`), and the keys are resolved by every role (the chart's one process has them). It does nothing else with them. The feature is on by default.
54. **`PgSecretStore::connect` waits ten seconds for a connection**, not sqlx's thirty (which is also how long it tries a database that is down): the command that connects once would hang that long before it could say `69`, and the broker that will call the store gives up on a request after ten.
55. **The chart: `orchestrator.userSecrets.{enabled, keys, current}`, off by default, a list of keys.** The plan of the slice named one property, `user_secrets_key`; the chart takes a list `[{ id, property }]` (default one key, `k1` from `user_secrets_key`, current `k1`) so that a rotation can be written in values: add `k2` from its own property, make it current, run `rewrap`, remove `k1`. Each key is mounted as
    `/run/secrets/orchestrator/user-secrets-key-<id>` and named in `secrets.pg.keys` as a `{ file }`. **Off, the render has no `secrets` section and asks the AWS secret for nothing, byte for byte**: an ExternalSecret that asks for a property the AWS secret lacks stops the whole Secret syncing (the thread tools' key and the agents' bearers with it), and an image built before the section refuses it (exit 78),
    so turning it on is the owner's step, after adding the property and after the pinned tag has the section (`deploy.yml` reads the render through the pinned image by itself from that tag on, and says so until then). The chart refuses: no key, more than 16, an id outside the alphabet or over 64 characters or used twice, a property used twice (two ids for one key rotate nothing), a missing property (with ExternalSecrets on),
    a `current` that is not a listed id, an `enabled` that is not a boolean. **No Job runs `rewrap` in this slice**: the README gives it as a one-off `kubectl exec deploy/<release>-orchestrator -- /orchestrator secrets rewrap` in the running pod, which has the configuration, both keys and the database URL mounted.
56. **A bound on the volume of seals.** NIST SP 800-38D, section 8.3, bounds the invocations of the authenticated encryption function at 2^32 for a key with random 96-bit nonces; every `put`, `rotate` and `rewrap` of a row is one under the current key, so a deployment at ten million writes a day reaches it in 430 days. The crate's README says so; rotating the key resets it. Nothing counts the seals.

**How the tests were checked to be able to fail.** Each change was made in turn, made at least one test fail, then reverted: no advisory lock in the write path; `rewrap` through a new version; `rewrap` without the check of the version it saw; a version left out of the associated data. The conformance suite's `breaker` is `PgSecretStore::close`.

**Facts, verified 2026-10-14.**
- *By reading the source* in `~/.cargo/registry/src` at the versions of `Cargo.lock`: `sqlx-core` 0.9.0 `src/migrate/migrator.rs` has `Migrator::dangerous_set_table_name(&mut self, impl Into<Cow<'static, str>>)` (the table may be schema-qualified; its documentation warns that changing the name for a database that has a history loses it, which a new crate has not); `aes-gcm` 0.11.1 is `Apache-2.0 OR MIT`, `rust-version` 1.85,
  with features `aes`, `alloc`, `zeroize` and `getrandom` (the last not enabled here: the nonce comes from the workspace's `getrandom`), and brings ten packages to the lock file (`aead`, `aes`, `cipher`, `cpubits`, `ctr`, `ghash`, `inout`, `polyval`, `universal-hash`, and itself).
- *By page reads of the PostgreSQL 16 manual* (the pages were fetched and their text read; wording is a paraphrase unless quoted): "Explicit Locking", advisory locks: transaction-level lock requests "are automatically released at the end of the transaction, and there is no explicit unlock operation"; "Functions", administrative: `pg_advisory_xact_lock(key bigint)` "obtains an exclusive transaction-level advisory lock, waiting if necessary";
  "Sequence Manipulation Functions": "the value obtained by nextval is not reclaimed for re-use if the calling transaction later aborts", so a failed write leaves a gap and never a repeat. The tests ran against PostgreSQL 16.14 (Ubuntu 24.04's package), the major version CI uses.
- *NIST SP 800-38D* (<https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38d.pdf>, text extracted with `pdftotext`), section 8.3: "The total number of invocations of the authenticated encryption function shall not exceed 2^32, including all IV lengths and all instances of the authenticated encryption function with the given key", for the RBG-based construction; section 8: the probability that the same IV and key are used twice shall be no greater than 2^-32.
- *By tests*: the cipher and the associated data against OpenSSL 3.0.13 through Python `cryptography` 49.0.0 (two vectors, the associated data encoded by a separate encoder); `sysexits.h` of glibc 2.39 (`libc6-dev` 2.39-0ubuntu8.7) has `EX_DATAERR` 65, `EX_UNAVAILABLE` 69, `EX_SOFTWARE` 70, `EX_TEMPFAIL` 75, `EX_CONFIG` 78;
  Helm v3.19.0 (checksum checked against get.helm.sh) renders the chart's checks and `helm lint` passes with the keys on; the real orchestrator binary reads the chart's rendered configuration with two keys through `print-config.sh` (`bin` mode) and prints references only.

*Unverified:* that `kubectl exec` of the image's entrypoint (`/orchestrator`, distroless, no shell) works in the pod on netcup (the command was run as a process against Postgres in CI, not in a pod); that the ExternalSecret syncs the extra properties on netcup; which `sha-<7>` is the first to read `secrets` (recorded in the chart's README once the bump after this slice's merge lands);
how the nonce volume behaves over a real deployment's years (departure 56 is arithmetic, not a measurement).

**Open for the next slices.**
1. **The composition root (slice 4)** builds the store from `secrets` (`orchestrator::config::load_user_secrets` already builds the `Keyring`; the service wants the same from its `Loaded`), shares the orchestrator's pool or opens its own, and runs `migrate` at boot beside the thread store's. With no `secrets` section it needs the store and the broker that refuse every call.
2. **A Job for `rewrap`** in the chart (as the migration Job of ADR 0058) if the owner would rather not `kubectl exec`.
3. **Decisions for the owner:** `75` as a non-zero exit for a secret that changed under the run (departure 52); a list of keys in the chart rather than one property (55); the format byte and eight-byte lengths of the associated data (50); 16 keys at most (51); ten seconds for a connection (54).
