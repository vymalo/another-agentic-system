# ADR 0023 — A UI component catalog, sent at conversation start, with a refetch seam

- **Status:** accepted (2026-10-01), on the owner's delegation: the extension's URI and open question 36 are
  decided in the [status note](#status-note-2026-10-01-accepted-on-the-owners-delegation). The owner may revisit it.
  Amended (2026-11-14): catalog version 7 adds five kinds of question to `Choices`, see the
  [status note](#status-note-2026-11-14-version-7-adds-five-kinds-of-question-to-choices).
  **Amended (2026-11-19, [ADR 0075](0075-a2ui-v1-0-cutover.md)):** with the A2UI v1.0 cutover the extension becomes
  `ui-catalog/v2`, a new URI and page (`docs/api/ui-catalog-v2.md`), because A2UI v1.0 requires `"protocolVersion": "1.0"`
  on the catalog, which breaks every v1 reader; the `catalogId` stays and `version` continues at 9 (the owner's decision,
  2026-11-19). Decisions 1 to 7 stand
  under v2. Nothing of it is built yet.

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

## Status note, 2026-10-01: Cards and Mermaid are built in the web (MVP slice 4, web side)

The web's catalog is at version 3: Cards and Mermaid join Text, Column and Choices
([`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md#version-3-slice-4-version-2-plus-cards-and-mermaid)). Both are output only.
A card never loads anything (ADR 0013 rule 5); a graph is drawn by mermaid in the browser, loaded only when one is drawn,
at its strict security level and without HTML labels, and shown as an image, so nothing in it can run or load. A surface that
breaks either component's schema is refused visibly, as for any component. **Not built yet:** the agent's side (adam-rs
`show` with Cards and Mermaid, slice 4's researcher) and the stack's pin of it.

## Status note, 2026-10-02: Image is built in the web (UI catalog version 4, [ADR 0032](0032-files-from-agents-live-in-an-artifact-store.md))

The web's catalog is at version 4: `Image` joins Text, Column, Choices, Cards and Mermaid
([`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md#version-4-adr-0032-version-3-plus-image)). It names a file of the thread by its
SHA-256, never a URL, and the validator refuses a hash the thread does not hold. The description of `Choices` now begins "ask_user
only" (a form shown with `show` has nothing waiting for its answer). Both are part of version 4's digest.

## Status note, 2026-10-11: version 6 is built in the web (UI catalog version 6)

The web's catalog is at version 6: 19 components join Text, Column, Choices, Cards, Mermaid and Image, in four groups
(code and files, structure and status, data, media; [`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md#version-6-code-and-files)).
An agent uses these components only when the thread's catalog is version 6 or higher.
Every reference to a file is the file's SHA-256 of the thread, and a video or a sound plays under the media policy of
[ADR 0054](0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md#amendment-2026-10-11-media-plays-from-this-origin-and-blob-and-from-nowhere-else).
Version 6 is not released (it is not on main); its digest is `RELEASED[6]` in `catalog.test.ts`, and version 5 is Choices' (PR #220).

## Status note, 2026-11-14: version 7 adds five kinds of question to `Choices`

The owner asked for more kinds of question, and for the contract to be the product: adam-rs builds its `ask_user` kinds on it next
(its ADR 0044, slice 11) and a Flutter client will be written from it, so the web is the first implementation and nothing the web
does rests on knowledge that is not in [`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md#version-7-more-kinds-of-question).

**Decided** (version 7, digest `RELEASED[7]` in `catalog.test.ts`; additive, a Choices of versions 2 to 6 is valid and reads and
answers as it did):

1. **Five more values of a question's `type`:** `confirm` (a yes or no, with `yesLabel`, `noLabel` and the safe answer as
   `default`), `number` (`minValue`, `maxValue`, `step`, `unit`), `scale` (a whole number from `from`, 0 or 1, to `to`, with the words
   `fromLabel` and `toLabel` at the two ends), `date` (a calendar day, `minDate`, `maxDate`) and `rank` (the options put in order).
   Not a new component: a form of several kinds stays one `Choices` with one submit, so that the version 5 machinery (`required`,
   `default`, the answer bubble, the 16 KiB of context) carries them.
2. **The answer keeps its shape.** `{id, values}` with `values` an array of **strings** for every kind, and no `other` and no `text`:
   `["no"]`, `["250.5"]`, `["7"]`, `["2026-12-15"]`, `["c", "a", "b"]` (a `rank`'s order is the person's, the one place where it is
   not the options'). A number is **a decimal string with a stated grammar, never a JSON number**: an A2A server holds the numbers of
   a message's metadata as doubles ([`ui-catalog-v1.md` section 5](../api/ui-catalog-v1.md#5-to-the-agent-the-extension)), and a string comes
   out as it went in; it is compared as an exact decimal (millionths in an integer), never through floating point. A date is a day
   and not an instant: no time, no zone. The same shape means a version 6 receiver still reads the answer, and the "no answer" of
   version 5, an empty `values` for a question that is not required, is the only way a question is left unanswered.
3. **The orchestrator does not read an answer; the agent validates.** The orchestrator checks the envelope, the size and that the
   thread has the surface ([`agui.md`](../api/agui.md#actions)) and relays the rest, as it did. The receiver is the agent that asked; the
   contract gives it ten rules (eight about a question's values: `required`, `too_many`, `not_offered`, `grammar`, `below_min`,
   `above_max`, `off_step`, `not_a_permutation`; two about the shape of the whole answer: `unknown`, `unexpected`) and the cases that exercise them as **conformance vectors in the
   page itself**, which the web's tests, the mock's receiver and a test of the page all read. An answer that fails is not acted on and
   never repaired (no clamping, no rounding, no sorting): the agent names the question and the rule in words and asks again with a new
   surface whose questions carry the valid answers as their defaults. The client validates first with the same check and holds the
   button with the reason in words, so a refusal means a stale form, another client or a hand-made action.
4. **Versions and degradation.** An agent learns what the screen can ask from the thread's catalog, in the two ways it already has: the
   `version` of the `ui-catalog/v1` metadata (7 or more) and the `type` enum of the `Choices` schema it was given (the better test, which
   also covers a later version). An agent that knows only version 6 never uses a kind it was not told, so nothing changes for it
   or for a client of version 6. When an agent wants a kind a screen lacks it asks in the nearest version 6 form (`single` with two
   options, `text` with the grammar in its help, ...) and validates the text itself. A client that meets a kind it does not know
   refuses the **whole surface** visibly (rule `schema` in a client of version 6, whose reason names the place of the `type`, `questions.1.type`)
   and never guesses a control; the person answers in words by a message. The one rule on the agent: **the `question` of every question
   is a sentence a person can answer in a message, and a message sent while the ask is open is the answer.**
5. **Accessibility is part of the contract**: a yes or no and a scale are `radiogroup`s of native radio buttons; a number is a text
   box with `inputmode="decimal"` that takes only the grammar (a browser number box reads locales and exponents); a day is the
   browser's date control with `min` and `max`; a rank is an ordered list with **Move up** and **Move down** buttons named with the item,
   operable by keyboard with no drag, that says the new place in words and keeps the keyboard's place; an untouched required rank is
   not an answer (the agent's order is not the person's ranking). Every error is words linked to the control.

**Also changed:** a form that was sent stays **answered** when the thread waits again. Until now only the newest copy of a surface
was live while the thread was `blocked`, so a form an agent asked again after refusing an answer would have left two open forms.

**Built:** the web (`choices-kinds.tsx`, `lib/a2ui/choices-kinds.ts`), the catalog (version 7), its lock, the mock script
`choices-v7` (whose scripted agent is a receiver, `mock/receiver.ts`, written from the page alone and run on the same vectors) and
the tests (`e2e/choices-v7.spec.ts`, axe in both colour schemes, the contract cases of the mock). **Nothing in the orchestrator
changes:** it treats the catalog as a document and a Choices answer as an action, and the forms catalog sent to an asked agent
([`ui-catalog-v1.md`](../api/ui-catalog-v1.md#the-forms-catalog-of-an-ask)) already carries the thread's `Choices` schema unchanged.
**Not built, and not proven:** an adam-rs agent that asks with the kinds and validates the answer (its slice 11); a second client; a
live model, a real browser other than Chromium, a screen reader.

**Amended after review (2026-11-14):** two reviews, one of them a validator written in Python from the page alone (it agreed with every
vector), found the page still left a stranger guessing; the choices are now written in it. `required` is `true` when absent; the rules
are tried in the table's order; a value that is not a string is `grammar` (`not_offered` for `confirm` and `rank`); a `note` without a
`freeInput` and `other` or `text` on a kind, even empty, are `unexpected`; the receiver never trims and the client does; a `default` is
a string, or a list for `rank`, and the other shape is a refused question; **a rank's `default` is the order it starts in and never an
answer, on a re-ask too** (the person acts once); an optional confirm, scale or rank has a **Clear answer** control; a re-ask is a
new surface; a form is answered once the person acts after it, by an action or a plain message (this is true of every version); an
unknown `type` is a `schema` refusal, never the "needs a newer version" placeholder; display follows the locale and the wire form
never does; the whole catalog is one file and a client need not compute its digest. The vectors grew from 64 to 133 cases, with
blocks for whole answers, invalid questions and surfaces, and a test fails when one is deleted.

## Status note, 2026-11-14 (second): version 8 lets a question show what it asks about, and makes `Checklist` fit for a live plan

The owner, asked which kinds of question an agent should be able to ask: "Images, graphs,... A2UI is enabled by default so we might as
well use it". Slices 9 and 10 of [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md). Version 7 gave the questions
their kinds; version 8 lets a question **show** components of its surface, and lets an agent's **plan** be one checklist that updates where
it stands. As for version 7, the page [`api/ui-catalog-v1.md`](../api/ui-catalog-v1.md#version-8-a-question-that-shows-what-it-asks-about-and-a-plan)
is the product (a Flutter client and the agent side in adam-rs are written from it alone) and the web is its first implementation.

**Decided** (version 8, digest `RELEASED[8]` in `catalog.test.ts`; additive, and **taking the additions away gives version 7's digest,
digest for digest**, which a test of the web proves):

1. **`show`, a member of any question of `Choices`: 1 to 4 ids of components of the same surface** (8 in a form), drawn with the question
   after its words and before its control. **A closed list of seven components may be shown:** `Text`, `Image`, `Mermaid`, `Code`,
   `Cards`, `Callout`, `Chart`. Each draws something to look at, none asks anything, and none holds other components, so a shown
   component can never contain the question that shows it: no cycle rule is needed, and a `Choices` shown by a question is refused for
   its type. A shown component is drawn where its question is and **nowhere else**: also placing it in the surface is refused (rule
   `expansion`); an id shown twice, a component not on the list, more than 4 or 8 and an `Image` with no `alt` are `schema` (never drawn
   as decoration); a shown image of a file the thread does not hold is `artifact`; an id the surface does not hold yet leaves it
   *pending*, as a child does. The picture of an option of an `images` question is a different thing and stays: it is what is chosen,
   `show` is what the question is about.
2. **A client of version 7 refuses a surface that has `show`** (rule `schema`, at `questions.N.show`) and never ignores the member, for a
   form that hides what it asks about is not the form the agent asked. So an agent never sends `show` to one: it reads the thread's
   version or whether `show` is in the schema it was given, as for the kinds of version 7.
3. **Accessibility:** what is shown is **inside the question's group**, after its words and `help` and before its control, so a screen
   reader meets the question, then what it asks about, then the control; a form that no longer takes input still lets the person read, copy
   and open what it shows (the group is not the disabled `fieldset`: its controls are in an inner one).
4. **`Checklist` gains the state `dropped` and an item `note`.** A dropped item is struck through and said in words ("Dropped"), is not
   counted in "N of M done" (neither in M) and, when every item is dropped, the count reads "0 of 0 done" and the bar is not drawn.
5. **A surface sent again.** Written once, in one place of the page: the same `surfaceId` is the same surface (as A2UI says and the
   orchestrator already does); a `createSurface` for a surface that exists starts it over (*verified 2026-11-14* in the reducer the web
   uses), so **a client reads, and counts against the size limit, only the operations from the last `createSurface`**; a copy in a later
   run of a surface that is not a plan keeps the rule it always had (the newest is live, the older says "updated further down"); a
   **plan** is matched by its `surfaceId` and **never rewrites what the person saw**: the thread is cut into stretches by the messages of a
   person, copies of a plan within one stretch are one drawing (where the first stands, from the newest), a copy after a message of a person is a
   new drawing, and the earlier one stays as it was, under "This plan was updated further down." (amended 2026-11-14 after review: the first
   build matched copies by activity key, which a new job changes, and drew the plan twice or rewrote the words a person had answered).
6. **The plan is a surface whose `surfaceId` starts with `plan-`** (and has at least one more character). The hook is the id, not a member
   of `createSurface`, because the id is the one thing every layer already carries and keys on and no layer has to learn anything; a
   client never guesses a plan from a component's type. A plan holds nothing the person can answer (a `Choices`, an input or a button
   is refused, rule `action`), so it never meets the rules of [Answered forms](../api/ui-catalog-v1.md#answered-forms), which stand as they were
   (a re-ask is a new surface id). It is **found** in the conversation where it first stood, and **pinned** as a **Plan** tab of the
   thread's panel (the existing pattern: the panel already holds Activity and Sources), present only while the thread has a plan, with its
   progress in its name; the latest plan sent is the pinned one. It is **announced politely** ("Plan updated: 2 of 5 done", a status
   region) after it has been quiet for 0.8 s, so a burst is one announcement; never for the first copy, never for the words the agent streams.

**Built:** the catalog and its lock (version 8), the validator (`prepare.ts`: `show`, the plan rule, the dead operations), the components
(`choices-shown.tsx`, `checklist.tsx`, `plan-announcer.tsx`, the panel's `plan-tab.tsx`), the mock scripts `choices-v8`, `choices-v8-bad`
and `plan`, and the tests: the vectors of the page (61 cases in four blocks, each run through the web's validator, through a second implementation
written from the page alone, `mock/surfaces-v8.ts`, and a test that fails when one is deleted), the DOM tests, `e2e/choices-v8.spec.ts`, `e2e/plan.spec.ts`, axe in both colour schemes.

**Found, and not changed here (the orchestrator is not part of this slice):** the projector keeps every operation of a surface, up to
256 KiB ([`agui.md`](../api/agui.md#surfaces-in-the-projection)), and does not drop the ones a later `createSurface` made dead, so a plan sent
very many times reaches the cap (about sixty times for a plan of four kilobytes) and its later updates are not shown. The client's rule 5
keeps its own size limit out of it; compacting the surface at its `createSurface` in the projector is a change to open (*closed 2026-11-14, third note below*). **Not built, and not
proven:** an adam-rs agent that sends a plan or asks with `show` (the agent side); a second client; a live model; a screen reader.

## Status note, 2026-11-14 (third): the projection keeps only what is alive

The orchestrator's projector now drops the operations before a `createSurface` for a surface it holds, as rule 2 of the contract says a client does, and a `deleteSurface` ends a surface even when it had passed the replay cap. The 256 KiB cap is on the **alive** operations of a surface, so a plan sent whole 200 times stays shown and is replayed as one copy; one copy over the cap still stops with the same error line, and the next `createSurface` whose copy fits shows it again. What the live stream sends does not change in kind (a snapshot after a re-create holds fewer operations). This closes the finding of the second note. Tests: `orchestrator/crates/agui-projection/tests/a2ui.rs`.
