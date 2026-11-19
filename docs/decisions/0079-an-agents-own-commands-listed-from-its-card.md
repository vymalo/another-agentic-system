# ADR 0079 — An agent's own commands, listed from its card

- **Status:** proposed (2026-11-19), as slice 11 of
  [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md) ("`/` commands: a menu of the agent's own
  commands") and the contract page [`docs/api/commands-v1.md`](../api/commands-v1.md). The orchestrator's side (the card read
  and the AG-UI capabilities key) and adam-rs's side are built; the web's `/` menu is the next slice.

## Context

[ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md) puts a `/` menu of the agent's own commands in
wave 2, slice 11, and notes that attachments (slice 12) change the same `composer.tsx`. adam-rs already has the agent's half:
a folder agent's `commands/<name>.md` are prompt templates, the card advertises them, and the runtime expands one when a
message invokes it (adam-rs ADR 0044 decision 9; `crates/adam-a2a/src/extensions.rs`'s `COMMANDS_EXTENSION`, `CommandInfo`
and `ExtensionConfig::commands`, and `crates/adam-a2a-runtime/src/commands.rs`). Nothing of it was written on this side: the
orchestrator had no type for the offer and the capabilities document had no key, so the web could not draw the menu.

Two constraints decide the shape. The core is pure and the event log is the only state (invariant 5, ADR 0001): a command
must not add an event, a field or a route. And protocols only (ADR 0007): the orchestrator must not know that the agent is
adam, so what it reads must be plain card data and what it sends must be plain A2A.

## Decision

1. **The extension is `commands/v1`**, URI `https://agents.vymalo.com/a2a/extensions/commands/v1`, optional
   (`required: false`, ADR 0008). The card's `params.commands` is a non-empty array of `{name, description,
   argumentHint?}`: `name` not empty, without whitespace and never starting with `/`; `description` not blank;
   `argumentHint` a string when present. The list is sorted by name and each name once; a client sorts and keeps the first
   entry of a repeated name. A card that lists it with anything else is a card that does not list it (fail closed).
2. **A command is the message's text, not a new field.** A message whose text begins with `/<name>` for a name the agent
   serves is the invocation, whether or not the extension is activated; the rest of the text, trimmed, is the arguments.
   `//name` for a served name is the literal `/<name>`. A `/word` that is not served is plain text, so "/tmp is full" is
   still a sentence. The metadata form (`metadata[<uri>] = {command, arguments?}`) exists for a client that activates the
   extension; the orchestrator neither sends it nor reads it.
3. **The agent expands, the orchestrator does not.** `$ARGUMENTS`, `$1`–`$9` with double quotes respected, `$$`, one pass,
   never re-read; at most 4 KiB of arguments and 32 KiB of expansion; an empty expansion refused; a form's answer is never a
   command. This is the agent's own behaviour and another agent may differ, as its prompt templates differ.
4. **The orchestrator reads the card live and carries the offer in the AG-UI capabilities document**:
   `custom[<uri>] = {commands: [...]}` while the card lists the extension with well-formed params, and no key otherwise
   (ADR 0008). It sends no activation and no metadata for this extension, logs the text as the person sent it, and expands
   nothing. The offer is per agent and never stored in a thread.
5. **The web inserts text.** Its `/` menu lists the commands, with their descriptions and hints, and picking one inserts
   `/name ` (or `/name`) into the composer; the person sends it as the message. Nothing is sent as metadata today.
6. **A name is not a boundary.** A command gives a person nothing their message would not: the agent's tools, credentials,
   access tier and the deployment's limits apply to what the expansion asks for. Card text is untrusted and drawn as text.

## Consequences

- A new pure type in `orch-core` (`COMMANDS_EXTENSION`, `CommandSpec`, `CommandsOffer`, `CommandsListing`), a field on
  `AgentCardInfo` (`commands`, defaulted to `Absent`) and a `CardFacts.commands` member in the projection: the ports change is
  **breaking** for an implementer that builds `AgentCardInfo` literally.
- No migration, no event, no outbox row, no route: the log and the core's transition are untouched, and a plain A2A agent is
  unaffected. The export of a thread shows the invocation, not the text the agent made of it, because the orchestrator never
  sees the expansion.
- The web gets its menu from the capabilities document it already fetches, so the feature needs no new endpoint and follows
  the agent across a redeploy.
- adam's contract page reference in its own source ("not written yet") is satisfied by
  [`commands-v1.md`](../api/commands-v1.md); the orchestrator reads the same card whatever agent serves it, kagent included,
  as long as the card lists the URI.

## Alternatives rejected

- **A core event or a message field for the command** (`user_message.command`). It would put the agent's private vocabulary
  in the log, in every export and in every migration, for something the text already says, and it would make the invocation a
  thing the orchestrator must parse and validate — a permission-shaped surface it cannot enforce.
- **The orchestrator expanding the template.** It would need the template, which is not on the card, and it would make the
  stored message differ from what the person sent; a replay would then be a different message from the one the agent saw.
- **A per-message pick, like the access tier** (`forwardedProps["vymalo.command"]`). A command is not a property of the
  message but its content; a member would duplicate the text and let a client name a command it did not type.
- **Serving the person's prompts from the same menu now.** The prompt library ([ADR 0070](0070-a-persons-prompt-library.md))
  is the orchestrator's and the agent's commands are the agent's; one menu over both is slice P3, and mixing the two stores in
  this contract would tie a person's saved prompts to one agent's card.
- **An MCP prompt endpoint on `thread-tools/v1` instead of a card entry.** It would make the menu need a live tool connection
  before it can be drawn, and it would be a second place to say what the card already says.
