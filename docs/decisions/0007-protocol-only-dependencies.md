# ADR 0007 — Protocol-only dependencies: an agnostic orchestration layer

- **Status:** accepted (2026-09-28). Amended (2026-09-29) by
  [ADR 0012](0012-ag-ui-user-facing-protocol.md): the *In* row gains AG-UI (server), the
  user-facing protocol; the chat API keeps its resource operations and its interaction
  operations are deprecated. Generative UI is A2UI, a standard A2A extension
  ([ADR 0013](0013-a2ui-generative-ui.md)).

## Context

Agent hosting — identity, revisions, runtimes, sandboxes, tools, credentials —
now has its own home: **another-agentic-platform**. This system must work with
agents from that platform, from kagent, or from anywhere else, and be consumed
itself by other systems.

## Decision

This system depends on **protocols only**:

| Direction | Protocols |
|---|---|
| In | A2A (server), MCP (server), AG-UI (server, ADR 0012), webhooks, chat API (resources; interaction deprecated), timers |
| Out | A2A (client), MCP (client), webhooks, chat, OpenAI-compatible model endpoint (ADR 0005) |

It has **no dependency on any agent host**: no Kubernetes API access, no
platform database, no host-specific SDK. An agent is an A2A agent-card URL.

Its processes are **stateless**; the only durable state is the job ledger and
event log in Postgres (the chat), because jobs outlive processes (ADR 0001).

## Naming

This system is an **orchestration layer**, not a "harness": in
another-agentic-platform, *harness* means an agent's internal framework
(`AgentConfig.spec.harness`, e.g. ADK-Rust → ACP → OpenCode).

## Consequences

- Worker sandboxes, toolchain images and agent credentials are the agent
  host's concern, not this repository's.
- Host-specific conveniences are allowed only as **optional, capability-detected
  extensions of a standard protocol** (ADR 0008), never as a hard dependency.
