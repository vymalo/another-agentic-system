# A2A extension: model pick (v1)

- **URI:** `https://agents.vymalo.com/a2a/extensions/model-pick/v1`
- **Status:** **proposed 2026-11-23; nothing is built.** Decided in [ADR 0086](../decisions/0086-a-person-picks-a-model-or-a-reasoning-level-for-a-thread-or-a-message.md).
- **Served by:** an agent whose card lists it (adam-rs does not yet). **Used by:** the orchestrator and the web.
- **Effort** is not this extension: it is adam's `reasoning/v1` (`https://agents.vymalo.com/a2a/extensions/reasoning/v1`, `params {efforts, default?}`, `metadata[URI] = {"effort": "<level>"}`), used as it stands.

## The card

```json
{"uri": "https://agents.vymalo.com/a2a/extensions/model-pick/v1",
 "required": false,
 "params": {"choices": [{"id": "fast", "label": "Fast"}, {"id": "deep", "label": "Deep"}], "default": "fast"}}
```

`id` is a neutral alias the agent defines (1 to 64 characters of `a-z0-9._-`). It is never a provider model id and carries no price.

## The message

The URI is activated in `A2A-Extensions` and `message.extensions`; `message.metadata[URI] = {"model": "<id>"}`. A message that picks nothing activates nothing and has no member.

## The client

`forwardedProps["vymalo.model"]` and `forwardedProps["vymalo.reasoningEffort"]` on an AG-UI run. `PATCH /api/threads/{id}` takes `model` and `reasoningEffort` as the thread's default (additive). **The message wins, then the thread, then the agent's own default.**

## Refusals

| Case | Answer |
|---|---|
| the live card lists no `model-pick/v1`, or the id is not in `choices` | `422 model_not_offered` with `offered: [ids]` (empty when not listed); nothing is written |
| the effort is not in the card's `reasoning/v1` `efforts`, or the card lists none | `422 reasoning_not_offered` with `offered: [levels]` |
| the agent answers `InvalidParams` | the message is undelivered, with the agent's reason; never retried without the pick |

## Stored

The `user_message` holds the resolved `model` and `reasoningEffort` (omitted when none); the thread row holds its default. Redelivery, a fork's first message and an edit carry them.

## Conformance (mock agent)

1. a pick arrives in the metadata, with the URI activated; 2. an unlisted pick is `422` and writes nothing; 3. no pick sends no member and activates nothing; 4. the message's pick beats the thread's; 5. a redelivery carries the recorded pick.
