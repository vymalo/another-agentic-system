# ADR 0046 — People add their own MCP servers from the UI; their secrets live in a credential broker, never here

- **Status:** **accepted (2026-10-11)**, on the owner's words of that day: "ADR 0046 should be accepted and built." Every name, default and
  shape marked *proposed* below is **accepted as written**, except what the amendment of 2026-10-11 amends (decision 3: the broker is in the
  orchestrator, behind two ports) and the few points it keeps open (its last section). **Nothing of this is built yet** *(as written on 2026-10-11; slice 1 is built since, see the amendment of 2026-10-14 at the end)*; the owner asked for it to
  be built. The amendment also reads the title's "never here" as *never in the log, an export, a frame, a file or in clear*: a sealed secret is
  now kept here (amendment, and invariant 3's note in `CLAUDE.md`).
  *Amended 2026-10-14: slice 1 of the amendment's list is built (the ports, their testkits, the in-memory stores and the egress guard); see the last section.*

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
     `orchestrator secrets rewrap` (it re-seals each row under the current key through `rotate`), then remove the old one. A row sealed under an id the file does not hold is `KeyUnknown`:
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

*Unverified:* the safe volume of random 96-bit nonces under one AES-GCM key (NIST SP 800-38D sets a limit near 2^32 invocations; from memory, the volume here is tiny); the platform's section (above).

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
4. **`complete_oauth(owner, server, state, code)`** takes the server, which the list of decision 3 did not: the pending flow is kept under `(owner, server)` (amendment, 2), so the broker cannot find the verifier without it. The callback knows the server from the table that maps `state` to the person (amendment of 2026-10-06).
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
`ClientBuilder::resolve_to_addrs` do what the egress docs say a caller uses them for; the slice that writes the broker reads them first.

**Open for the next slices.**
1. **`orch-credential-broker`**: its conformance macro (`credential_broker_conformance!`), over `MemorySecretStore`, `MemoryUserToolServers` and a mock authorization server; how it takes an `EgressPolicy`; a resolver hook for `reqwest`. **The macro must have a case for departure 15**: a credential stored for one URL is not lent after the definition's URL changes (`NeedsReauth`), nor for a definition made again under the same id; a revoked or never-given credential is `NeedsReauth`, a missing server and a `none` kind are `Unknown`; an expired `state` is `Unknown` and a used one too; `revoke` is idempotent and runs before the definition is deleted.
2. **`Authorization` in an `api_key` header** (departure 8): LibreChat offers Basic. Allow it by letting the broker lease `Authorization` with a whole value (`Basic …`), which `ToolServerEndpoint` cannot carry today (it sets `Authorization` from `bearer` only), or keep it out.
3. ~~`ToolSecret` is not zeroized~~ *(amended 2026-10-14, review: it is, departure 18)*. What stays: a lease's value sits in memory for the length of a call, and a request makes copies while it is sent, which the wipe does not cover.
4. **A deployment with no `secrets` section** needs a `SecretStore` and a `CredentialBroker` that refuse every call (`NoArtifacts` is the model); not built, the composition root's slice.
5. **`orch-secrets-pg`**: versions from a sequence (never given twice for a key, across a delete included, in the order the writes land), `rewrap` through `rotate`, and a `breaker` for the testkit that closes its pool. *(Amended 2026-10-14, review: "the limit under a lock per owner" is no longer here; it is the definitions table's, slice 4, below.)*
6. **The definitions table (slice 4)**: revisions from a sequence, the limit of eight under a lock per owner taken before the count (an advisory lock keyed by the owner or `SELECT … FOR UPDATE` on an owner row; the testkit races it twenty times), `replace` and `set_state` as one statement with the revision in its `WHERE`, rows read back through `UserToolServer::from_stored` (a failure is `Corrupt`), and the API showing `EgressRefusal::public_reason()` and never `rule()`.
7. The IPv6 table treats anything outside `2000::/3` as special: a new allocation is added to `egress.rs` when a server lives in it.

## Amended 2026-10-14, review of the slice 1 pull request

The review of [pull request 250](https://github.com/vymalo/another-agentic-system/pull/250) found five major points and twelve minor ones, all fixed in the same pull request before it merged. The departures above carry each change where it belongs, marked *(review)* or *(Amended 2026-10-14, review)*; in short: the lists judge addresses in any spelling and a suffix never lifts the private ban (1, 2); the two checks that could not fail now can (18, 19); a credential is bound to its URL, which is the broker's contract (15); the definitions store has no upsert (13). Also: the lease is a closed enum (3), the version contract is what `rotate` needs (7), the redirect and the state are values (16), the headers a client frames a request with are refused (8), a URL never prints its path (14), the refusal a person sees is one for two causes (17), the limit race is repeated (12), and `ToolServerStatus` and `UnavailableReason` stay in `orch-ports`, with a reason (9). Decisions 1 to 5 stand.
