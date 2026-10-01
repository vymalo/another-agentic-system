# ADR 0026 — Agent mentions as structured references

- **Status:** proposed (2026-10-01). The reference is proposed; **who coordinates is not decided**
  (open question 34).

## Context

The owner's example, a message to the chosen agent (2026-10-01): "Help me understand football in
Europe from 2011 till 2019. @researcher first check for data from that period and @browser you look
for pictures to illustrate this experiment. And then @coder will plot the whole thing."
([vision](../vision.md#4-several-agents-in-one-message-by-mention)).

Today a thread has one agent, chosen when it starts; a verifier agent can review the worker's commit
([ADR 0018](0018-verification-gate-and-rework-loop.md)). The orchestrator has no model, so it cannot
read "first … and … then" by itself. Agents come from the registry
([ADR 0022](0022-platform-provisions-agents-system-discovers-them.md)).

## Decision

1. **The composer autocompletes mentions** from the registry's agents and sends **structured
   references**, never raw `@researcher` text: for each mention, the agent id, its card URL, the
   label shown, and where it sits in the text.
2. **The orchestrator checks them before writing anything.** A mention of an agent the registry does
   not know is refused (422), like an unknown agent today. The references are part of the
   `user_message` event.
3. **The addressed agent receives them** through an optional A2A extension, detected from the card
   (the [ADR 0008](0008-platform-integration-via-a2a-extension.md) pattern): the message text keeps
   the labels, and the extension's metadata carries the references. If the addressed agent does not
   list the extension, the UI says so before sending.
4. **Every agent that works on the message appears nested** under the step that started it
   ([ADR 0025](0025-nested-steps-events-carry-their-source-path.md)), and every A2A call goes through
   the orchestrator, so the log stays the one record (ADR 0001).

### Who coordinates: options, not decided

| Option | How | For | Against |
|---|---|---|---|
| **A. The addressed agent coordinates** | It reads the message, then asks the orchestrator to run the mentioned agents (for example through the orchestrator's MCP `start_job`, [ADR 0019](0019-mcp-server-over-streamable-http.md), with the thread as parent) and combines the results. | No AI in the orchestrator; the agent understands "first", "and", "then". | Every agent that can be addressed must know how to coordinate, and needs a credential back to the orchestrator. |
| **B. A planner agent** | A message with mentions goes first to a configured planner agent, which returns a plan (who does what, in sequence or in parallel); the orchestrator runs the plan. | The plan is data in the log, visible and replayable; this is MVP step 4's planner. | One more agent and a plan format to define; a plan can be wrong before anything runs. |
| **C. The orchestrator runs an explicit sequence** | The order is given structurally (the order of mentions, or a sequence the UI builds), and the orchestrator sends the message to each agent in turn with the previous results as context. | Deterministic, no model needed. | Cannot read "in parallel" or "then" from text; rigid for real requests. |
| **D. Agents call each other directly over A2A** | The addressed agent calls the others itself. | Nothing to build here. | The system sees nothing: no steps, no log, no gate. Listed for completeness; it contradicts point 4. |

A and B can coexist (an agent may coordinate, or hand off to the planner). The choice waits for
open question 34.

## Consequences

- The UI, the log and the agents agree on who was mentioned, whatever the label says.
- Mentioned agents' work is visible and verifiable like the main agent's.
- Required elsewhere: the composer's autocomplete over the registry, the reference in
  `user_message`, the extension contract, and the coordination path once chosen.

## Alternatives rejected

- **Raw `@name` text parsed by agents.** Names collide and change; the system could not tell which
  agent was meant.
- **One thread per mentioned agent.** The person asked one question; the answer should be one
  conversation.
