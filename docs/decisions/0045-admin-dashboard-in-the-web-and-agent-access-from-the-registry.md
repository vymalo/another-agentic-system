# ADR 0045 — The admin dashboard is an area of the web; who may use a platform agent comes from its registry entry

- **Status:** proposed (2026-10-05), on the owner's request of that day: *"The MVP worked and now we need a dashboard for
  configuring all these. The same one actually."* "The same one" is read as one dashboard inside this repository's web, with
  its sign-in, look and roles, not a second app; that reading is an assumption the owner has not confirmed. **Nothing of this
  is built.** The dashboard itself (its Platform API, the custom resources it writes, its screens and slices) is specified in
  another-agentic-platform, architecture §60a "Admin dashboard v0" (AD-025, proposed P-007 to P-012); this ADR is the system's
  side of it: decision 1 is P-009 there, decision 2 is P-007 there. The owner's questions, each with its recommendation, are
  in §93 of that repository, group *Dashboard v0*. Extends [ADR 0022](0022-platform-provisions-agents-system-discovers-them.md)
  and [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md); would amend the bullet "The web never talks to the
  orchestrator server to server" of [`architecture.md`](../architecture.md) when decision 2 is built.

## Context

The owner wants one dashboard to configure what is set today in Helm values in home-os, in Keycloak and in AWS Secrets
Manager: one coder per GitHub owner (`coder-vymalo`, `coder-stephane`, each limited by `GITHUB_APP_OWNERS`), the folder agents,
who may use which agent, models, tool servers, the run-pod size class and the rest. The platform writes agents as custom
resources (`AgentService`, `AgentConfig`, its §59a) and lists them in its registry, `agent-registry/v1`, which the orchestrator
reads live ([ADR 0022](0022-platform-provisions-agents-system-discovers-them.md)).

What exists here (*verified 2026-10-05* at `e5da0a4`):

