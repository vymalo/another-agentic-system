# ADR 0059 — A thread opens at its end: the transcript is seeded off screen and shown at the bottom; a paged history read is built only if that is not enough

- **Status:** proposed (2026-10-09, revised the same day after a review), on the owner's words of that day: *"load messages from the bottom, so that
  long sessions won't load in front of the eyes of the user, while scrolling down – it takes 3-4s for long chats and it's annoying."* **Design,
  and since then built (see the amendments).** The names, defaults and limits are the planner's and the owner may revisit them. It builds on the static export and the desktop
  app of the open pull request (`wip/tauri`), cited below as "after the static-export PR". Extends [ADR 0012](0012-ag-ui-user-facing-protocol.md) and
  [ADR 0029](0029-forking-a-thread-copies-its-log.md) (a fork's log is a copy, so a fork is shown like any thread). The copy that a device keeps is
  [ADR 0060](0060-the-client-keeps-a-bounded-copy-of-recent-threads-behind-a-chatstore-port.md). The paged read, if it is built, is specified in
  [`docs/api/history.md`](../api/history.md) (proposed then; built since, and in `chat-api.yaml`).
  *What the review changed:* a full replay seeded off screen is now the first thing built after the web-only fix, and the orchestrator's cursor waits
  for a measured need (gate G); pages cover settled chains only and the stream says the open one; the fold's cost when reading back is named;
  the seed's safety conditions are listed; the slices are reordered so that nothing ships before the panels are right.
  **Built since (2026-10-09):** option C, [below](#built-2026-10-09-slices-6-to-10); the owner has not tried it.
- **Amended (2026-10-09, measured):** slices 0 and 1 are built, and a spike of option B was measured; [below](#measured-2026-10-09-slice-0-slice-1-and-a-spike-of-b).
  **Gate G fails for B**, so C (the history read) is the path after slice 1.
- **Amended (2026-10-10, after review):** a reader of a public link reads no configuration and waits for none; an older page is held only once the transcript has it;
  the fold applies the byte cap as it goes; the chart has a kill switch and the bounds; a 404 of the history route opens the thread by the replay; "Load earlier turns" stops after
  a few pages; what a carry cannot do is pinned. [Below](#changed-after-the-review-2026-10-10).
- **Amended (2026-10-09, built, slices 6 to 10):** the web opens a thread from its newest turns and reads older ones on scroll-up, the carry
  (token totals, kept files, turn numbers) and Sources on demand are in, a link to an old message lands on it, `ui.history.windowed` is on by default, and
  the open is measured before and after; what differs from the decisions above is listed [below](#built-2026-10-09-slices-6-to-10).
- **Amended (2026-10-09, built, slices 4 and 5):** the fold (`orch_agui_projection::History`), the three routes (`getThreadHistory`, `getSharedThreadHistory`,
  `getPublicSharedThreadHistory`, in `surface-agui`, not in `orch-api`, beside the connect routes they share their authorisation with), the configuration
  (`ui.history`, `server.history`) and the contract (`chat-api.yaml`, [`docs/api/history.md`](../api/history.md) now the rules of a built contract) are
  built; the `carry`, the web and the measurement follow in the slices below. The server's cost-lowering option is the **web's growing pages**, measured
  [below](#measured-2026-10-09-the-history-read).

## Context

What happens when a person opens a long thread, from reading the code on 2026-10-09 (a claim not read is marked *unverified*):

1. **There is one read, and it is everything.** The only way for a client to get a thread's events is `connectThread`
   (`GET /agui/threads/{id}/connect`), from `Last-Event-ID` or from the first event. The server reads the log from event 1 and folds all of it,
   writing frames only after the cursor ([`agui.md`](../api/agui.md#connect-binding): "a connect reads the thread's log from the start, even for a
   cursor at its end"; `orchestrator/crates/agui-projection/src/connect.rs`). `exportThread` is the whole log as a file. The store's `list_events`
   is forward only (`after`, `limit`: `orchestrator/crates/ports/src/store.rs`).
2. **The web applies the runs one after the other.** `ThreadAgent` (`web/src/features/chat/lib/agui/thread-agent.ts`) turns the stream into
   `ExternalRun`s and `driveExternalRuns` (`live-runs.ts`) applies each: wait until the transcript shows the earlier runs (`untilShown`, a React
   render), `thread.append` the person's message, wait again, `thread.startRun`. Its comment says why: `append` hangs a message off the transcript
   the runtime shows, and a run applied too early would start a branch and replace the earlier runs. A thread of *R* runs is *R* sequences of renders
   over a transcript that grows, and `useTurnSteps` and `useThreadFilesList` (and `useSources` while the panel is open) read all the messages on
   every change. *Unverified:* that this is where the 3 to 4 seconds go. The owner measured the symptom; we have not (slice 0).
3. **The page scrolls while it fills.** The viewport is `ThreadPrimitive.Viewport` with the class `scroll-smooth` and no scroll prop
   (`web/src/components/assistant-ui/elements/thread.aui.tsx`). `@assistant-ui/react` 0.15.22 defaults `scrollToBottomOnRunStart` and
   `scrollToBottomOnInitialize` to true and calls `scrollTo({top: scrollHeight, behavior})`, with `behavior: "auto"` on a run start, which the CSS
   turns into a smooth scroll (`useThreadViewportAutoScroll.ts` in the installed package; props documented at
   <https://www.assistant-ui.com/docs/api-reference/primitives/thread>; *verified 2026-10-09*). The transcript is drawn from the first replayed run
   (`Thread loading={!loaded}` shows a skeleton only while there are no messages), so the person sees turns arrive and the page chase the bottom.
   *Unverified:* that `thread.runStart` fires for every replayed run; slice 1 finds out.
4. **Panels read the whole transcript.** Sources (`features/panel/lib/sources.ts`) is derived from every message, including the links in the
   agents' words; the steps tree numbers turns by position ("Turn 3"); the usage ring folds every `vymalo.usage` frame
   (`features/chat/lib/usage.ts`); an A2UI `Image` may only name a file that some message of the thread kept
   (`features/chat/lib/a2ui/prepare.ts`, `checkImage`). A transcript that holds only the end of a thread shows these wrongly unless something else
   supplies the rest.
5. **Reading back a thread in pages costs a fold each time.** Every `STATE_SNAPSHOT` carries state built from the start of the log (job number,
   catalog, tools, fork origin, usage), and the `Projector` keeps every message id, text record and run id it has seen (`message_ids`, `texts`,
   `run_ids`: `projector.rs`). A page of older turns therefore needs a fold of the whole prefix before it, and a store index of where turns start
   cannot skip it. Reading a thread of *L* events back in pages of *p* events costs *L²/p* event folds: 20 000 events in pages of 200 is two million.

## Options considered

| | What | Verdict |
|---|---|---|
| A | **Web only.** Hold the transcript back until the replay is applied, then show it at the bottom; no smooth scroll except the button. | Removes the scroll the owner complains of, none of the cost. **Slice 1.** |
| B | **Seed the full replay off screen in one import, and show only its tail.** The whole log comes as today, is replayed in a runtime nobody sees, and the visible runtime is given the last *N* turns in a single `import`; older turns are in the off-screen transcript and join it on scroll-up. | **No server change**, one mechanism (the seed) that C and ADR 0060 need anyway. Its cost is the wire and the off-screen replay; if that meets the target it is the whole answer. **Slices 2 and 3, then gate G.** |
| C | **A finite `history` read of the same projection**, from the newest settled chain back, and the stream resumed at its `end`. | The orchestrator's cursor. More work on both sides, and each older page folds the prefix again (context 5). **Built only if B misses gate G.** |
| D | `?tail=<n>` on `connectThread`. | Older turns need a second mechanism anyway, and a cached open and a first open would differ. Rejected. |
| E | Offset paging of raw events, or of REST messages, projected in the browser. | The projection is Rust and golden-tested (ADR 0012); a second one in TypeScript is a second truth. Rejected. |
| F | One `MESSAGES_SNAPSHOT` for the old turns. | A message list has no run, subagent or actor markers, which the turns and the steps tree read. Rejected. |

## Decision (proposed)

1. **Order, and gate G.** Measure (slice 0). Stop the visible scroll (slice 1). Build the seed (slice 2) and open threads with it (slice 3). **Gate
   G:** on a thread of 1 000 turns whose turns are as large as slice 0 found real ones to be, the person has a usable page at the bottom within
   **1 second of navigating** (p50, a mid-range laptop, the compose stack), no `scroll` event follows, and the memory held is below a figure we
   write down. If B meets it, C is not built: `docs/api/history.md` stays as a rejected proposal, and the slices 4 to 9 below are dropped. If it
   misses, C is built, and **its target is stricter, because it adds a request**: the response in **300 ms** at the server for a thread of 10 000
   events (the fold, the read and the serialisation, p50), and the page usable **500 ms after it arrives**.
2. **Chains and turns (for C).** A *chain* is the events from one that opens a run while none is open to the first event after which no run is open (a
   steered message extends it: the projector closes the open run and opens its own in one event, `RunClose::Superseded`, so no settled point lies
   between). A *turn* is a chain that starts with a person's message. A page is a whole number of **settled** chains, never the open one, so its last
   event is a settled point, `end`, which is a clean `Last-Event-ID`; the stream says the chain in progress, in full. Cuts fall at chain starts only,
   including for the byte cap, so a thread with no person's message is bounded too ([`history.md`](../api/history.md#chains-turns-and-what-a-page-holds)).
3. **The read (for C).** `getThreadHistory` and the two shared operations: `before`, `limit` or `since`, or `after` for a catch-up; the answer is
   JSON with `start`, `end`, `head`, `earlier`, `projection`, `frames` and, for a page that is not a catch-up, a `carry`; `after` brings an `anchor`.
   Pages tile a **replay** over a fixed `ThreadMeta`, not "the stream": a live stream's frames are rewritten by `LiveOverlay::logged`
   (`live.rs`: the continued message loses its `START`, its `CONTENT` is cut to the unsaid words, a given-up message is renamed `<id>~final`) and the
   early snapshots carry the thread's current title and description (`meta_of`). So a client **stores only what a page returned**, never a frame
   written after a stream caught up, and takes the title, description and share from the resource.
4. **What the fold costs, and what would lower it.** The pure fold keeps a ring of chain buffers, so memory is bounded by the page and nothing in the
   core, the store port or the event model changes. The cost per page is a connect's (a read and a fold of the prefix), and reading back is *L²/p*
   (context 5). Options that work, in the order we would try them: **(a)** the web asks for pages that grow (20, 40, 80 turns), which makes the sum
   *L log L*; **(b)** a silent fold that builds no frames before the page (the work is the state, not the output); **(c)** a short-lived in-process
   cache of projector states at chain starts, per replica, as a latency optimisation only (the log decides, invariant 3); **(d)** a stored projector
   checkpoint, which is as large as the thread, so only at coarse intervals and only if the state is first made bounded; **(e)** a projection change
   that makes the state at a settled point small (the dedupe sets limited to the job, the catalog and usage read from their own columns), which is
   the real fix and a change of every golden. A stored index of where turns start is **not** on the list: it names where to stop reading, not what
   the state is. Slice 0 measures the cost of reading a 10 000-event thread back to its start, with and without (a).
5. **Capability by configuration (for C).** `GET /api/config` gains `ui.history` (`initialTurns` 12, `pageTurns` 20, `projection`); its presence
   means the route exists, so a static web, an app built last month and an orchestrator that has not rolled out all work together, falling back to
   the full replay of B. `server.history.maxTurns` and `maxPageBytes` bound a page; the newest chain is returned whole.
6. **The web opens at the end.** *With B:* the stream is read as today but into the **off-screen** runtime; when it has caught up, the last *N*
   turns are imported into the visible runtime in one step and shown at the bottom. The skeleton shows until then; nothing moves on screen because
   nothing is on screen while it happens. The off-screen transcript stays in memory (this is the "memory held" of gate G) and serves scroll-up, the
   usage fold, the files an `Image` may name, the labels and Sources. *With C:* the page's frames are seeded the same way and the stream resumes at
   `end`.
7. **The seed is the one mechanism, and it has conditions.** The runtime has no "insert before". `useAgUiRuntime` passes `onImport` and `setMessages`
   to `applyExternalMessages`, so `thread.import(repository)` replaces the transcript with a linear chain of the messages given, keeping their ids;
   `thread.export()` gives the current ones (`@assistant-ui/react-ag-ui` 0.0.62, `@assistant-ui/core` 0.3.21, *verified 2026-10-09*). The messages must
   be built by the library's own aggregator, or the turns would differ from the live ones: a **scratch runtime**, a second `useAgUiRuntime`
   mounted without a view, driven by the same `ExternalRun` replay, whose `export()` is imported into the visible one. `applyExternalMessages` does
   more than replace: it clears `pendingA2uiAction` and `pendingA2uiResumeOwner`, drops the resume target if its message is missing, and can
   **hard-replace** the session when an existing message's parent differs (`AgUiThreadRuntimeCore.js`, *verified 2026-10-09*). So:
   - **When.** An import is made only when no message is streaming, **no interrupt or form is pending and no A2UI action is staged**
     (`unstable_getPendingInterrupts()` is empty, `ThreadAgent` has no staged action), and no send is in flight. Otherwise it waits.
   - **Older turns wait for the end of a run.** While a run is open, scroll-up shows the turns already in the visible runtime and a row that says the
     rest will load when the agent is done; the import happens at the next settled point. (With B they are already in memory, so only the import waits.)
   - **Nothing is delivered during an import.** `ThreadAgent` holds the stream's groups from the moment an import begins until it has committed, then
     releases them in order; the connect is opened, or resumed, only after the first seed (ADR 0060's reader follows the same rule).
   - **Repeated ids.** `applyExternalMessages` keeps the *first* copy of a repeated message id (`seen.has(id)`), so an older page put in front would
     hide a newer copy. The merge of `older ++ current` removes a repeated id **keeping the newest**. This matters for exactly the activities that
     are said again, whole, by a later event: an A2UI surface keeps the id `a2ui-<seq of its first event>` and is snapshotted again by every event
     that touches it, in later turns too, until a new job clears it (`projector.rs`, `surfaces`, `forget_job`). C's tests list every such id
     ([`history.md`](../api/history.md#rules), rule 5).
   - **Gate of slice 2**, on every golden and on generated logs: the transcript built by replaying one run at a time equals the seeded one (message by
     message, part by part, ids aside); this includes **a surface updated in a later turn**, **a steering message** (two runs in one event), a fork, a
     pending question and a staged action (the import waits and loses nothing), and a run in flight (its message keeps streaming, groups held during
     the import arrive after it). A seed of 20 turns takes a time we write down. If it fails: first a small patch that exports the aggregator
     (`web/patches/UPSTREAM.md` shows the package is patched already), then a converter of our own checked against the goldens.
8. **Older turns on scroll-up, without a jump.** A sentinel above the first turn (an `IntersectionObserver` with a margin of about a screen) asks
   for the next older turns when there are any; one at a time; a row says "Loading earlier messages" and, on failure (C only), offers Retry. The
   position is kept by an **anchor**: before the import the first turn in view and its offset from the top of the viewport are noted, after the
   commit (a layout effect, before paint) the viewport is moved so the same turn has the same offset, `behavior: "instant"`. The browser's scroll
   anchoring is not relied on: `overflow-anchor` reached Safari in 27 (MDN browser-compat-data, *verified 2026-10-09*), and older iOS web views are
   Safari's. `scroll-smooth` stays only on the "Scroll to bottom" button; the run-start scroll is off while a seed is applied.
9. **The live stream continues as before.** Nothing in `connectThread`, the run route, `ThreadAgent`'s grouping, reconnect and backoff, live text,
   steering or cancel changes. A run from another tab or a webhook arrives as an `ExternalRun` and is appended after the seed, as now.
10. **Forks and edits need nothing special.** A fork's log holds the parent's events up to the cut with the same `seq`, then `thread_forked`, then its
    own: its end is its own turns and the marker, and its older turns are the parent's conversation. `seqOfUser` and `endOfRun`, which "fork from
    here" and "Edit" read, exist for the turns that were delivered, and the buttons exist for those. The versions picker (`listBranches`, REST) is
    untouched; a link to another version (`/threads/<id>#m-<seq>`) may name a message far from the end: see 13.
11. **Panels.**

    | Readout | With B (full seed) | With C (pages) |
    |---|---|---|
    | Transcript, the steps of a turn | the visible runtime holds the last *N* turns; the rest is in the off-screen transcript | the loaded turns |
    | Token ring, totals, groups | the usage fold has seen every run (the off-screen replay folds them) | **server:** `carry.usage` is the *initial state* of the fold, then the loaded pages' usage frames; the invariant is that `summarize` equals the whole thread's |
    | Files an `Image` may name | all the thread's files, from the off-screen transcript | **server:** `carry.files` (at most 500) unioned with the loaded pages' |
    | Turn labels ("Turn 3") | exact: all turns are known | **server:** `carry.turns` offsets the position (default until the owner decides: the server's numbering; it counts chains with agent output and may differ from the web's `drawsPart` by an odd card-only run; question 71) |
    | Sources | on demand, from the off-screen transcript and the visible one | **lazy:** from the loaded turns, "Sources from the last N turns" and a button that loads the rest; no automatic crawl (default until the owner decides; question 72) |
    | Branch versions, Export JSON, fork, edit | REST and the server | unchanged |

12. **Shared and public readers** are shown by B exactly as the owner's thread (their stream is the reader's projection). With C they get the same
    routes over their projections; the public one holds one of the link's stream permits for as long as it folds, and is rate limited like its
    connect. Nothing is stored for them (ADR 0060).
13. **Links to an old message.** `useScrollToMessage` waits for `#m-<seq>` in the DOM. With B the message is in the off-screen transcript and is
    imported with the turns around it; with C the web asks for `since=<seq>` until it is loaded or `maxTurns` turns are reached; beyond that it opens
    at the end and offers "Load earlier". The visible transcript stays contiguous.

```mermaid
sequenceDiagram
  autonumber
  actor P as Person
  participant W as Web (ThreadAgent, transcript)
  participant S as Seed (runtime, off screen)
  participant R as Chat runtime (on screen)
  participant O as Orchestrator
  P->>W: opens /threads/T
  W->>O: GET /api/config, GET /api/threads/T
  alt B, the full seed
    W->>O: GET /agui/threads/T/connect (from the start)
    O-->>W: the whole replay, then live frames
    W->>S: every run, replayed off screen
  else C, history pages (only if gate G failed)
    W->>O: GET /agui/threads/T/history?limit=12
    O-->>W: HistoryPage of settled chains: frames, end, carry
    W->>S: the page's runs
  end
  S-->>W: messages
  W->>R: import(the last N turns), shown at the bottom, no scroll
  Note over W,R: groups of the stream are held while the import commits
  P->>W: scrolls near the top
  W->>R: when no run is open: import(older + current), anchor kept
```

```mermaid
stateDiagram-v2
  [*] --> Closed
  Closed --> Seeding: opened
  Seeding --> Live: imported, shown at the bottom
  Seeding --> Failed: network, 5xx
  Failed --> Seeding: Retry
  Live --> LoadingEarlier: the top is near and there are older turns
  LoadingEarlier --> Live: imported, anchor kept
  LoadingEarlier --> Live: failed (C), a row with Retry
  Live --> WaitingForRun: older turns wanted while a run is open
  WaitingForRun --> LoadingEarlier: the run settled
  Live --> Complete: no older turns
  Seeding --> NotFound: 404
  Live --> NotFound: 404 on a reconnect (deleted, restored from a backup)
  LoadingEarlier --> NotFound: 404
  NotFound --> [*]: Thread not found
  Complete --> [*]: left
  Live --> [*]: left
```

### Measured (2026-10-09): slice 0, slice 1, and a spike of B

*Measured 2026-10-09* with the slice-0 measurements (`web/e2e/open-long-thread.spec.ts`, `web/e2e/open-probe.ts`, the mock's
`POST /__mock/long-thread?turns=n`), unthrottled headless Chromium on a shared, loaded 4-core machine. The absolute numbers are that machine's;
their shape is the finding.

- **The replay is quadratic, and it is the client's.** The last turn is in the page after 4.1 s for 20 turns, 12.4 s for 50, 36 s for 100 and
  139 s for 200. The wire and parsing cost 98 ms for 200 turns, and layout and style 3%. The rest is the runtime applying run after run.
- **Slice 1 (built): the transcript is not drawn while the log replays,** then is drawn whole and scrolled at once. Scroll events after the first
  turn drop from 58 and 158 (40 and 200 turns) to 0. First paint and final scroll go from 9.2 s / 9.7 s to 6.3 s at 40 turns, and from
  75.9 s / 77.0 s to 39.1 s at 200. Hiding a mounted transcript removed the scroll but not the cost, so the hold does not mount it.
- **The spike of B (measured, not merged):** `thread.import` itself takes 1 to 7 ms and one draw of the imported transcript 0.36 s to 2 s. The
  scratch runtime that makes the transcript costs 230 to 270 ms a run, so a full seed is slower than slice 1 (40 turns: 10.7 s against 6.3 s; 200:
  46.5 s against 39.1 s). It also logged React error #185 14 to 19 times per 200-turn open, and 1 open in 6 never finished. **B misses gate G by far.**
- **What `import` resets**, as measured (`runtime-import.dom.test.tsx` on the spike branch):
  - the transcript is replaced, with its ids kept;
  - a pending interrupt survives only if its message is in the import;
  - an open run is not canceled, but the person's message that opened it is lost and its reply comes back as a new last message;
  - a queued A2UI click is forgotten.

  `thread.startRun` resolves before the run ends, so a replay counts as done when its last run *starts*.

So the cost is per run applied, and it grows with the transcript. The next step is C: the history read, so that a first open applies at most
*N* turns, plus slices 4 to 9 behind the flag.

### Measured (2026-10-09): the history read

*Measured 2026-10-09* on the same shared 4-core machine, release build, a synthetic thread of 12 000 events (1 000 turns of a message, two steps, a usage report,
the agent's words, a file and the end): `orchestrator/crates/agui-projection/tests/history_cost.rs`.

- **The server.** The newest 12 turns fold in **61 ms** (p50 of 9), well inside the 300 ms the target allows for the read, the fold and the serialisation, so
  neither a cache of projector states (option c) nor a silent fold (option b) is built.
- **Reading back.** Pages of 20 turns back to the start of the thread fold 306 000 events in 1.33 s over 50 pages (*L²/p*); pages that grow (20, 40, 80, 100)
  fold 85 000 in 0.49 s over 12 pages. The option built is **(a): the web asks for growing pages**.

### Built (2026-10-09): slices 6 to 10

The web side of option C. The fold, the routes, the contract and the configuration were slices 4 and 5; what is new is how the page uses them
([`web/README.md`](../../web/README.md#opening-from-the-history) has the sequence and the states).

- **A thread opens from its newest turns.** `ThreadAgent` reads `history?limit=<initialTurns>` (12), hands the page's runs over as the seed, holds the stream
  until the seed is in the runtime and follows it from the page's `end`. A page that cannot be read, a seed that cannot be made and an orchestrator without
  `ui.history` open the thread the old way, from event 1; a 404 is "Thread not found". **`ui.history.windowed` is on by default now**, as slice 9 said it
  would be once 7 and 8 were in; a deployment that wants the replay sets it to `false`.
- **Older turns on scroll-up**, in pages of 20, 40, 80 turns up to `server.history.maxTurns`, one at a time, the turn being read kept in place (a noted
  offset restored in the layout phase, not the browser's scroll anchoring), a failed page with Retry. Reading back is *L log L* on the server
  ([measured](#measured-2026-10-09-the-history-read)).
- **The readouts of the turns that are not held** come from the `carry`: the token ring and its groups, the kept files an `Image` may name, and the numbers of
  the turns ("Turn 49" stays 49 when older pages load). `carry.rs` checks, for every page of every walk of every golden and of generated logs, that the state
  built from the carry and the pages held equals the whole thread's, against a second implementation of the web's usage fold; the web's tests do the same with
  its own folds on the pages the orchestrator wrote.
- **Sources on demand.** The tab says "Sources from the last 12 turns" and the count has a `+` while turns are not loaded; a button loads the rest, one page
  after the other. Nothing is loaded unasked (question 72).
- **A link to a message** (`#m-<seq>`) asks `since=<seq>` for the first page and scrolls to the message. A link further back than `maxTurns` (100 turns) opens
  at the end and says so; it does not crawl pages until the message is found (decision 13 said "offers Load earlier", which is what the row does).
- **Docs.** `docs/api/history.md` and `chat-api.yaml` (slice 5), `agui.md`, `architecture.md` (the sequence and the outbound flow), `web/README.md`, the crate READMEs.

What differs from the decisions above, and why:

1. **The seed is made by the runtime's thread core, not a scratch `useAgUiRuntime` (decision 7).** `AgUiThreadRuntimeCore` is exported by the pnpm patch
   ([`web/patches/UPSTREAM.md`](../../web/patches/UPSTREAM.md#export-the-thread-core)) and driven the way `LiveRuns` drives the runtime, without a view and
   without waiting for a render: 4 to 10 ms a run, against 230 to 270 ms for the scratch runtime that the spike measured. The gate of slice 2 (the transcript
   built from one run at a time equals the seeded one) holds on every golden (`seed.dom.test.tsx`), and now across pages: `windowed.dom.test.tsx` cuts every
   golden at every place a chain starts and holds `older ++ newer` equal to the replay of the whole.
2. **A question that waits for the person does not hold an older page back (decision 7, "When").** The import is `older ++ current`, so the message of the
   question is in it, and the runtime reads a pending interrupt off the last assistant message; `use-earlier.dom.test.tsx` shows the question still pending
   and answerable afterwards. Waiting for the person to answer before older turns could load would make the history unreachable exactly while the agent is
   waiting for them. A run that is open or on its way, a message being sent and a staged A2UI action still hold the import back (`idleForImport`), and the
   runs the stream delivers are held while it is made (`pauseRuns`); a staged action and a send in flight have tests of their own.
3. **The question an older page ended on is settled when a page follows it.** Found by `windowed.dom.test.tsx` on its first run: in seven of the goldens the joined
   transcript differed from the replay in one place, the status of the message of a question that a later turn answered (`requires-action`, where a replay
   leaves it `complete`, because the next run clears it). `buildMessages(runs, threadId, followed)` clears it the way the runtime does.
4. **Two scroll behaviours of the library's, found by the link.** `@assistant-ui/react` 0.15.22 pulls a viewport back to the end whenever what it holds changes
   size while it believes the person has not scrolled up, and it schedules such a pull once when a thread first has messages (`scrollToBottomOnInitialize`).
   It decides that the person scrolled up from a scroll event, which arrives after the resize that follows a programmatic scroll, and only if the content's height
   did not change in between (`isUserScrollUp` compares `scrollHeight`). So a scroll to a linked message made while the transcript settles was taken back,
   most of the time under load (*verified 2026-10-09* in the installed package, `useThreadViewportAutoScroll.js`, and in a browser: the link test failed in 2 of 3
   runs before). `scrollToBottomOnInitialize` is off (the reveal scrolls to the end itself) and the viewport does not follow the end for two seconds after a link
   has put a message in view (`Thread`'s `pinned`).
5. **The mock's failure and delay hooks are per thread**, so that two specs that run side by side do not spend each other's failure.
6. **The page does not crawl for a link beyond `maxTurns`**, as above.
7. **A reader with no session replays the log (decision 12).** The web learns `ui.history` from `GET /api/config`, which is behind the identity layer, so a public
   link opened signed out opens by the replay through the public connect route, as before. The public history route is served, rate limited, takes a stream
   permit and is tested, but the web has no way to know it is there until the configuration has a public twin, which is not built (what is left). A reader
   who is signed in (an internal link, or a public link with a session) is served the history route of the link.
   *Amended 2026-10-10:* the page of a public link does not read the configuration at all (it is the defaults at once) and does not wait for it. Before, it asked
   `GET /api/config` through the signed-in client, where a 401 is held for a sign-in for up to ten minutes (`withSessionRefresh`, `PARK_MS`), and the stream
   waited for the answer: a reader with no session saw a skeleton. The test stubs `NEXT_PUBLIC_SIGN_IN_PATH`, the setting under which the client holds the request, and
   fails without the change.

### Measured (2026-10-09): opened from the history

*Measured 2026-10-09* with the slice-0 measurements (`web/e2e/open-long-thread.spec.ts`, now with `OPEN_HISTORY=windowed|on|off`, which records the pages of history and the
heap too), on the same shared 4-core machine, headless Chromium, no throttling, the web's mock on the loopback. Medians of 3 to 5 opens. `on` is the replay with
the transcript held (slice 1); `windowed` is option C.

| | 40 turns | 200 turns | 1 000 turns |
|---|---|---|---|
| Replay as it was (slice 0): first turn painted | 9.2 s | 75.9 s | not measured |
| Replay with the transcript held (slice 1, today): first paint, script, heap | 6.5 s, 4.9 s, 32 MiB | 44.8 s, 34.0 s, 110 MiB | the tab died |
| **From the history: page arrived, first paint** (12 turns, at the bottom) | **0.65 s, 1.14 s** | **0.65 s, 1.13 s** | **0.75 s, 1.17 s** |
| From the history: page size, script, heap | 71 KiB, 0.71 s, 18 MiB | 72 KiB, 0.76 s, 18 MiB | 72 KiB, 0.72 s, 18 MiB |
| `scroll` events after the first paint | 0 | 0 | 0 |

- **The open is flat in the length of the thread**, 1.1 to 1.2 s from navigation to the newest 12 turns painted at the bottom, for 40, 200 and 1 000 turns; the
  replay's grows with the turns and it did not open a thread of 500 turns or more at all (the tab died after about two minutes, a trap in Chromium's compositor
  thread; the cause is *unverified*, the heap at 200 turns is 110 MiB).
- **Gate G, read honestly.** The first target, a usable page within 1 s of navigating on a 1 000-turn thread, is **missed by 0.17 s** on this loaded machine
  (1.17 s; the first 0.75 s of it is the app loading, `GET /api/config` and the request, and a mid-range laptop on the compose stack was not available to measure).
  The stricter targets of C, which adds a request, are met: the fold of the newest turns takes 61 ms on 12 000 events (release; target 300 ms at the server),
  and the page is usable 0.42 s (1 000 turns) to 0.50 s (40 turns) after the response arrives (target 500 ms). No `scroll` event follows. The memory held is
  the loaded turns' alone: 18 MiB of heap for the 12 newest turns of any thread, against 110 MiB for the replay of 200.
- **Scrolling back costs what it holds.** Loading every older turn of a thread (the Sources button, pages of 20, 40, 80, then 100 turns, one import each) took
  7.4 s and left 202 MiB of heap for 300 turns, and 61 s and 905 MiB for 1 000, with no crash (a throw-away probe, not kept). The transcript is not windowed out
  of view (Consequences), so a very long thread read to its start is the next thing to measure and, if it is slow, to virtualise.
- **Not measured**: the compose stack with a real orchestrator (the system specs `web/e2e-system` run the windowed open there in CI and have not been run
  here), a phone, a CPU slowed to a laptop's.


### What a first open costs

| | Today | B, the full seed | C, history pages |
|---|---|---|---|
| Orchestrator reads and folds | the whole log | the same | the same, and again for each older page (context 5) |
| On the wire | every frame | every frame | the frames of *N* turns and the carry |
| In the browser | *R* runs applied one by one on screen | *R* runs replayed off screen, one import of *N* turns | at most *N* runs seeded, one import |
| Memory | the transcript | the transcript off screen and on | the loaded turns |
| Unknown until slice 0 | how the 3 to 4 seconds divide between the fold, the wire and the renders | whether the wire and the replay of a 1 000-turn thread of real size fit one second | the fold's share on 10 000 events, and the cost of reading back |

### Changed after the review (2026-10-10)

An independent review of the branch asked for these before a pull request; each is built and tested.

- **An older page is held when the transcript has it (M1).** `ThreadAgent.readEarlier` reads and checks a page and returns its runs with `commit`; the window, the usage,
  the turns before it and the files take the page in only after `thread.import` succeeded. A page that waited minutes for a moment to import showed labels and totals of turns
  the screen did not have, and one whose import failed left them, so Retry asked for the page after it. The hold on the runs is looked at again once taken (a run that began in
  between would lose its message), a page written by another projection than the ones held (`PageMeta.projection`) is refused, and the question of the newest turn is shown to
  be answerable after an import, not only pending.
- **The byte cap is applied as the fold goes (M2).** The ring keeps a running total and drops the oldest turn, with the chains that ride along with it, once the ones after it still
  weigh more than a page may (the end of the read would have dropped them too, so the page is the same: a capped page is the newest part of the uncapped one, tested over
  generated logs and any cap). A catch-up stops at the cap. `History::peak_bytes` is the most the ring held: on 300 turns of 6 KB it is the cap and two chains for a page, a page
  before one and a catch-up.
- **The chart has a kill switch (M3).** `orchestrator.history.windowed` (`ui.history.windowed`, default true) and the counts and bounds of `ui.history` and `server.history` are values,
  written only when they are not the orchestrator's defaults, so that the default render stays readable by an image built before them; `render-check.sh` asserts the render and the refusals.
- **The shapes of a long walk (M4).** `surface-turns`, `steer-turns` and `form-turns` walks (`tests/carry.rs`) are joined one page at a time in the web and equal the replay of the pages held
  after each.
- **Smaller.** A 404 of the history route opens the thread by the replay (an orchestrator that does not serve it answers 404 too; the stream says whether the thread is there). "Load earlier
  turns" reads at most five pages a click. The permit of a public read is tested under overlapping reads. A call a page says again after the carry counted it is counted twice by that reader:
  the carry is a summary and names no call (a test pins the limit; agents say a call once per task).

## Consequences

- The visible scroll goes in slice 1 whatever else happens; the wait is cut by B if its numbers allow, and by C if they do not.
- The seed is a second instance of a library hook and a set of conditions around `import`. If the gate of slice 2 fails, the cost moves to a patch of
  `@assistant-ui/react-ag-ui`.
- If C is built, the projection gets a **version** and a digest table, the contract gains three operations and the web a generated client, and
  `GET /api/config` gains `ui.history`; an older orchestrator or web keeps working.
- Turn labels, Sources and `Image` get server help or a visible "from the last N turns" only with C; each is a rule to keep equal to the web's, and
  the tests that compare a windowed reading with the whole one are the guard.
- The loaded transcript grows as the person scrolls back and is not windowed out of view. Virtualising turns out of view (`@tanstack/react-virtual`
  is already a dependency, used by the steps tree) is a later slice if measured, and it costs browser find-in-page.
- A device copy (ADR 0060) needs canonical frames: with C they come from `history`; without C, from the replay phase of a connect (before it caught up).

## Facts

- *Verified 2026-10-09:* the connect route folds from event 1 (`surface-agui/src/connect.rs`, `app.rs` `events_after`); `list_events` is forward only;
  `events` has the primary key `(thread_id, seq)` (`0001_init.sql`); the web's replay path (`live-runs.ts`); `scroll-smooth` and the library's scroll
  defaults; `thread.import` reaches `applyExternalMessages`, which clears the pending action and owner, may hard-replace, and keeps the first copy
  of a repeated id; the projector closes an open run and opens another in one event for a steering message (`projector.rs`); a surface keeps
  `a2ui-<first seq>` (`projector.rs`); `LiveOverlay::logged` rewrites logged frames (`live.rs`); the projector is built from the thread's current title
  and description (`surface-agui/src/run.rs`, `meta_of`); `overflow-anchor` support starts at Chrome 56, Firefox 66, Safari 27
  (<https://github.com/mdn/browser-compat-data>, `css/properties/overflow-anchor.json`).
- *Measured 2026-10-10* (`orchestrator/crates/agui-projection/tests/history_cost.rs`, release build, the shared 4-core machine, a synthetic thread of 11 999 events, 1 000 turns): the fold of the
  newest 12 turns (336 frames) takes **48.8 ms** (p50 of 9) and held **81 108 bytes** at most; with the byte total counted as the fold goes it was 46.4 ms before, so counting costs about 5 %
  (within the machine's noise: 61 ms and 52 ms were seen on other runs). Reading back to the start in pages of 20 folds 305 999 events in 1.28 s (1.51 s before) over 50 pages; growing pages
  (20, 40, 80, 100) fold 84 719 in 0.41 s (0.45 s) over 12. A real turn is heavier than the mock's (*unverified*: no real thread was measured).
- *Unverified:* where the 3 to 4 seconds go; that `thread.runStart` fires for each replayed run; that `import` is safe while a run streams (the
  conditions above are what we expect it takes); the size of a real turn (the goldens
  are mocks) and so the wire cost of B; that the scratch runtime's messages equal the live ones; that the runtime gives each message an id that the
  merge can dedupe.

## Open questions for the owner

[`open-questions.md`](../open-questions.md) 71 and 72, with the defaults used here **until the owner decides**: turn labels keep the server's
numbering (71); Sources loads on demand (72).

## Implementation plan

Thin slices, each with the test that closes it. Slices 0 to 3 need no server change.

| # | Slice | Done when | State |
|---|---|---|---|
| 0 | **Measure.** Generators of long threads (200, 1 000, 5 000 turns) for the web's mock and `orch-testsupport`; the byte size of real turns from exported threads; timings of the fold, of **reading a 10 000-event thread back to its start page by page**, of the wire and of the first paint; a Playwright spec counting `scroll` events during an open. | The numbers are in this ADR's status; the spec fails today on "no scroll after the transcript is shown". | built, measured |
| 1 | **Hold and reveal (web only).** The transcript is not shown until `loaded && !replaying`, then shown at the bottom; `scroll-smooth` only on the button; `scrollToBottomOnRunStart` off while replaying. | The slice-0 spec passes; the existing specs pass; a jsdom test of the reveal gate. | built |
| 2 | **The seed.** The scratch runtime, `export` + `import`, the conditions of decision 7 (when, deferral, held groups, newest copy of a repeated id). | The equality on every golden and generated log, including a surface updated in a later turn, a steering message, a fork, a pending form and a staged action, and a run in flight; the timing of a 20-turn seed is recorded. | not built: gate G failed for the full seed |
| 3 | **Open with the full seed.** Replay into the scratch runtime, import the last *N* turns, scroll-up from memory with the anchor, the 404 path. | Playwright against the mock: a 1 000-turn thread opens at the bottom with at most *N* turns in the DOM and no `scroll` event; scroll-up moves the anchor at most 1 px; a live run continues; **gate G is read from the slice-0 measurements and written into this ADR.** | not built: as 2 |
| 4 | *(only if G fails)* **The fold.** `orch-agui-projection::history`: chains, the ring, the byte cap, `settled`, `anchor`, the digest table with its CI check. | The tiling and self-containment properties of `history.md` over every golden and generated logs (steer, fork, open chain at the end, no person's message, frameless tail). | built (`History`, `tests/history.rs`, the digest table) |
| 5 | *(only if G fails)* **The routes and the configuration.** `App::history`, the three operations, `ui.history`, `server.history.*`, metrics (pages, events folded, fold seconds), the cost-lowering option of decision 4 that slice 0 chose; the contract moves into `chat-api.yaml`. | `surface-agui` contract test; authorisation and 400 cases; the public route's permit; `config.md` and the schema. | built (`surface-agui`, `ui.history`, `chat-api.yaml`) |
| 6 | *(only if G fails)* **The web's transport and window.** The generated client, a pure `Transcript` (pages, continuity), growing page sizes, the mock server answers `history`; scroll-up from pages. | Vitest on the pure part (a gap or an overlap between pages is an error); the mock's contract test; Playwright scroll-up over the mock's pages. | built (`seed.ts`, `use-earlier.ts`, the mock's fold) |
| 7 | *(only if G fails)* **The carry.** `usage`, `files`, `turns` on the server; the web builds its folds from them. | Rust: carry plus page equals the whole on the usage goldens; vitest: `summarize` equal; labels stable when an older page loads. | built (`carry.rs`, `usage.ts`, `tests/carry.rs`) |
| 8 | *(only if G fails)* **Sources on demand, deep links.** "Load earlier turns"; `since` for `#m-<seq>`. | Playwright: a link to a message 300 turns back lands on it; beyond the cap it opens at the end and says so. | built (Sources, `since` for a link) |
| 9 | *(only if G fails)* **Open from the tail, behind `ui.history.windowed` (off by default) until 7 and 8 are in**, then on. | Playwright: with the flag on, a long thread opens from `history`; with `ui.history` removed, the full seed runs; the flag flips in the same change that lands the last of 7 and 8. | built; `ui.history.windowed` is on by default |
| 10 | **Docs.** `agui.md`, `web/README.md`, `architecture.md` (diagrams), `docs/api/README.md` status. | `node tools/docs-check/check-docs.mjs` is green; every diagram node exists in the code. | built |
