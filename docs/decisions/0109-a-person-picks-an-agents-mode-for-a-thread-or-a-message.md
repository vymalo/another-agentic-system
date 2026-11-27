---
sidebarTitle: "0109 · Pick an agent's mode"
---

# ADR 0109 — A person picks an agent's mode, for a thread or a message

- **Status:** **proposed 2026-11-27** (card ANR-979). **adam's side is built** (adam-rs `2c2b551d` (2026-11-27, "feat(a2a): agent-mode/v1 — folder modes, the extension, activation (#465)")); **the orchestrator's side is not**: no card read, activation, check, `422`, log member or web control. **Contract:** [`docs/api/agent-mode-v1.md`](../api/agent-mode-v1.md) (it holds the sequence and state diagrams).
  **Pattern:** [ADR 0086](0086-a-person-picks-a-model-or-a-reasoning-level-for-a-thread-or-a-message.md) (the model pick), which this copies on purpose; the tier ([ADR 0061](0061-the-person-picks-an-access-tier-for-each-message-the-agent-keeps-to-it.md)) for the hold rule; [ADR 0008](0008-platform-integration-via-a2a-extension.md) for the pattern. The scenario that will prove it (ANR-913) is *unbuilt*.

## Context

An agent may work in more than one way: OpenCode's `build` and `plan` are the model. adam-rs decided that an agent can declare
**primary modes** (a folder's `modes/<name>.md`: a prompt, `tools:` and `permission:` of its own, over the agent's), one of them the
default, and that a person picks one per message. Its decision, *verified 2026-11-27* (adam-rs `docs/decisions/0039-the-agent-loop-takes-opencodes-structure-and-agents-get-plugins.md`,
section "The owner's answers (2026-10-11)", answer 3, read at `origin/main` `e8809b4d`; the file's last commit is `2c2b551d`):

> the choice travels as an **optional extension of its own**, `https://agents.vymalo.com/a2a/extensions/agent-mode/v1`: the card
> declares `params {modes: [{name, title, description}], default}`, a message carries `metadata[URI] = {"mode": "<name>"}`, an
> unknown mode is `InvalidParams`, and an agent that does not declare it ignores nothing and works as before. The tier still caps
> every mode: a mode's rules only narrow.

The same answer leaves "the orchestration layer's side (the contract page `docs/api/agent-mode-v1.md`, an ADR, the composer's
switch, carrying the mode on its own reworks like the tier)" to this repository. That is this record.

**adam's side is built** (*verified 2026-11-27*, `e8809b4d`): `crates/adam-a2a/src/modes.rs`, `crates/adam-a2a-runtime/src/mode.rs`, and the coder's card lists `build` (default) and `plan`. Its amendment of 2026-11-27 adds: `InvalidParams` already uses the code `mode_not_offered`; the mode is **not echoed** in task metadata (unlike the tier); a mode file's `permission:` is **refused at load** and only `tools:` narrows a mode.

## Decision

1. **The wire is adam's, unchanged.** URI `https://agents.vymalo.com/a2a/extensions/agent-mode/v1`; card `params`
   `{modes: [{name, title, description}], default}`; a message carries `metadata[URI] = {"mode": "<name>"}`, with the URI in
   `A2A-Extensions` and `message.extensions`. A message that picks nothing activates nothing and has no member. Effort and model
   are other extensions ([ADR 0086](0086-a-person-picks-a-model-or-a-reasoning-level-for-a-thread-or-a-message.md)) and are
   independent of this one.
2. **The orchestrator reads the live card, every send (ADR 0008).** A mode is offered only when the card lists the exact URI with
   well-formed `params`: a non-empty `modes` of distinct `name`s, a `default` that is one of them. Anything else reads as "not listed".
3. **A mode the card does not list is refused, typed, before anything is written.** `422 mode_not_offered` with `offered: [names]`
   (empty when the card lists no `agent-mode/v1`); nothing is written and nothing is sent. It fails **closed**, like the model pick:
   **a person's choice is never dropped**, never replaced by the agent's default. A card that cannot be read to check it is `503`
   with `Retry-After`; nothing is written. An agent that lists the extension and answers `InvalidParams` leaves the message
   undelivered, with its reason, and the orchestrator never resends it without the mode.
