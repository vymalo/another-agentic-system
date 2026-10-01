# ADR 0022 — The platform provisions A2A agents; the system discovers them

- **Status:** proposed (2026-10-01)

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