- **Who may use an agent** is per role: `auth.roles.<role>.agents` in the orchestrator's file lists agent ids or `"*"`
  ([`config.md`](../api/config.md#roles-and-permissions)), checked by `Access::allows(permission, Resource::Agent { id })` in
  `orchestrator/crates/app/src/authz.rs`; the roles are the values of the token's roles claim as the identity provider spells
  them (`Requester::roles`). The file is read once at startup, so a change is a restart. A registry item carries no access
  information: `RegistryEntry` is an endpoint, a name, tags and an origin (`orchestrator/crates/ports/src/registry.rs`).
- **The registry is read live.** `App::list_agents` reads `ports.registry().list()` on every call
  (`orchestrator/crates/app/src/app.rs`); the platform reader keeps a copy at most `max-age`, capped at 60 s
  ([`registry-platform`](../../orchestrator/crates/registry-platform/README.md)); `dev/registry-e2e.sh` (b) asserts that an agent
  added to the mock registry is listed within 10 s with no restart (not run for this ADR).
- **`admin` is operational and content-free** ([ADR 0039](0039-nobody-reads-another-persons-thread.md)): reserved for endpoints
  that show no thread content and no personal data beyond counts, and gating nothing yet.
- **The web never calls a server.** "There are no Next.js API routes, no server-side fetches and no secrets in the web"
  ([`architecture.md`](../architecture.md)). Yet every request to the web carries the person's token: the edge's `forward_auth`
  gets `Authorization: Bearer <ID token>` from oauth2-proxy (`--set-authorization-header=true`) and `copy_headers Authorization`
  puts it on the request to the web as on the one to the orchestrator
  ([`deploy/chart/files/Caddyfile`](../../deploy/chart/files/Caddyfile), [`dev/Caddyfile`](../../dev/Caddyfile)). The web
  ignores it today.

## Decision

### 1. Who may use a platform agent is on its registry entry, and the orchestrator enforces it

1. **The attribute.** A registry item may carry `audience`, an array of strings: values of the roles claim
   (`auth.jwt.rolesClaim`, `agentic_roles` on netcup), or `"*"`. It is the platform's `AgentService.spec.access.audience`,
   proposed as an optional attribute of `agent-registry/v1` (additive under that contract's Versioning; P-009 there).
2. **The rule.** For an agent the platform registry lists, a person may read it (`agent.read`) or invoke it (`agent.invoke`)
   when **both** hold: a role of theirs grants the permission over the id (`auth.roles`, unchanged), **and** the item's
   `audience` holds `"*"` or one of the values of their roles claim, **or** a role of theirs holds `admin`.
3. **Fail closed.** An item with no `audience`, an empty one, or one the reader cannot read (not an array of strings, more
   than 32 values, a value over 64 characters) is for people whose roles hold `admin` only. An agent the dashboard has just
   made is therefore seen and tried by administrators and by nobody else until it is given an audience. The `admin`
   exception is about agents, not content: it reaches no thread (ADR 0039 stands).
4. **Static agents are unchanged.** An agent of the agents file has no audience and keeps today's rule; an id listed by
   both sources is the static one ([ADR 0022](0022-platform-provisions-agents-system-discovers-them.md)'s composite).
5. **Every agent check applies it**, through one predicate in `orch_app::authz`: the list and one agent
   (`list_agents`, `describe_agent`, `GET /api/agents`, the AG-UI capabilities), the default agent, starting a thread,
   sending, forking, a mention and an asked agent (`mentions.rs`, `asks.rs`), and attaching a tool server for an agent. A check
   that has only an id resolves the entry first, as invoking already does to find the endpoint, and a registry that cannot
   be read refuses as it does today. `Resource::Agent` gains the entry's audience, and `RegistryEntry` an `audience` field
   (a change of the port's public struct: the testkit and every adapter move in the same slice).
6. **A thread whose agent the person may no longer use** is read-only for them, as when `agent.invoke` stops covering its
   agent today; it is not hidden, because it is theirs.
7. `GET /api/me`'s `agents` stays the roles' patterns: an audience cannot be said as a pattern, and the web takes its list of
   agents from `GET /api/agents`, which is filtered.

### 2. The admin dashboard is an `/admin` area of the web

1. **Capability-detected, optional, removable** (the pattern of [ADR 0008](0008-platform-integration-via-a2a-extension.md)): the
   web's server has one setting, `PLATFORM_API_URL`. Unset, `/admin` is a 404 and nothing links to it. Set, each page of the
   area asks `GET /v1/info` of the Platform API, live and never cached; a failure is "The platform API cannot be reached" with
   nothing editable. Removing the area leaves the chat as it is.
2. **Drawn for administrators** when `GET /api/me` lists `admin`. It is a hint, never a check (the web's rule since
   [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md)): the Platform API authorizes every request itself, and
   its 403 is shown. Configuration holds no thread, file, message or listing of anybody's, so `admin` (ADR 0039) fits.
3. **One server-side call.** A route handler, `/admin/api/[...path]`, forwards the request to `PLATFORM_API_URL` with the
   `Authorization` header the edge put on it, unchanged, and no other header of the person's. It stores no token and logs none
   (nor the body of an error that might echo one). It is the web's only server-side fetch; the rest of the web keeps calling
   `/api/*` and `/agui/*` from the browser. `/admin/api/*` is not under `/api/*`, so the edge routes it to the web.
4. **Same look, sign-in and roles**: a feature folder `web/src/features/admin/`, the shadcn primitives and DESIGN.md's tokens,
   Playwright with axe against a mock Platform API in `web/mock`.
5. **Read-only views from what exists**: the tool servers a person attaches (`GET /api/tool-servers`), the sharing cap
   (`GET /api/me`), the registry's state (`GET /api/registry`), the public `ui` settings (`GET /api/config`). The orchestrator's
   own configuration stays its file; nothing in this ADR makes it editable from the dashboard.

## Consequences

- The dashboard sets who may use an agent with one field of a custom resource, which reaches people within the registry's
  `max-age` and the reader's cap (30 s and 60 s), with no restart, no write to Keycloak and no write to this repository's
  configuration. People still get roles in Keycloak (a client role such as `team-stephane` on `another-agentic`, which the
  client's role mapper puts in `agentic_roles`).
- A deployment that reads a platform registry and does not give its items an audience sees its platform agents vanish for
  everyone but administrators when this is built. The platform has no deployment yet (its registry is v0 slice S7); the dev
  mock registry's items get an audience in the same slice, and `dev/registry-e2e.sh` asserts both sides.
- The web stops being "no server-side fetches": `architecture.md` and `web/README.md` change in the slice that builds the route
  handler, and the chart gains `web.platformApiUrl`.
- Invariant 2 speaks of "a standard protocol's extension mechanism". The dashboard depends on the platform's own HTTP API, not
  on an A2A extension. It keeps the invariant's other rules (optional, detected live, fail closed, removable, no SDK), and the
  orchestrator never calls the Platform API: the owner is asked to accept this reading.
- Slices (numbered in the platform's §60a): D8 the orchestrator's rule, D9 to D11 the web, D12 the chart.

## Alternatives rejected

- **(a) The dashboard edits Keycloak roles and the orchestrator's `auth.roles`.** It needs a Keycloak admin credential and write
  access to this deployment's configuration in a process that takes browser traffic, a restart for every change, and a second
  writer of a file GitOps owns.
- **The platform filters the registry per person.** The orchestrator reads the registry with one service token for everybody;
  reading it per person would put the registry on every request and needs the platform's control plane. It stays the
  platform's target (its §12b); the audience works with the registry it has.
- **An edge route `/platform/*` to the Platform API**, called from the browser. It keeps the web free of server-side calls, but
  puts a configuration-writing API on the public edge and makes the system's edge route into the platform's namespace. The
  owner may still prefer it (a question in the platform's §93).
- **A second app.** Not what "the same one" is read to mean; it would duplicate the sign-in, the look and the roles.