4. **A thread has a default and a message overrides it.** `PATCH /api/threads/{id}` takes `mode` (additive; `null` clears). The
   client key on an AG-UI run is `forwardedProps["vymalo.mode"]`. **The message wins, then the thread, then the agent's own `default`**
   (which the orchestrator never sends: an unpicked mode is no member) **when a new task starts**. A thread default that the card has since dropped is a `422`
   for the message that resolves to it, naming the mode; never a silent fallback. The `422` code `mode_not_offered` is **aligned with adam's own name** for its `InvalidParams`; `mode` and `vymalo.mode` are this record's, matching the model pick (and `vymalo.mode` is also adam's run-context key `CONTEXT_MODE`, as `vymalo.model` is: no collision).
   **The hold rule is the tier's** ([`access-tier-v1.md`](../api/access-tier-v1.md)): the mode holds until another is picked; the newest message that names one wins from the next step and one that names none leaves it. The ladder applies when a task starts; within a task an unpicked message keeps the task's mode, and clearing the thread default cannot bring a running task back to the agent's default.
   **Reworks and answers mirror the tier:** the orchestrator sends the job's last mode on the messages it sends for the thread (a rework, an A2UI answer, an MCP `start_job`), and nothing to a child's task: a child run starts in its parent's mode, adam's behaviour.
   **The name** on `PATCH` and `forwardedProps` is any non-blank string of at most 64 bytes with no control characters (adam requires only non-blank, and folder file names vary); the card decides what is offered, with the `422`.
5. **What is stored.** The `user_message` holds the resolved mode as its own member `mode: "<name>"` (omitted when none); the thread row holds its
   default (its own column in each store, no event). A redelivery, a fork's first message and an edit that names none carry it; a
   thread's default is not copied into a fork or an export. Only the name is stored: never the mode's prompt, which is the agent's. **There is no echo to log:** adam does not put the mode in task metadata, so there is no `agent_status.mode`.
6. **The capabilities document says it.** The agent's entry in `GET /api/agents` and the AG-UI capabilities carry the modes the live
   card lists (`name`, `title`, `description`, and which is the `default`), and nothing when it lists none, so a client draws a switch
   only for an agent that offers one.
7. **The access tier caps every mode** (adam's rule). The orchestrator does not combine them: it sends the tier and the mode as two
   independent members, and the agent keeps to the narrower. A mode narrows by `tools:` only; a `permission:` in a mode file is refused by adam at load, which constrains the `chat` folder of the proof.
8. **The web shows the control only when the live card lists a mode** (a switch beside the tier's), nothing otherwise. Built later.

## Consequences

- Nothing runs differently until an agent lists the extension. A plain A2A agent gets the same request as before.
- `chat-api.yaml` changes are additive only (one optional member on the message and the thread, one error code, one capabilities
  member), when the build lands.
- **Prerequisites of the proof:** the coder pin (`compose.yaml` about line 196, `dev/coder/UPSTREAM` `9f90f1e`) predates adam-rs `2c2b551d`, so a `bump-adam` comes first; and `dev/agents/chat/agent/` needs a `modes/` folder with two modes that narrow by `tools:` only.
- **The proof is a scenario, *unbuilt* (ANR-913)** in `dev/e2e-all.sh`: see the page's "Conformance".

## Alternatives rejected

- **A member inside `pick` (ADR 0086):** the mode is a separate extension with its own URI, its own refusal and its own hold rule, so it gets its own `mode` member on the `user_message` and its own column; `pick` is the model and the effort, which hold per message and travel on the model extensions.
- **Folding the mode into the access tier:** a tier is a limit the person sets on what the agent may do; a mode is a way of working
  the agent defines. A mode may be `plan` under the `auto` tier, and the tier caps every mode.
- **A silent fallback to the default for an unlisted mode:** the person's choice would be ignored without a word.
- **Sending the default mode on every message:** it would activate the extension for nothing and pin the agent's choice in the log.
- **Caching the card's modes:** forbidden by ADR 0008; a mode removed in a new release would still be offered.

## Proof (to build)

`dev/mode-real-e2e.sh` (ANR-913, unbuilt): see [`agent-mode-v1.md`](../api/agent-mode-v1.md#conformance). Each test red before its fix.
