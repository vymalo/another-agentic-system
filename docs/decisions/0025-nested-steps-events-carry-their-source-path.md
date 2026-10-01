# ADR 0025 — Nested steps: events carry their source path

- **Status:** accepted (2026-10-01), on the owner's delegation: the extension's URI is decided in the
  [status note](#status-note-2026-10-01-accepted-on-the-owners-delegation). The owner may revisit it.

## Context

"Too many tool calls make the UI unreadable" (owner, 2026-10-01). In one real thread, 7 messages
produced 336 status events, 197 of them OpenCode's. The work has a shape: orchestrator → agent
(adam-coder) → sub-agent (OpenCode over ACP) → its commands. The chat flattens it
([vision](../vision.md#5-nested-steps)).

- An agent's progress reaches the log as `agent_status` with a free-text `detail`
  ([`core/src/event.rs`](../../orchestrator/crates/core/src/event.rs)), e.g. "opencode: …". Nothing
  says who produced it or under what.
- The web draws one flat step list per turn ([`lib/steps.ts`](../../web/src/features/chat/lib/steps.ts)).
- AG-UI 1.0 can already carry a tree: `SUBAGENT_STARTED` has `parentSubagentRunId` "for nested
  delegation", and events can carry the `subagentRunId` they belong to (*verified 2026-10-01*, the
  vendored [`ag-ui-1.0.schema.json`](../../orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json)).
  The verifier is shown as a subagent today.

## Decision

1. **Every step event carries its source path**: the chain of step ids from the thread's agent down
   to the producer, plus the step's own id, kind (sub-agent, tool, command, message), label, state
   and optional icon. The orchestrator is the root.
2. **Agents report steps through an optional A2A extension**, detected from the card (the
   [ADR 0008](0008-platform-integration-via-a2a-extension.md) pattern): the status message's metadata
   under the extension's URI holds the step (id, parent id, kind, label, state, icon). An agent
   without it is one level, as today, its detail text shown under the agent.
3. **In the log** this is a new event kind, `agent_step`, beside `agent_status` (ADR 0004, ADR 0001).
   Progress updates of one step coalesce: the log keeps a step's start, its end and a bounded number
   of updates, so a chatty sub-agent cannot fill it.
4. **In AG-UI**, a sub-agent step becomes `SUBAGENT_STARTED` with `parentSubagentRunId`, and its
   children carry its `subagentRunId`; tool and command steps are activities attributed the same way.
   Agents mentioned into a thread (ADR 0026) nest the same way.
5. **In the web**, steps are a tree. Each level is collapsed by default and shows a one-line summary
   and a spinner while it works. A click shows a little more (the latest few children); another click
   shows more, as a virtual list. Errors stay visible at every level. "Cute, sober, but still rich."

## Consequences

- A turn with hundreds of commands reads as a few lines until someone opens it.
- Agents that want nesting must adopt the extension (adam-rs first; its OpenCode bridge reports ACP
  tool calls as child steps).
- Tool calls through an orchestrator relay (ADR 0024, if chosen) can be steps without the agent's
  help.
- Required elsewhere: the extension contract, the `agent_step` event and migration, the projection,
  goldens and conformance, the web tree, adam-rs.

## Alternatives rejected

- **Parsing "opencode: …" prefixes.** Fragile, and it gives one level only.
- **Hiding sub-agent detail entirely.** The person loses the ability to see what ran; the owner wants
  more on demand, not nothing.
- **Every update as its own event, uncoalesced.** It is today's volume problem in the log.

## Status note, 2026-10-01: accepted on the owner's delegation

The owner delegated the points this ADR left open so that the MVP can be completed (2026-10-01). They were
decided on that delegation as follows; the owner may revisit them.

- **The extension's URI** is `https://agents.vymalo.com/a2a/extensions/steps/v1`, after the release-channels pattern
  of [ADR 0008](0008-platform-integration-via-a2a-extension.md); its contract is a page under
  [`docs/api/`](../api/README.md), written with the slice that builds it (MVP slice 5).
- **Steps the orchestrator reports itself.** A tool call relayed on the per-thread MCP endpoint
  ([ADR 0024](0024-mcp-tools-attached-per-conversation.md)) is a tool step with the server's icon, and an agent asked
  through `ask_agent` ([ADR 0026](0026-agent-mentions-as-structured-references.md)) is a sub-agent step under the step
  of the agent that asked; neither needs the agent's help.
