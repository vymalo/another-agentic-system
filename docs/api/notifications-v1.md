# Push notifications (v1)

- **Status:** contract proposed 2026-11-19 by [ADR 0077](../decisions/0077-push-notifications-for-a-persons-runs.md)
  (accepted, the owner's decision: in R1, self-hosted delivery). **Nothing is built.** The names, limits and timings
  below are the ADR's proposals; slice N3 adds the routes to [`chat-api.yaml`](chat-api.yaml) and this page follows
  what is built. Epic: [#384](https://github.com/vymalo/another-agentic-system/issues/384).

A push tells a person's device that a thread of theirs waits for them or ended. It never carries what the thread
says. The log is the truth; a push is a hint to open the app.

## Constants

| Name | Value (proposed) | What |
|---|---|---|
| Devices per person | 10 | the eleventh `POST /api/me/devices` is `409 too_many_devices` |
| Payload | at most 4096 bytes, encrypted | what a push service must accept (RFC 8291 §4, *verified 2026-11-19*) |
| Presence lease | 45 s, renewed every 20 s | how long an open connect stream keeps a thread quiet on its device |
| `needs_you` expiry | what waits expires, or 24 h | a push is never sent after it |
| `done` expiry | 6 h | |
| Retries | from 15 s, doubling, at most 10 min, at most 8 attempts | `Retry-After` wins when it is longer |

## Kinds

`kind` is a closed set. A client that meets a kind it does not know shows a generic text and opens the thread.

| `kind` | Class | When |
|---|---|---|
| `needs_you` | `needs_you` | a form or confirm, a credential, a hand-off approval or a gate waits for the person |
| `checkpoint` | `needs_you` | an agent-team checkpoint ([ADR 0074](../decisions/0074-agent-team.md)) |
| `over_limit` | `needs_you` | the run is parked over the person's limit |
| `done` | `done` | the job ended `done` |
| `pull_request` | `done` | the job ended and opened a pull request |
| `failed` | `done` | the job ended `failed` |
| `cancelled` | `done` | the job was cancelled from another device |
| `test` | none | `POST /api/me/devices/{id}/test` |

A newer push of the same thread and class replaces the older one on the device.

## What a push carries

| Channel | The device receives |
|---|---|
| Web Push, UnifiedPush (encrypted to the device, RFC 8291) | `{"v": 1, "id": "<notification id>", "kind": "needs_you", "thread": "<thread id>", "count": 2}` |
| APNs | `{"v": 1, "id": "<notification id>"}` with `mutable-content`; the app's notification service extension reads the rest from `GET /api/me/notifications/{id}` |
| ntfy | a fixed title for the `kind` in the person's language, the count, and a link to the thread; never a title of the thread or a word of it |

Never in a push: a message, a thread's title or description, a tool's input or output, a secret, an agent's name.

## The routes, in short

Every route needs a signed-in person and `thread.read`. A device, a setting or a notification of another person is
`404`, whoever asks. A deployment with no channel configured answers every route below `404 notifications_disabled`.

| Route | Body | Answer |
|---|---|---|
| `GET /api/me/devices` | | `{devices: [{id, channel, platform, label, createdAt, lastPushAt}]}`: never an endpoint, a key, a token or a topic |
| `POST /api/me/devices` | `{channel: "webpush" \| "unifiedpush" \| "ntfy" \| "apns", platform: "web" \| "android" \| "ios" \| "desktop", label, subscription?: {endpoint, keys: {p256dh, auth}}, token?}` | `201 {id}`; for `ntfy`, also `{topic, server}` once, for the ntfy app to subscribe to. The same endpoint or token again is the same device |
| `PATCH /api/me/devices/{id}` | `{label?, subscription?}` | `200`: a renamed device, or a rotated Web Push subscription (`pushsubscriptionchange`) |
| `DELETE /api/me/devices/{id}` | | `204`: consent withdrawn for that device |
| `POST /api/me/devices/{id}/test` | | `202`: one push of `kind: test` |
| `GET`, `PUT /api/me/notification-settings` | `{level: "all" \| "needs_you" \| "off", language}` | the person's level, `all` until they change it (a new device starts at `all`, the owner's decision), and the language of ntfy's fixed text |
| `GET`, `PUT /api/threads/{id}/notifications` | `{level: "all" \| "needs_you" \| "off" \| "inherit"}` | the thread's level |
| `GET /api/me/notifications/{id}` | | `{id, kind, thread, count, at}`; `404` after it expired |
| `GET /api/config` | | gains `notifications: {channels: [...], vapidPublicKey}` |

An endpoint must be `https` and pass the egress guard of
[ADR 0046](../decisions/0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md) (no private address),
else `422 invalid_endpoint`. Endpoints, keys, tokens and topics are sealed and never returned.

## Presence

A client that shows a thread opens `GET /agui/threads/{id}/connect` with the header `X-Vymalo-Device: <device id>`. While
the stream is open, that device gets no push for that thread; the person's other devices still do (each device is judged on its own, the
owner's decision). A device id that is not the caller's is ignored. The web
client closes its stream when the page has been hidden for 30 s, and catches up on return with the history read
([`history.md`](history.md)).

## The client recipe

1. Ask the OS or the browser for permission only after the person meets a first wait, never on first open.
2. Web: register a service worker, read `vapidPublicKey` from `GET /api/config`, call `PushManager.subscribe` with
   `userVisibleOnly: true`, then `POST /api/me/devices`. Android: register with the person's UnifiedPush distributor
   and post its endpoint and keys. iOS: post the APNs device token at each launch.
3. On a push, build the title from `kind` in the UI language, use `thread` and `kind` as the notification's tag, and
   open the thread on a tap. With the id alone, read `GET /api/me/notifications/{id}` first.
4. A push may arrive twice: key on `id`.
5. A setting changed or a device removed takes effect for the next push.

## What the server logs

The device id, the channel, the class and the outcome of each attempt (`delivered`, `retry`, `gone`, `failed`,
`expired`, `superseded`). Never an endpoint, a key, a token, a topic or a payload.
