# ADR 0023 — A UI component catalog, sent at conversation start, with a refetch seam

- **Status:** accepted (2026-10-01), on the owner's delegation: the extension's URI and open question 36 are
  decided in the [status note](#status-note-2026-10-01-accepted-on-the-owners-delegation). The owner may revisit it.

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

## Status note, 2026-10-01: accepted on the owner's delegation

The owner delegated the points this ADR left open so that the MVP can be completed (2026-10-01). They were
decided on that delegation as follows; the owner may revisit them.

- **The extension's URI** is `https://agents.vymalo.com/a2a/extensions/ui-catalog/v1`, after the release-channels
  pattern of [ADR 0008](0008-platform-integration-via-a2a-extension.md): detected from the card, read live, failing
  closed, removable without breaking plain A2A. Its contract is [`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md)
  (accepted; not built yet).
- **The refetch seam (decision 5; the transport of open question 36)** is the tool `get_ui_catalog` on the
  orchestrator's per-thread MCP endpoint, the "thread tools" (extension
  `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`; contract
  [`api/thread-tools-v1.md`](../api/thread-tools-v1.md), accepted; built, see the last status note). The endpoint is
  `/thread-tools/{threadId}/mcp`: a path under the `/mcp` mount of
  [ADR 0019](0019-mcp-server-over-streamable-http.md) would collide with it. The A2A adapter gives the agent the
  endpoint's URL, a token and its expiry in the message metadata under that URI. The token is short-lived (two hours by
  default, 60 seconds to 24 hours by configuration), scoped to the thread, signed with HMAC (HS256) under a key from
  the configuration so that any replica can check it, minted when the message is sent, and never written to the event
  log, the outbox or a log line. Its claims name the thread, the job, the agent, the message, and who is calling
  (`main`, or an asked agent and its depth, for [ADR 0026](0026-agent-mentions-as-structured-references.md)). The same
  endpoint carries the tools of [ADR 0024](0024-mcp-tools-attached-per-conversation.md) and `ask_agent`: inside the
  surface, tools come from the built-in `get_ui_catalog` and from providers composed at build time
  ([ADR 0009](0009-swappable-implementations-at-build-time.md)), listed in that order.
- **Versioning (open question 36).** The catalog has an integer `version`, bumped by hand, beside its digest; the
  newest catalog is the one with the highest `version`. Every digest a thread saw stays in its log as a `ui_catalog`
  event for as long as the thread exists, and the refetch answers the newest. A UI given a surface that names a
  component it does not have shows a visible placeholder, "needs a newer version of the app": refused visibly, rule 4
  of [ADR 0013](0013-a2ui-generative-ui.md). A UI older than the thread's newest catalog sends no catalog.
- **Refinements of decisions 2 and 4** (the contract has the details). `vymalo.uiCatalog` carries `version` too.
  Every message about a thread that has a catalog, to an agent that lists the extension, carries
  `{catalogId, version, digest, inline}` under its URI and lists our `catalogId` first in `supportedCatalogIds`, not
  only the message that carries the catalog, so that an agent that holds to A2UI never thinks the screen lost it.
  `inlineCatalogs` is sent on the message that makes a new digest the thread's current one, and only to an agent
  whose A2UI entry says `acceptsInlineCatalogs: true`; any other agent gets `inline: false` and refetches.
- **Detection.** The orchestrator reads the extensions a live card lists into a closed set (thread tools, UI catalog,
  steps, mentions), and the AG-UI capabilities document lists each one under `custom`, so the web can flag an agent
  before the person sends, as [ADR 0024](0024-mcp-tools-attached-per-conversation.md) (decision 3) and
  [ADR 0026](0026-agent-mentions-as-structured-references.md) (decision 3) require.

## Status note, 2026-10-01: the handshake is built (MVP slice 3)

The web's catalog and Choices (versions 1 and 2) and the orchestrator's side of the handshake are built:
`forwardedProps["vymalo.uiCatalog"]` on a run (checked before anything is written), the `ui_catalog` event (migration
`0006`) and the thread's ledger in the core, `thread.uiCatalog` in the state snapshot, the closed `KnownExtension` set
read from the live card and listed in the capabilities document, and the A2A adapter's metadata, `supportedCatalogIds`
and `inlineCatalogs`. The contract is [`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md). **Not built yet:** the adam-rs
side. One detail the contract now says: an A2A server reads the numbers of message metadata as doubles, so an agent
recomputing a digest writes whole numbers as integers first.

## Status note, 2026-10-01: the refetch seam is built (MVP slice 3)

The thread tools of the decision above are built: the token (`orch-thread-token`: HS256, the ten claims with the caller
as a closed `main` | `ask:<n>`, the current and the previous key, known-answer vectors computed by an independent
implementation and pinned in its tests and in [`api/thread-tools-v1.md`](../api/thread-tools-v1.md)), the endpoint
(`orch-surface-thread-tools`, a machine route at `/thread-tools/{threadId}/mcp`, stateless, any replica serves it) with
the built-in `get_ui_catalog` (the newest catalog the thread recorded, `knownDigest` for "unchanged", an error to read
for a thread with no catalog) and the provider seam slices 8 and 10 add their tools through, and the binary's
`thread-tools` surface with `THREAD_TOOLS_*`, and the A2A adapter's grant (minted at send time from the non-secret
`ToolsGrant` on the send request, only for a card that lists the extension; the token is in the message and nowhere
else, which a test of every log line, of the log, of the export and of every row of the database checks). Two details
the build settled, in the contract: a token is refused unless
the thread exists and is the token's agent's (an `ask:<n>` token is refused until slice 10 builds the ledger it names),
and the key and the URL are required in every role when the surface is named, because the adapter of a worker is what
mints.
