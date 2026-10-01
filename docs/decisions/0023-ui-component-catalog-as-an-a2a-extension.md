# ADR 0023 — A UI component catalog, sent at conversation start, with a refetch seam

- **Status:** proposed (2026-10-01)

## Context

The owner wants agents to know which components the person's screen can show, and to answer and
ask through them: lists, choices (a radio list of several questions), steppers, cards, a web view,
an agent suggestion, a skill request, a mermaid graph, images placed in text, a notification
opt-in, and any combination ([vision](../vision.md#2-rich-output-and-rich-input-through-components-the-agent-knows-about)).
"The UI should define components, e.g. by key and params, then let the A2A seam know about them."

**The owner decided (2026-10-01):** "since the components are not dynamic, their params only are,
then it's safe to send the UI components once at the beginning, then have a seam to refetch them,
like a tool. So that a chat across multiple UI versions can get the newest UI items."

What exists ([ADR 0013](0013-a2ui-generative-ui.md)): agents send A2UI surfaces over A2A; the web
validates and renders a vocabulary of ten basic components; the orchestrator advertises only the
A2UI basic catalog in `message.metadata["a2uiClientCapabilities"]`.

A2UI facts (*verified 2026-10-01*, <https://a2ui.org/specification/v0.9.1-a2ui/>): a surface names
its `catalogId`; catalogs are JSON Schema documents whose `$id` equals the `catalogId`; the client
capabilities carry `supportedCatalogIds` and `inlineCatalogs`, "an array of inline catalog
definitions provided directly by the client (useful for custom or ad-hoc components and
functions)"; and the specification has no mechanism for an agent to request or refetch a catalog.

## Decision

1. **The web defines the catalog.** It is an A2UI catalog: one entry per component, a key and a JSON
   Schema of its parameters. It ships with the web build and has a `catalogId` and a content
   **digest** (its version). The basic components the web renders stay available beside it.
2. **The web sends it at conversation start**, in the first run of a thread
   (`forwardedProps["vymalo.uiCatalog"] = {catalogId, digest, catalog}`), and again whenever its
   digest differs from the one the thread last recorded (a newer UI opened the chat). Other runs
   carry nothing.
3. **The orchestrator records it** as a new event kind, `ui_catalog {catalogId, digest, catalog}`,
   once per digest (ADR 0004: a closed-enum variant; ADR 0001: it is part of the log). It is size
   capped and checked as JSON Schema, not interpreted.
4. **Agents get it through an optional A2A extension**, detected from the card (the
   [ADR 0008](0008-platform-integration-via-a2a-extension.md) pattern), on top of the A2UI extension:
   - on the first message of an A2A context, and on the first message after the thread's digest
     changed, the A2UI capabilities list our `catalogId` in `supportedCatalogIds` and carry the
     catalog in `inlineCatalogs`;
   - every other message carries only `{catalogId, digest}` under the extension's URI, so the agent
     can tell its copy is stale.
   An agent whose card does not list the extension gets exactly what it gets today.
5. **A refetch seam.** An agent can ask for the thread's current catalog at any time, and is meant
   to expose this to its model as a tool. The answer is the catalog of the newest digest the thread
   recorded. Its transport (a tool on the orchestrator's MCP surface,
   [ADR 0019](0019-mcp-server-over-streamable-http.md), or a URL in the extension's metadata) is
   open question 36.
6. **Agents turn the catalog into model calls** (for example one tool per component, or one `show`
   tool whose schema is the catalog) and emit A2UI surfaces naming our `catalogId`. This is agent
   work (adam-rs first); the system only defines the contract.
7. **Rich input goes back as A2UI actions** (ADR 0013): a choice, a filled stepper, an accepted agent
   suggestion. An action that answers an agent's question is shown in the chat as the person's
   answer. adam-rs's `ask_user` gains options, shown as the Choices component.
8. **The renderer validates parameters** against the component's schema, on top of ADR 0013's
   limits. A surface that names a component the thread's catalog does not have, or breaks a schema,
   is refused (rule 4 of ADR 0013 holds).
9. **First components:** Choices, Cards, Mermaid. Then List, Stepper, Agent suggestion, Skill
   request. Image placement and Web view wait for open question 38; Notification opt-in waits for
   open question 37.

## Consequences

- An agent learns the screen it is talking to without any change on the platform (ADR 0022: the
  platform knows nothing about the UI).
- A chat opened in a newer UI reaches the agent with the newer catalog; an older UI can receive a
  surface it cannot draw, and refuses it visibly.
- ADR 0013's "vocabulary of ten" becomes "the basic subset plus the thread's catalog"; ADR 0013 gets
  a dated status note when this is built.
- Required elsewhere: the catalog source in `web/`, the `ui_catalog` event and migration, the
  extension contract (`docs/api/`), the A2A adapter, the refetch seam, goldens, and the adam-rs side.

## Alternatives rejected

- **The catalog on every message.** Simple, but it repeats several kilobytes on every turn although
  only parameters change (the owner's point).
- **The orchestrator owns the catalog.** The components are the UI's; putting them in the
  orchestrator couples its releases to the web's.
- **A format of our own instead of A2UI catalogs.** A2UI already has catalogs and inline catalogs;
  the owner's "redo their semantic here ourselves" is met by our own catalog inside A2UI.
- **OpenUI.** Rejected in ADR 0013 for lack of a transport binding.
