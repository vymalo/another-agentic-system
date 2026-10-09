# Thread history pages (proposed)

- **Status:** **proposed (2026-10-09), not built.** The decision is [ADR 0059](../decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md);
  nothing here is in [`chat-api.yaml`](chat-api.yaml) yet, because that file is what the orchestrator's contract tests and the web's generated
  types follow, and they would fail on operations nobody serves. When slice 3 of the ADR builds the routes, this page's schemas move into the
  contract and this page keeps only the rules.
- **Defined by:** the orchestrator. **Used by:** the web (the browser, the desktop and mobile apps).
- **Not an A2A extension.** It is a second, finite read of the AG-UI projection that [`agui.md`](agui.md#connect-binding) describes, for a
  client that wants the end of a long thread first.

## Purpose

`connectThread` replays a thread from its first event. For a thread of a thousand turns that is a thousand turns in front of the person, and a
fold of the whole log on the server whatever the cursor. A history page is the same frames for **a few turns**, from the newest back, in one
JSON document, with the small facts about everything before it that the screen needs at once. The stream stays the way to follow a thread.

## Routes

| Operation | Route | Who | Projection |
|---|---|---|---|
| `getThreadHistory` | `GET /agui/threads/{threadId}/history` | the owner (`thread.read`) | the viewer's, as `connectThread` |
| `getSharedThreadHistory` | `GET /agui/shared/{token}/history` | a signed-in reader of an `internal` or `public` link | the reader's, as `connectSharedThread` |
| `getPublicSharedThreadHistory` | `GET /agui/public/shared/{token}/history` | anybody, outside the identity layer, for a `public` link | the public one, as `connectPublicSharedThread` |

Authorisation, the 404 that is one answer for every way a thread or a link does not work, and the rate limit of the public route are exactly those
of the connect routes ([`agui.md`](agui.md#connect-binding), [Reading a shared thread](agui.md#reading-a-shared-thread)).

## Request

| Query | Meaning |
|---|---|
| `before` | An integer `seq`, normally the `start` of the page the caller already holds: the answer is the turns that precede the turn that holds that event (a `seq` that is not a turn's first event is taken as the first event of its turn). Absent: the end of the log, the newest turns. A value beyond the log counts as the end; `0` or less is a 400. |
| `limit` | How many turns, `1` to `server.history.maxTurns` (100), default `ui.history.pageTurns` (20). |
| `since` | Instead of `limit`: go back at least to the turn that holds this `seq`, never beyond `maxTurns` or `maxPageBytes`. `limit` together with `since` is a 400. |

`Accept: application/json`. No cursor header: the position is the query.

## A turn

A turn runs from the run a person's message opens up to the run before the next person's message; what precedes the first message (a fork's
marker, a webhook) is part of the first turn. A `user_message` event always opens its own run, even when it arrives while an agent works (it closes
the open one first, [ADR 0036](../decisions/0036-sending-while-an-agent-works.md)), so **a page never starts inside a run**: its first frame is a
`RUN_STARTED`. The frames that close the run a steering message ended belong to the turn before it.

## Response

`200 application/json`, `Cache-Control: no-store` (and `X-Robots-Tag: noindex, nofollow` on the shared routes).

```yaml
HistoryPage:
  type: object
  required: [threadId, start, end, head, earlier, open, projection, frames]
  properties:
    threadId: { type: string, format: uuid }
    start:   { type: integer, description: "seq of the event whose frames open the page's first run; 1 when the page is the start of the log" }
    end:     { type: integer, description: "the last seq the page says something of. The newest page's `end` is the cursor: send it as Last-Event-ID to connect and nothing is missed or said twice. An older page's `end` is the `start` of the turn after it, of which the page holds only the frames that precede its first RUN_STARTED (the end of a run a steering message closed)" }
    head:    { type: integer, description: "the thread's last seq when this was read" }
    earlier: { type: boolean, description: "events exist before `start`" }
    open:    { type: boolean, description: "a run is open at `end`: the last frames are the beginning of it" }
    projection: { type: integer, description: "the version of the projection that wrote these frames (see Versions)" }
    frames:
      type: array
      items:
        type: object
        required: [event]
        properties:
          id:    { type: integer, description: "the frame's resume point, as the SSE id: of the same frame; absent where the stream has none" }
          event: { $ref: "#/components/schemas/AgUiEvent" }
    carry: { $ref: "#/components/schemas/HistoryCarry", description: "present when `earlier`" }

HistoryCarry:
  type: object
  description: What the log before `start` contributes to the readouts that cover the whole thread
  properties:
    turns: { type: integer, description: "the runs before `start` in which an agent produced something (labels, see ADR 0059)" }
    usage:
      type: object
      properties:
        tasks:  { type: array, description: "per A2A task: what is known of it at `start` (its latest totals plus the calls after them, else the sum of its calls), per provider and model" }
        groups: { type: array, description: "per {kind, name} (agent, sub-agent, asked agent): calls and counts, summed over every call before `start`" }
        latest: { type: object, description: "the newest call of the thread's own agent before `start`, if the page holds none" }
    files:
      type: array
      description: "the files the thread kept before `start`, newest last, at most 500 (the shape of a `vymalo.artifact` of kind file)"
```

`frames` are the frames the connect stream would send for those events, **byte for byte** (a `data:` of the stream is an `event` here), with the same
`id`s. Live text is never in a page: it is relayed, not stored ([ADR 0027](../decisions/0027-live-text-relayed-not-stored.md)); the connect that
follows says it.

## Rules

1. **Pages tile the stream.** Let *S* be the frames of `connectThread` from the first event. For any thread, any `limit` and any chain of
   `before`, the pages in order of `start` concatenate to *S* cut at the newest page's `end`, with nothing added, dropped or moved. This is what
   the tests assert on every golden of [`examples/`](examples/README.md).
2. **A page is a fold of the log up to `end`**, from the first event, by the same `Projector` as the stream. It is therefore as costly on the
   orchestrator as a connect (one read of the log and one fold), and as cheap on the wire as a few turns. A checkpoint of the projector would
   save the fold; the projector holds every message id it has seen, so a checkpoint is the size of the thread, which is why this design has none.
3. **Cut.** The server keeps the frames of the last `limit` turns while it folds (a ring of turn buffers), starts a turn at the first `RUN_STARTED`
   that a `user_message` event produces, and drops the oldest buffer when the ring is full. When the bytes of the frames exceed
   `server.history.maxPageBytes` it drops the oldest turns until they fit, **but never the newest turn**: a single turn larger than the cap is
   returned whole.
4. **`end` is a cursor.** `Last-Event-ID: <end>` on `connectThread` gives the rest. If a run is open at `end` the stream's preamble reopens it, as
   for any resume; a client that wants only settled runs cuts the page at its last `RUN_FINISHED` or `RUN_ERROR` group and resumes there, and the
   stream says the open run in full.
5. **Versions.** `projection` is an integer in `orch-agui-projection`. It rises whenever the frames written for an event that is already in a log
   could change (a new activity, another id rule); a golden test pins it to a digest of the goldens, so a change of frames without a new number
   fails. A client that stored frames of another version discards them ([ADR 0060](../decisions/0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md)).
   The frames' dependence on the thread's *current* title and description (they are said as the replay finds them, [Titles](agui.md#titles)) is
   not a version: a client takes both from the resource and from the frames after its cursor.
6. **Carry** is computed in the same pass: the pass records, at each turn start, how many usage events and kept files it has seen, and the carry
   of a page is those prefixes folded with the rules of `usage.rs`. It never grows with the thread except `files` (capped) and the per-task list
   (one entry per A2A task). Merging it with the page is defined so that `summarize(carry ⊕ page)` equals `summarize(whole thread)`; the tests
   assert it on the usage goldens.

## Errors

| Status | When |
|---|---|
| 400 | `before` or `since` is not an integer of 1 or more, `limit` is outside `1..=maxTurns`, or `limit` comes with `since` |
| 401 | no identity (owner and signed-in routes) |
| 403 | no role of the caller holds `thread.read`, or their roles grant nothing |
| 404 | the thread or the link does not work for the caller (one answer, one body) |
| 406 | `Accept` excludes `application/json` |
| 429 | public route: too many requests for the link or for all links (`Retry-After`); a 404 costs the shared bucket more |
| 503 | the store is unavailable (`Retry-After`) |

## Configuration (proposed keys)

| Key | Default | Meaning |
|---|---|---|
| `ui.history.initialTurns` | 12 | turns the web asks for when it opens a thread (served by `GET /api/config`) |
| `ui.history.pageTurns` | 20 | turns per page when the person scrolls up |
| `ui.history.projection` | the build's | the `projection` version this server writes, so a client can compare it with what it stored before it paints from a copy |
| `ui.clientCache` | `true` | whether a client may keep a copy of threads on its device ([ADR 0060](../decisions/0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md)); `false` makes a client wipe what it holds |
| `server.history.maxTurns` | 100 | the largest `limit` |
| `server.history.maxPageBytes` | 4 MiB | see rule 3 |

**The presence of `ui.history` in `GET /api/config` is the capability.** An orchestrator without it has no `history` route, and a client falls back
to the connect stream from the first event, as it does today. `ui` is served with its defaults filled in, so a build that has the route always
has the key.

## Tests the contract needs

- The tiling property of rule 1, over every golden of [`examples/`](examples/README.md) and over generated logs (steering messages, forks, an open run at
  the end, a thread with no person message, a thread of one turn), for every `limit` from 1 to the number of turns plus one.
- `surface-agui`'s contract test: each documented status is answered and no other, and each body validates against the schemas (the frames against the
  vendored AG-UI schema, as the stream's do).
- Authorisation as for connect: 404 for another person's thread whatever the roles, 403 without `thread.read`, one body for every dead link.
- `earlier`, `start` and `end` on the boundary cases: `before=1`, `before` beyond the log, `limit` larger than the thread, a byte cap smaller than a turn.
- The carry invariant of rule 6, and `projection` pinned to the goldens' digest.
