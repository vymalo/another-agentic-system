# ADR 0022 — The platform provisions A2A agents; the system discovers them

- **Status:** accepted (2026-10-01), on the owner's delegation: decision 6 (open question 39) is decided in the
  [status note](#status-note-2026-10-01-accepted-on-the-owners-delegation). The owner may revisit it.

## Context

Agents come from a static file read at startup (`AGENTS_FILE`,
[`orchestrator/agents.example.yaml`](../../orchestrator/agents.example.yaml)); the first entry is the
default ([ADR 0014](0014-adam-coder-default-agent-over-a2a.md)). The only thing the system knows
about another-agentic-platform is the release picker
([ADR 0008](0008-platform-integration-via-a2a-extension.md)).

The owner (2026-10-01): another-agentic-platform "intend[s] to give a seam to manage (system prompt,
configs, promote, tag, ...) multiple A2A dynamically, and -system should understand that", and "the
platform should not know about the UI normally, it just provisions A2A-capable Agents"
([vision](../vision.md#1-the-platform-is-the-agent-registry)).

A2A names three ways to discover agents: a well-known URI per server, curated registries, and direct
configuration; it "does not prescribe a standard API for curated registries" (*verified
2026-10-01*, <https://a2a-protocol.org/latest/topics/agent-discovery/>). The platform plans an API
catalog per service (`/.well-known/api-catalog`, its §12) but no fleet-wide listing yet.

## Decision

1. **The orchestrator talks to agents only over A2A.** This restates
   [ADR 0007](0007-protocol-only-dependencies.md): an agent is an agent-card URL, whoever hosts it.
2. **The list of agents comes through a port, `AgentRegistry`**, in the `ports` crate with a
   conformance testkit ([ADR 0009](0009-swappable-implementations-at-build-time.md)). It returns
   agent entries (id, display name, card URL, credentials reference, gate). Two implementations:
   - **static**: today's `AGENTS_FILE`, unchanged;
   - **platform**: reads the platform's registry live, so agents added, retagged or promoted in the
     platform appear without a restart.
   A deployment picks one or composes both (static entries first) at build time and by
   configuration.
3. **Read live, never cached in the database** (ADR 0008 rule). The registry is read when a list is
   needed, honouring HTTP cache headers. A registry that cannot be read fails closed: the static
   entries stay, platform entries are absent, and the UI says the registry is unreachable.
4. **Releases and tags stay on the agent card** (the release-channels extension of ADR 0008). The
   registry says which agents exist; each card says which releases it offers.
5. **The platform knows nothing about the UI.** No catalog, component, thread or chat concept crosses
   to the platform. Management (system prompt, configuration, promotion, tags) happens in the
   platform; the system shows the result and may link to the platform's own UI.
6. **The registry format is agreed with the platform** as an extension contract versioned by URI,
   like release-channels. The candidate is a linkset at a platform URL listing one agent card per
   agent service, in the `api-catalog` linkset shape the platform already plans per service (that
   this is RFC 9727's shape, and that it fits, are *unverified*). It is open question 39.

## Consequences

- The default agent can no longer be "the first line of a file" for platform entries; the default
  marker question (open question 23) has to be answered.
- A thread names its agent by id; if the platform removes that agent, the thread's next message fails
  with a clear error, as for an unknown agent today.
- Required elsewhere: the `AgentRegistry` port and testkit, the platform adapter crate, a registry
  mock in compose, and the platform-side contract (another-agentic-platform).
- Authentication to the registry joins open question 11.

## Alternatives rejected

- **The system manages agents itself** (stores prompts, configurations). It would duplicate the
  platform and break ADR 0001's "only the job ledger and event log persist".
- **Kubernetes API access to the platform's CRDs.** Ties the system to one host and its RBAC;
  contradicts ADR 0007.
- **Agent-to-agent discovery only through each card's links.** There is no root to start from.

## Status note, 2026-10-01: accepted on the owner's delegation

The owner delegated the points this ADR left open so that the MVP can be completed (2026-10-01). They were
decided on that delegation as follows; the owner may revisit them.

- **The registry format (decision 6, open question 39).** A versioned contract in another-agentic-platform,
  [`docs/extensions/agent-registry-v1.md`](https://github.com/vymalo/another-agentic-platform/blob/main/docs/extensions/agent-registry-v1.md)
  (AD-021 there, [another-agentic-platform#8](https://github.com/vymalo/another-agentic-platform/pull/8)), named by
  the URI `https://agents.vymalo.com/registry/v1`: a JSON linkset in
  the `api-catalog` shape that lists one agent-card URL per agent service as an `item` link, with the service id and
  optional tags, and nothing else. Releases stay on each card (decision 4) and no UI concept is in it (decision 5). An
  api-catalog is `application/linkset+json` with the profile `https://www.rfc-editor.org/info/rfc9727`, and lists its
  members as `item` links (*verified 2026-10-01*, <https://www.rfc-editor.org/rfc/rfc9727.html>); attributes of a link
  target beyond the registered ones are arrays (*verified 2026-10-01*, <https://www.rfc-editor.org/rfc/rfc9264.html>,
  section 4.2.4.3). The document names the contract with a `profile` link to its URI, so a client refuses a version it
  does not know.
- **Tags and releases (decision 4, reworded).** A registry item may carry optional `tags`: labels the platform keeps
  on the service, for the UI to display and to filter by; they route, authorise and select nothing. Decision 4's
  "releases and tags stay on the agent card" therefore reads: **releases (and their channel tags) stay on the agent
  card** (release-channels), and a registry item's `tags` are not releases.
- **The implementations (decision 2).** The port `AgentRegistry` is in `orch-ports` with its conformance testkit,
  beside the static list and the composition of two registries (static entries first; when both list an id, the static
  entry wins). The platform reader is a crate of its own, and compose has a WireMock registry (MVP slice 9,
  [`mvp.md`](../mvp.md#the-new-build-order)).
- **Fail closed (decision 3)** applies to the list: a registry that cannot be read lists none of its agents and the UI
  says so. A delegation to one of its agents is retried while it cannot be read, and fails only when the registry
  answers without that agent.
- Still open: authentication to the registry and its agents beyond a bearer token (question 11) and the default
  marker (question 23).
- **Built (2026-10-01): MVP slice 9, sys.** The port and its testkit (`orch-ports`: `AgentRegistry`,
  `FixedRegistry`, `CompositeRegistry`, `MemoryRegistry`, `agent_registry_conformance!`), the platform
  reader (`orch-registry-platform`, feature `registry-platform`, set by `AGENT_REGISTRY_URL`, with
  `AGENT_REGISTRY_TOKEN`, `AGENT_REGISTRY_AGENT_TOKEN`, `AGENT_REGISTRY_TIMEOUT_SECS` and
  `AGENT_REGISTRY_MAX_AGE_SECS`) and `GET /api/registry`, which says which source could not be read. Two points the decision left to the build. **The default agent** (open question 23 stays
  open) is the first agent listed, the file's agents first, so a registry never changes it. **The agents of
  the registry run under the deployment's gate**: gate layers and the verifier belong to the agents of
  `AGENTS_FILE`, which the startup checks know. **A delegation** to a registry agent is retried with the
  usual backoff while the registry cannot be read, never dead-lettered for that, and dead-lettered ("agent
  '<id>' is no longer listed") only when the registry answers without the agent; a person's request that
  names an agent only the registry could list is a 503 while it is down, not a 404.
