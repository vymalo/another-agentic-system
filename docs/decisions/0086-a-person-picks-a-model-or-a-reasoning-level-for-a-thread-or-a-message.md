# ADR 0086 — A person picks a model or a reasoning level, for a thread or a message

- **Status:** **proposed 2026-11-23** (task ANR-14, issue #415, parity gap 15). **The orchestrator's side is built (slice 2,
  2026-11-23)**: the card read, the activation, the checks and the typed `422`s, the log, the thread default and its `PATCH`;
  the web's chips are slice 3 and the adam half is ANR-139. This record fixes the names and the rules the build follows. **Contract:** [`docs/api/model-pick-v1.md`](../api/model-pick-v1.md).
  **Pattern:** [ADR 0008](0008-platform-integration-via-a2a-extension.md); the closest working examples are the access tier
  ([ADR 0061](0061-the-person-picks-an-access-tier-for-each-message-the-agent-keeps-to-it.md)) and the locale hint
  ([ADR 0085](0085-the-interface-language-travels-with-the-message-as-a-hint.md)).
  **Not this:** [`model-relay/v1`](../api/model-relay-v1.md) (ADR 0063) is an endpoint per thread that the orchestrator serves; it
  stays planned and is a different thing. A pick here names a choice the *agent* offers and keeps its own model server.

## Context

The agent owns its model (invariant 1). Today a person cannot say "answer fast" or "think harder" for one message. adam-rs has the
two halves on its side, and this record uses its names rather than inventing others (*verified 2026-11-23*, adam-rs
`docs/decisions/0063-a-reasoning-effort-is-one-setting.md` and the body of adam-rs#338):

- **Reasoning effort:** the A2A extension `https://agents.vymalo.com/a2a/extensions/reasoning/v1`. The card lists it only for a model
  that takes a level, with `params {efforts: [...], default?}`; an activated message carries `metadata[URI] = {"effort": "<level>"}`;
  an unlisted level is `InvalidParams` (-32602) before anything is written; the agent echoes the effort. Levels are the closed
  vocabulary `none, minimal, low, medium, high, xhigh, max`.
- **Model:** adam's run takes a model through the protected context key `vymalo.model` (draft adam-rs#338), set today only by the
  embedding engine (`Say::with_model`); **a served agent exposes no A2A extension for it yet**. This record names the extension
  the orchestrator will speak so that adam can add it.

## Decision

1. **Two extensions, one per axis.** Effort uses adam's `reasoning/v1` unchanged. The model uses a new
   `https://agents.vymalo.com/a2a/extensions/model-pick/v1`, in the shape of `reasoning/v1`: the card's `params`
   `{choices: [{id, label?}], default?}`, a per-message `metadata[URI] = {"model": "<id>"}`, activated in `A2A-Extensions` and
   `message.extensions`. An `id` is a **neutral alias** the agent defines (for example `fast`, `deep`), never a provider model id,
   and the contract and the examples carry no price.
2. **The client key is `forwardedProps["vymalo.model"]` and `forwardedProps["vymalo.reasoningEffort"]`**, each a plain string,
   the names adam stores them under, on an AG-UI run. A message that names none activates nothing and carries no member.
3. **A thread has a default and a message overrides it.** `PATCH /api/threads/{id}` (additive) sets the thread's `model` and
   `reasoningEffort`; each message may name its own; **the message wins, then the thread's, then nothing** (the agent's default).
   The thread default is a convenience of the orchestrator: what reaches the agent is always one resolved pick per message.
4. **A pick the card does not list is refused, typed, before anything is written.** The orchestrator reads the live card
   (never cached, ADR 0008) and answers `422` with the code `model_not_offered` or `reasoning_not_offered` and the offered list;
   a card that lists no such extension also refuses a pick (`422`, the same codes with an empty list). This fails **closed**, unlike
   the locale hint: a person who chose "deep" must not silently get the default. A pick on a thread default is checked when it is
   set and again when a message uses it; a card that has since dropped the choice makes the thread's default a `422` for the message
   that resolves to it, naming the pick, and never a silent fallback.
5. **What is stored.** The log records the resolved pick on the `user_message` (`model`, `reasoningEffort`, omitted when none) and
   the thread's default in the thread row, **so a redelivery, a fork's first message and an edit carry what the message recorded**,
   as the tier and the locale do. The projection shows the pick on the message (a chip), nothing else. No model id, key or price
   is stored or shown: only the alias the agent offered.
6. **Fails closed on the wire as well.** An agent that lists the extension and answers `InvalidParams` leaves the message
   undelivered and says why; the orchestrator never retries it without the pick.
7. **The web shows the control only when the live card lists a choice**: a model chip when `model-pick/v1` is listed, an effort
   chip when `reasoning/v1` is, nothing otherwise.

## Consequences

- adam-rs needs a served-agent `model-pick/v1` (card, activation, `InvalidParams`, echo) over `vymalo.model`; until then no card
  lists it and the model control never shows. The effort control works as soon as an agent lists `reasoning/v1`.
- **The adam half is ANR-139:** adam serves `model-pick/v1` (card, activation, `InvalidParams`, echo) over `vymalo.model`; it
  waits on ANR-117 / adam-rs#338. Until it lands no card lists the extension, so every model pick is a `422 model_not_offered`
  with an empty `offered` and the web shows no model chip; the effort control works as soon as an agent lists `reasoning/v1`.
- `chat-api.yaml` changes are additive only (two optional members on the message and the thread; two error codes).
- A new value type in the core, `ModelPick` (a validated alias and a closed `ReasoningEffort` enum), carries the pick; every
  constructor of the send request gains a `None`. The log holds it as `user_message.pick` (one member, `{model?, reasoningEffort?}`);
  the thread's default is a `threads.pick` column (Postgres `0031`, SQLite `0003`), set by two `ThreadStore` methods.

## Alternatives rejected

- **One combined extension for model and effort:** the two have different owners in adam (a profile's levels vs the agent's
  choices) and different cards; a combined one would force the agent to list both.
- **Reusing `model-relay/v1`:** it moves the model call through the orchestrator; a pick needs none of that.
- **Message-only, no thread default:** forces the person to repeat the pick on every message.
- **Silent fallback to the default for an unlisted pick:** a person's choice would be ignored without a word.

## Proof (to build)

On mocks, each test red before its fix: the pick arrives at the agent; an unlisted pick is `422` and writes nothing; no pick sends
nothing; removing the forwarding once turns the main test red.
