# A2A extension: model pick (v1)

- **URI:** `https://agents.vymalo.com/a2a/extensions/model-pick/v1`
- **Status:** **proposed 2026-11-23; the orchestrator's side is built** (slice 2 of ANR-14: the card read, the activation, the checks and the typed `422`s, the log, the thread default and its `PATCH`), **the web's chips are built** (slice 3b, 2026-11-24: `web/src/features/pick`, drawn from `AgentInfo.pick` while the live card lists each axis) and **a `dev/` mock lists both** (`dev/wiremock/pick`, `dev/pick-e2e.sh`); **no real agent lists it yet** (adam's half is ANR-139). Decided in [ADR 0086](../decisions/0086-a-person-picks-a-model-or-a-reasoning-level-for-a-thread-or-a-message.md).
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
| the card cannot be read to check it | `503` + `Retry-After`; nothing is written |
| the agent answers `InvalidParams`, or its card no longer offers the pick at the send | the message is undelivered (`AgentError::Unsupported`, never retried), with the agent's reason; never sent without the pick |

## Stored

The `user_message` holds the resolved pick as `pick: {model?, reasoningEffort?}` (omitted when none, an axis omitted when not named); the thread row holds its default (`threads.pick`, one column in each store, no event). Redelivery, a fork's first message (`model`, `reasoningEffort` on the fork body) and an edit that names none carry it; the projection shows nothing of it (the chips are the composer's, `web/src/features/pick`). A thread's default is **not** copied into a fork or an export.

`PATCH /api/threads/{id}` with `model` and/or `reasoningEffort` answers the thread with those two members set to the default as it is now; `null` clears an axis, an absent member leaves it.

## Conformance (mock agent)

1. a pick arrives in the metadata, with the URI activated; 2. an unlisted pick is `422` and writes nothing; 3. no pick sends no member and activates nothing; 4. the message's pick beats the thread's; 5. a redelivery carries the recorded pick.
