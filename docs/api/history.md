# Thread history pages

- **Status:** **built** (2026-10-09, [ADR 0059](../decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md) option C, after the full replay seeded off screen missed its gate): the three operations are in [`chat-api.yaml`](chat-api.yaml)
  (`getThreadHistory`, `getSharedThreadHistory`, `getPublicSharedThreadHistory`) with the schemas the orchestrator's contract tests and the web's generated types follow, and this page keeps the **rules**.
  The `carry` of the readouts that cover the whole thread (token totals, kept files, the number of the turns before the page) is served too (rule 9).
- **Defined by:** the orchestrator (`orchestrator/crates/agui-projection/src/history.rs` is the fold, `orchestrator/crates/surface-agui/src/history.rs` the routes). **Used by:** the web (the browser, the desktop and mobile apps).
- **Not an A2A extension.** It is a second, finite read of the AG-UI projection that [`agui.md`](agui.md#connect-binding) describes, for a client that
  wants the end of a long thread first, or the settled runs after a point it holds.

## Purpose

`connectThread` replays a thread from its first event. A history page is the same frames for **a few chains of settled runs**, in one JSON document,
with the small facts about everything before it that the screen needs at once. The stream stays the way to follow a thread.

## Routes

| Operation | Route | Who | Projection |
|---|---|---|---|
| `getThreadHistory` | `GET /agui/threads/{threadId}/history` | the owner (`thread.read`) | the viewer's, as `connectThread` |
| `getSharedThreadHistory` | `GET /agui/shared/{token}/history` | a signed-in reader of an `internal` or `public` link | the reader's, as `connectSharedThread` |
| `getPublicSharedThreadHistory` | `GET /agui/public/shared/{token}/history` | anybody, outside the identity layer, for a `public` link | the public one, as `connectPublicSharedThread` |

Authorisation and the 404 that is one answer for every way a thread or a link does not work are those of the connect routes
([`agui.md`](agui.md#connect-binding), [Reading a shared thread](agui.md#reading-a-shared-thread)).

## Chains, turns and what a page holds

- A **chain** is the events from one that **opens a run while none is open** to the first event after which **no run is open** (the thread is
  *settled*). A person's message that arrives while an agent works closes the open run and opens its own in the same event
  (`Projector`, `RunClose::Superseded`, [ADR 0036](../decisions/0036-sending-while-an-agent-works.md)), so a chain of steered runs is one chain: no
  settled point lies between them, and a page never cuts inside it. Events that have no frame (`ui_catalog`, `thread_shared`) never open a run and
  belong to the chain before them.
- A **turn**, for `limit`, is a chain that starts with a person's message (the first chain counts as one whatever starts it). Chains between two turns
  (a title, a tool attached, a CI report that opened a run of its own) ride along with the turn before them and cost nothing of `limit`.
- A **page** is a whole number of **settled** chains, covering the events `[start, end]` with nothing missing: the events of a chain run to the event
  before the next chain's first, so the pages of a thread partition its log. **The chain that is still open is never in a page**; the stream says it,
  in full, from `end`. A page's first frame is a `RUN_STARTED`, and its last is the end of a run.

## Request

| Query | Meaning |
|---|---|
| `before` | An integer `seq`, normally the `start` of the page the caller holds: the answer is the turns that precede the chain that holds that event. Absent: the newest turns. A value beyond the log counts as absent; `0` or less is a 400. |
| `limit` | How many turns, `1` to `server.history.maxTurns` (100), default `ui.history.pageTurns` (20). The web grows it with each older page it asks for (20, 40, 80, up to `maxTurns`). |
| `since` | Instead of `limit`: go back at least to the chain that holds this `seq`, never beyond `maxTurns` or `maxPageBytes`. |
| `after` | A catch-up read: the settled chains from the one after the chain that holds this `seq`, to the newest settled point (at most `maxTurns` turns, `maxPageBytes`, counted from the first chain after it). With `after` the response carries `anchor` and no `carry`. `start` is `after + 1` when `after` was a settled point (a previous `end`), and larger when it was inside a chain: the gap is how a client sees that its copy is not what the log says. A value beyond the log counts as its end; nothing newer is an empty page. |

`limit`, `since` and `after` are exclusive (400 together); `before` goes with `limit` and `since` only. `Accept: application/json`. No cursor header: the
position is the query.

## Response

`200 application/json`, `Cache-Control: no-store` (and `X-Robots-Tag: noindex, nofollow` on the shared routes). The schema is `HistoryPage` in
[`chat-api.yaml`](chat-api.yaml):

| Member | Meaning |
|---|---|
| `threadId` | the thread (not a capability on the shared routes) |
| `start` | the first event the page accounts for: a chain's first event (`end + 1` for an empty page) |
| `end` | the last event it accounts for, and a **settled point**: no run is open after it. Send it as `Last-Event-ID` to `connectThread` and nothing is missed or said twice |
| `head` | the thread's last `seq` when this was read; more than `end` while a chain is open or a message has just arrived |
| `earlier` | events exist before `start` |
| `projection` | the version of the projection that wrote these frames (see Versions) |
| `frames` | `[{id?, event}]`: the frames, with the same `id`s a connect stream writes (absent where the stream has none) |
| `anchor` | a catch-up only: `{seq, runId}` of the last run that had ended by event `after` (absent when none had) |
| `carry` | what the log before `start` contributes to the readouts that cover the whole thread: `turns`, `usage`, `files` (rule 9); present when `earlier` and the read is not a catch-up |

`frames` are the frames the projector writes for those events, with the same `id`s a connect stream writes. They are **not** the frames of every connect
stream (rule 2). Live text is never in a page: it is relayed, not stored ([ADR 0027](../decisions/0027-live-text-relayed-not-stored.md)).

## Rules

1. **Pages tile a replay.** Let *S* be the frames of a connect stream that holds no live text (a replay: its overlay is empty) over a **fixed
   `ThreadMeta`**, from the first event. For any thread, `limit` and chain of `before`, the pages in order of `start` concatenate to *S* cut at the
   newest page's `end`, nothing added, dropped or moved. This is the test, on every golden of [`examples/`](examples/README.md) and on generated logs.
2. **Why not "every stream", and why a fixed meta.** Two things make a live stream differ from a page. (a) `LiveOverlay::logged`
   (`orchestrator/crates/agui-projection/src/live.rs`) rewrites the frames of the log while a reply is being written: the final message of a live
   message loses its `TEXT_MESSAGE_START`, its `CONTENT` carries only the words not yet said, and a given-up message is renamed `<id>~final`. Those
   frames are right for a screen that holds the draft and wrong for anything stored. (b) The projector is built from the thread's *current* title,
   description, target and gate (`meta_of` in `surface-agui/src/run.rs`), and the snapshots before the first title or description event say those;
   a description is rewritten after every job. So **a client stores only what a page returned, never what a stream wrote after it caught up**, and
   takes the title, the description and the share from the resource and from the frames after its cursor, never from stored frames.
3. **A page is a fold of the log up to `end`**, from the first event, by the same `Projector`. It costs a connect's read and fold on the orchestrator
   *every time*: a page of older turns folds the whole prefix again (up to the chain that holds `before`, and not beyond), so reading back through a
   thread of *L* events in pages of *p* events costs *L²/p* (ADR 0059, decision 4). A checkpoint of the projector would save the fold, and the
   projector holds every message id, text record and run id it has seen, which is why this design has none. **The cost-lowering option built is the
   web's: pages that grow** (20, 40, 80, up to `maxTurns`), which makes the sum *L log L*. Measured on a thread of 12 000 events
   (`tests/history_cost.rs`, release, a shared 4-core machine): the newest 12 turns fold in 61 ms; reading the thread back to its start costs 306 000
   events folded in 1.33 s in pages of 20, and 85 000 events in 0.49 s in growing pages.
4. **Cut.** The server folds, closes a chain buffer at each settled point, and keeps the last `limit` turns' worth of buffers (a ring, plus one turn
   for a chain that is still open, and at most 512 chains for a thread with no person's message). When the bytes of the frames exceed
   `server.history.maxPageBytes` it drops the oldest **chains** until they fit (a catch-up, which must start where it was asked to, drops the
   newest), which includes the chains between turns, so a thread with no person's message (a webhook, a CI-driven thread) is bounded too. **The newest
   chain is never dropped**: a single chain larger than the cap is returned whole. A page is cut at chain starts only.
5. **Self-contained.** A page may say again an activity that an older page first said, only as a snapshot that **replaces** it (`replace: true`): an
   A2UI surface keeps the id `a2ui-<seq of its first event>` and is snapshotted whole by every later event that touches it until a new job clears it
   (`Projector`, `surfaces`, `forget_job`); the `vymalo.step`, `vymalo.check` and `vymalo.ask` cards do the same inside a job. No other id may repeat
   across pages. The test lists, for every golden and generated log, each id a page says that an older page said first, and fails on any that is not a
   replacing snapshot. A client that merges pages must keep the **newest** copy of such an id.
6. **`end` is a settled cursor.** `Last-Event-ID: <end>` on `connectThread` gives the rest, with no run to reopen. A client that wants a run in
   progress asks the stream, which says it in full from its `RUN_STARTED`.
7. **Versions.** `projection` is an integer in `orch-agui-projection` (`PROJECTION_VERSION`). It rises whenever the frames written for an event already in a log could change
   (a new activity, another id rule). A table in the repository (`orchestrator/crates/agui-projection/tests/projection-digests.txt`) pins, **per golden, over a fixed `ThreadMeta`**, the SHA-256 of its frames, and the table's
   first line is the version; a test fails when an existing line differs from the frames, a new golden adds a line and changes nothing else, and a CI
   check (`tools/projection-digests-check.sh`, in `orchestrator.yml`) fails a change to an existing line that does not also raise the version. A client that stored frames of another version discards them
   ([ADR 0060](../decisions/0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md)).
8. **The anchor** of a catch-up is what makes a stored copy checkable: the log of a thread restored from a backup and then written to again can reach
   a `lastSeq` above a copy's last event while telling a different story. The copy keeps the `runId` of its last settled run, and a catch-up whose
   `anchor.runId` is not that one drops the copy.
9. **Carry** is the initial state of the web's own folds, not a second rule. It is read off the **frames** the projector writes (the web folds those same frames),
   never off the events: the pass records, at each chain start, how many usage frames and kept files it has seen and how many runs have drawn something of the agent's,
   and the carry of a page is those prefixes folded with the rules of `usage.ts`. Its members:

   | Member | What | How the web uses it |
   |---|---|---|
   | `turns` | the runs before the page that drew something of the agent's (words, reasoning, a step, an artifact, a check, a CI report, a rework, an action, an ask, a surface, an error, a status that is not `completed` or `input_required`; not the job marker, a fork's divider or a tools card): the web's `isAgentTurn` | added to the place of a turn among the ones held, so "Turn 49" stays 49 when older pages load |
   | `usage.tasks` | per A2A task: its latest totals plus the calls reported after them (all its calls while it has none), per provider and model | each is held as a total that covers no call of the pages held (`after: 0`), which a real `vymalo.usage_total` of the task replaces |
   | `usage.groups` | the calls of the agent, of each sub-agent and of each asked agent, apart, in the order they first spent: `kind`, `name`, `calls` and the counts | the base of the groups the ring's details list |
   | `usage.latest` | the latest call of the thread's agent, as a `vymalo.usage` value without its path | what the ring fills with when no call of the pages held is the agent's |
   | `files` | the kept files before the page, each hash once, in the order handed over, the newest 500: the `content` of their `vymalo.artifact` activities | unioned with the files of the turns held: an `Image` of a surface may name only a file the thread holds |

   The web rebuilds its state from the carry of the **oldest page it holds** and the frames of the pages it holds, oldest first, then the stream; an older page
   puts its frames in front and replaces the carry. The invariant, tested on every golden and on generated logs (`orchestrator/crates/agui-projection/tests/carry.rs`,
   against a second implementation of `usage.ts` written for the test), and again with the web's own folds on the pages the orchestrator wrote
   (`examples/history/usage-turns.walk.json`, `file-turns.walk.json`): `summarize` of that state equals `summarize` of a fold of the whole thread, the files held and carried
   are the thread's, and `turns` plus the turns held is the thread's. The number of a turn is the one thing that is a *rule* and not a fold: it counts what the web draws
   (`drawsPart`), checked against the web on the goldens (`history-turns.dom.test.tsx`).

## Errors

| Status | When |
|---|---|
| 400 | `before`, `since` or `after` is not an integer of 1 or more, `limit` is outside `1..=maxTurns`, or parameters that exclude each other are given |
| 401 | no identity (owner and signed-in routes) |
| 403 | no role of the caller holds `thread.read`, or their roles grant nothing |
| 404 | the thread or the link does not work for the caller (one answer, one body) |
| 406 | `Accept` excludes `application/json` |
| 429 | public route: too many requests for the link or for all links (`Retry-After`), or the link's stream permits are taken (`code: too_many_streams`) |
| 503 | the store is unavailable (`Retry-After`) |

**The public route holds one of the link's stream permits for as long as it folds**, as an open connect does (5 per link, 50 in all by default): a
request there costs one limiter token and a full fold, and the permit is what bounds how many folds run at once. A 404 costs the shared bucket
more, as for every public route.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `ui.history.initialTurns` | 12 | turns the web asks for when it opens a thread (served by `GET /api/config`) |
| `ui.history.pageTurns` | 20 | turns of the first older page; later ones grow (20, 40, 80, up to `maxTurns`) |
| `ui.history.windowed` | `true` | whether the web opens a thread from its history; `false` opens it as it always did, by replaying the log |
| `ui.history.projection`, `ui.history.maxTurns` | the build's, `server.history.maxTurns` | not keys of `ui.history`: the `projection` version this server writes, and the largest page it accepts (where the web's growing pages stop), served beside the others; a client with stored frames compares the version before it paints from them |
| `server.history.maxTurns` | 100 | the largest `limit`, and the most a `since` read goes back |
| `server.history.maxPageBytes` | 4 MiB | see rule 4 |

`ui.history.initialTurns` and `pageTurns` may not exceed `server.history.maxTurns` (the file is refused at startup). Full table: [`config.md`](config.md).

**The presence of `ui.history` in `GET /api/config` is the capability.** It is served only by a process that mounts the AG-UI surface on a role that serves routes (`orchestrator/bin/orchestrator/src/config.rs`, `Config::public_config`), so an
orchestrator without the route, an older one, or a worker leaves it out, and a client falls back to the connect stream from the first event, as it did before.
**A reader with no session does not learn it.** `GET /api/config` is behind the identity layer, so the web, opened on a public link with no sign-in, never reads
`ui.history` and replays the log through the public connect route, as it did before; `getPublicSharedThreadHistory` is served, limited and tested for a client that
knows the route is there, and the web will use it when the configuration has a public twin (not built).
The `ui.clientCache` key of [ADR 0060](../decisions/0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md) is not built.

**What a read costs** is exposed at `/metrics`: `history_pages_total`, `history_events_folded_total` and `history_fold_seconds_total` (this process's, naming no thread and no person).

## Tests the contract has

- `orchestrator/crates/agui-projection/tests/history.rs`: the tiling property of rule 1 over every golden and over generated logs (steering messages, forks,
  surfaces, usage, a settled chain followed by an open one, a thread with no person message, a thread of one turn, a log that ends in frameless events),
  for every `limit` and with a byte cap smaller than a chain; the self-containment listing of rule 5, with a surface updated in a later turn; a steered chain
  never cut; `before`, `since` and `after` on their boundaries (`before=1`, `before` beyond the log, `limit` larger than the thread, a thread whose only chain is
  open: an empty page, `end` 0).
- `orchestrator/crates/agui-projection/tests/projection_digests.rs`: the digest table of rule 7, and that the meta is part of the frames.
- `orchestrator/crates/surface-agui/tests/contract.rs`: each documented status is answered and no other, and each body validates against the contract's schemas (the frames against the
  vendored AG-UI schema, as the stream's do). `tests/history.rs`: the pages tile the connect stream over HTTP, `end` is a cursor, the open chain is not in a page, authorisation as
  for connect (404 for another person's thread whatever the roles, 403 without `thread.read`, one body for every dead link), a deleted thread is a 404, the public
  route takes its permit before it reads and gives it back on every exit, and the counters.
- `orchestrator/crates/agui-projection/tests/carry.rs`: the invariant of rule 9 for every page of every walk of every golden and of generated logs (usage of tasks, steps and asked
  agents, totals, the page cut by a small byte cap), that the oldest page and a catch-up have no carry, that the files are named once each and at most 500 (the newest),
  and the two fixtures of `examples/history/*.walk.json` that the web's tests read (`UPDATE_GOLDEN=1` rewrites them). `web/mock/history.test.ts`: the mock's copy of the carry finds the carry of
  each page of those fixtures. `web/src/features/chat/lib/agui/history-carry.dom.test.tsx`: the web's folds, from the carry and the pages held, show the thread's totals,
  and `ThreadAgent` does the same page by page; `history-turns.dom.test.tsx`: the numbers of the turns of 25 goldens do not change as older pages load.
