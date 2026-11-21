# ADR 0083 — Trusted components: the agent names one, the client draws it, and a secret never enters the data model

- **Status:** **proposed (2026-11-21)**, and nothing is built. The owner asked for it on 2026-11-21, in his own words:
  *"using A2UI, we can show a component to a user, that he'll use to perform an action … the action can be api-key generation … the model shows him the api-key
  creation widget, 'Here, I can't see what you're doing, but just use this component to create the api-key and voilà!' The info never goes to the server.
  We can use this for everything sensible"*, and then *"It can even generate a secret and save into the vault directly right? The UI can show a first form for the
  generation, then later the form for collecting"*. The four rules below are the ones he accepted; the two extra components come from the second sentence.
  Private context: the `trusted` page of the simpler-UI build (another-context#71, `a626f294`; 21 boards) and the decision rows 106–108 (another-context#70).
  **Nothing of it is copied here** — the public page names no payment, checkout, pack, price or market, and the `credits` boards stay private.

## Context

A2UI lets an agent draw a form and read what the person types into it. That is right for a plan, a question, a choice — and wrong for a secret, because of one
path that already exists:

> anything a component's `action.event.context` binds is resolved by the client, **logged as `ui_action.context`** and forwarded to the agent.

*Verified 2026-11-21 by reading the pinned v1.0 schemas and the code:*

| Fact | Where |
|---|---|
| An action's `context` is "the key-value pairs from the component's `action.event.context`, **after resolving all data bindings**", and it is **required** on every action | `orchestrator/crates/e2e/tests/a2ui-v1_0/renderer_to_agent.json:38-42`, `:54` |
| A binding is `{"@path": …}` into the data model, and a value may be a literal, a binding or a function call | `…/common_types.json:104-114`, `:115-155`, `:302-348` |
| The orchestrator logs that context verbatim (`UiActionData.context`, "already resolved by the renderer") and relays the action to the agent | `orchestrator/crates/core/src/ui.rs:296-325`, `docs/api/agui.md:1962-1986`, `:1990-2001` |
| Nothing redacts it: `redact` is never called on this path, and `UiActionData::check` bounds sizes only (256 bytes per id, 16 KiB of context) | `orchestrator/crates/core/src/ui.rs:378-409`, `:29-31` |
| **A2UI v1.0 has no client-only state**: no `writeOnly`, no "renderer-only value", no field an agent cannot bind. The only role restriction is on **functions** (`allowedCallers: "rendererOnly"`) | `…/catalog_definition.json:146-172`; the whole pinned set greps clean for `client-only`, `writeOnly`, `renderer-only`, `sensitive` |
| The closest thing to a secret input, `TextField` `variant: "obscured"`, is ordinary data-model state that a bound context resolves and sends | `…/basic-catalog.json:419-446`; the web's `password` input, `web/src/features/chat/components/surface/surface-view.tsx:218` |
| Our catalog is **exactly three root members** (`catalogId`, `protocolVersion`, `components`), with no `metadata`, `extensions` or `functions`, and a component is a plain JSON Schema | `docs/api/ui-catalog-v2.md:100-104`, `:112-116` |
| The orchestrator does **not** validate components: "that is the renderer's job and it refuses what it does not know" | `docs/api/agui.md:1912-1913` |

Two conclusions follow, and they decide this ADR:

1. **A catalog rule cannot hold the secret.** There is no member to carry "this field is never bound", and the schema has no such concept to borrow.
2. **A catalog component cannot hold the trust.** The agent draws from our catalog, the orchestrator relays what it draws without checking it, and the web cannot tell our drawing from the agent's — which is exactly the imitation rule 2 forbids.

## Decision

1. **A trusted component is not a surface component.** It is a request, made through an A2A extension, that the **client** answers by drawing its own interface. The agent never draws it, so it cannot change its words, its chrome or its fields.
   *The extension is `trusted-components/v1`* (a URI of the same shape as the others), listed on the agent's card with the components it may ask for
   (`{"components": ["ApiKeyCreate", "McpCredential", "ModelServer", "SecretGenerate", "SecretCollect"]}`), and activated per message like every other
   extension (ADR 0008). The request is a message whose `metadata[<uri>]` is `{component, hints}`, and the hints are the only thing the agent controls.
   *Rejected: a component in the catalog marked trusted.* The mark has no home in `ui-catalog/v2` (three root members, no metadata), and a marked component is
   still drawable by the agent, so the client would have to distinguish our drawing from the agent's — the imitation problem rule 2 exists to prevent.
2. **The client draws the chrome, the mark and the sentence "the agent can't see this".** That frame is the client's, exactly as the surface's own frame already is
   (`web/src/features/chat/components/surface/surface-activity.tsx:210-218` draws the frame outside the component tree). An agent-drawn surface can never produce
   it, because an agent cannot draw a trusted component at all.
3. **The secret never enters the data model, so no action context can carry it.** A trusted component keeps its fields in the client's own state, and its action
   carries a **receipt**, never a value. This is the one protocol change, and it is small:
   - the receipt is `{status, id, name, scopes?, last4?, version?, where?}` and rides in the action's `context` — the member that already exists, is already logged
     as `ui_action.context`, and is already forwarded to the agent. No new A2UI member, no new event kind, no new route for the receipt.
   - **`last4` is never derived by the agent or the orchestrator**: the client sends the four characters it received from the API that stored the secret, and only
     for a kind of secret where a suffix is safe to show (an API key's last four). For a generated secret it is omitted.
   - The client refuses to bind any of its own trusted fields into an action context: the seam is `rewriteContext`
     (`web/src/features/chat/lib/a2ui/prepare.ts:178-194`), where a binding becomes a `$field` marker, and `resolveFields`
     (`web/src/features/chat/lib/a2ui/context.ts:11-35`), the last place a value leaves client state for the agent. A trusted component's values never enter either.
4. **The agent's hints are suggestions.** The client shows "suggested by <agent>", the person edits and confirms, and **the API caps every grant at what the person
   may grant** — the role decides, and the cap is greyed out with the reason when it is reached. A destructive action is confirmed by something the agent cannot
   press (typing the name, or holding the button), per the boards.
5. **A secret is a reference, not a value.** Where the vault makes or takes the secret (`SecretGenerate`, `SecretCollect`), the agent receives
   `{status, name, version, where}` and nothing else, and may then write a configuration that *names* the reference; the platform injects the value at call time.
   Rotation makes a new version and the agent sees only the number.
6. **A client without the extension fails closed**: the person gets **words and a link**, never a plain form. The agent's request is answered as an unknown
   extension is answered everywhere (ADR 0008): ignored, so the agent falls back to saying what to do, and the client offers the link to the screen that does it.

## The first components, and what each one can call today

*Verified 2026-11-21 against `98d6d61e`.*

| Component | What it does | Route or store | Built? |
|---|---|---|---|
| `ApiKeyCreate` | makes an API key for the orchestrator, shows it once (masked, revealed for 20 s, the clipboard cleared after 60 s) | **none** — the orchestrator issues no API keys; its API authenticates by OAuth JWT and DPoP (`docs/api/config.md:317-326`), and the only `api_key` in the product is the *outbound* MCP credential kind (`docs/api/chat-api.yaml:2756-2757`). Whether the platform issues them is outside this repository | **not built** |
| `McpCredential` | a person's own MCP server credential (ADR 0046) | `POST /api/user-tool-servers`, `PATCH /api/user-tool-servers/{serverId}` — `docs/api/chat-api.yaml:397-446`, `:447-469`; the secret is **write-only**, sealed in the broker, "in no response, error or log line" (`:2813`, `:426`); permission `tools.user.add` (`:402`, `:453`) | **built** (ADR 0046; `dev/user-tools-e2e.sh`) |
| `ModelServer` | a person's own OpenAI-compatible model server, per agent, chat or project | **none** — model endpoints are deployment config (`docs/api/config.md:103,187-188`); `model-relay/v1` is *per thread*, proposed and nothing built, and it lends the orchestrator's own key (`docs/api/model-relay-v1.md:4-6,17-19`) | **not built** |
| `SecretGenerate` | the vault makes the secret; it never exists on the device and never reaches the agent; an optional "show me once" for a person who must paste it elsewhere | the sealed `SecretStore` port (ADR 0046) holds a **tool server's** credential by server id; a **named secret with versions** is not what it stores today | **not built** |
| `SecretCollect` | the person pastes a third-party key; the client checks its format and, where it can, makes a test call; then it saves it | same store, same gap | **not built** |

So one of the five is built, and it is the one that proves the shape: ADR 0046's `secret` is already write-only and never echoed. The other four need a route or a
store, and this ADR does not pretend otherwise.

## Consequences

- **The log keeps the receipt and nothing else.** No secret is in the log because no secret is in an action context; the receipt is a `ui_action.context` the
  transcript already knows how to carry. A secret that a person *does* see (the show-once) is on the device, from the API's response, and never in a frame.
- **The agent's vocabulary grows by an extension, not by a catalog.** The catalog stays exactly three members, and a client that does not know the extension keeps
  working: it ignores the request, as it ignores every extension it does not know.
- **Two rules are the client's to keep, and they are testable**: a trusted field never becomes a `$field` marker (`rewriteContext`) and never resolves
  (`resolveFields`); and the frame is drawn outside the component tree, so no surface can reproduce it.
- **`last4` is new work everywhere.** No route returns a suffix or a fingerprint of a stored secret today (`user_tools.rs`, `orch-core`: no `last4`, no
  `fingerprint`), so the receipt's `last4` arrives with the route that can answer it — and it is omitted until then.
- **A reference makes rotation invisible to the agent**, which is the point: the agent's configuration names a reference, the platform injects the value, and a new
  version changes nothing the agent wrote.
- **Not decided here**: which store a *named* secret lives in (the sealed `SecretStore` is the obvious one, and it would grow from "one per server" to "one per
  name with versions"), and whether `ApiKeyCreate` is the orchestrator's or the platform's. Both are named in the table as gaps, not answered.

## Alternatives considered

- **A catalog component marked trusted.** Rejected above: no member to carry the mark, and the agent could still draw it.
- **A basic-catalog `TextField` with `variant: "obscured"`, and a rule that the client does not bind it.** Rejected: the schema has no such rule, so it would be a
  client convention that every client must reimplement, and one buggy client leaks the secret. The trust has to be in a component the agent cannot draw.
- **A new core event kind for the receipt.** Rejected for now: `ui_action.context` already carries a resolved map, is already logged and forwarded, and already
  has a size bound. A new kind would be a larger change for the same information — worth revisiting if the receipt ever needs to be an activity of its own.
- **Sending the secret to the orchestrator and letting it store it.** Rejected by the owner's own rule: "the info never goes to the server". The client calls the
  API with the person's credentials; the orchestrator never sees the value.

## What this ADR does not fix, and should be filed

**A measured gap between the catalog contract and the code.** `docs/api/ui-catalog-v2.md:114-116` and its refusal table (`:1517`) say a component schema with an
`@` property, a top-level `anyOf`/`oneOf` or `allowedParents`/`allowedChildren` is refused; `orch-core` refuses none of those (`orchestrator/crates/core/src/ui_catalog.rs:63`,
`:480-516`, `:526-543`; `orchestrator/crates/app/src/catalog.rs:63-85`). It is unrelated to trusted components — and it is exactly the kind of place where a rule
"the contract says so" would later be trusted without being enforced. It gets its own issue rather than a silent fix here.
