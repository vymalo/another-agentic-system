# web — chat surface

Next.js (App Router, TypeScript strict), [shadcn/ui](https://ui.shadcn.com/) on Tailwind v4 and
[assistant-ui](https://www.assistant-ui.com/) on **AG-UI**: the conversation is the orchestrator's
[AG-UI 1.0](../docs/api/agui.md) projection of the event log, rendered by
`@assistant-ui/react-ag-ui`. It lets the owner pick an agent (and a release, when the agent offers
one), start threads, follow up, answer an interrupt, cancel and watch progress live (under a
verification gate: the attempt it is on, each check and its findings, and the rework), see what the
agents shared (pull requests, branches, CI runs, files, links) in a panel beside the conversation, and it keeps
following a thread that another tab or the orchestrator itself moves. Decisions:
[ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md) (AG-UI as the user-facing
protocol), [ADR 0006](../docs/decisions/0006-assistant-ui-external-store.md) (assistant-ui),
[ADR 0011](../docs/decisions/0011-web-shadcn-tailwind-feature-layout.md) (UI kit and layout);
release picker: [ADR 0008](../docs/decisions/0008-platform-integration-via-a2a-extension.md).

The UI renders what the server says. It never invents state: "running" is the thread state the
server sent, a refused send is taken back out of the transcript, and everything after a reload is
the log replayed.

## Contract

The interfaces are [`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml) (the resource API and the
AG-UI operations) and [`docs/api/agui.md`](../docs/api/agui.md) (what travels on them). Types are
generated from the contract at build time (`pnpm gen:api`, openapi-typescript + openapi-fetch);
`src/lib/api/schema.d.ts` is never committed. `src/lib/api/contract.typecheck.ts` holds deliberate
mismatches (`@ts-expect-error`) that must stay type errors, so a contract change that the client
does not follow fails `pnpm typecheck`. The AG-UI operations reference the vendored AG-UI JSON
Schema by file (`orchestrator/crates/agui-proto/schema/`, which `web/Dockerfile` copies in beside the
contract).

| The web calls | For |
|---|---|
| `POST /agui/agents/{agentId}` | a run: create a thread (the browser mints its id, a UUIDv7), send a message, answer an interrupt (`resume`) |
| `GET /agui/threads/{id}/connect` | the conversation: replay, then follow across runs, resuming with `Last-Event-ID` |
| `GET /api/agents`, `GET /api/threads`, `GET /api/threads/{id}` | the agent list, the thread list, a thread's title and target |
| `POST /api/threads/{id}/cancel` | Cancel (AG-UI has no consumer cancel) |
| `PATCH /api/threads/{id}` | **Rename** in the thread's overflow menu: the title in the top bar becomes a field (Enter or leaving it saves, Escape gives it up, the same title or an empty one is no request); a refused rename says why and keeps the field. The answer is the thread, so the header says the new title at once, and the sidebar's list is fetched again with it. A person's title is final (the orchestrator will never replace it) |
| `GET /api/threads/{id}/export` | **Export JSON** in the thread's overflow menu (the `…` of the top bar): the whole thread (messages, agent statuses, artifacts, check, CI and verifier cards, reworks, the job) as `thread-<id>.json`, to send to a developer |

The four legacy interaction operations (`createThread`, `postMessage`, `listEvents`,
`streamEvents`) were removed from the contract and the orchestrator on 2026-09-30 (ADR 0012); the web
never called them after its move to AG-UI.

The browser calls `/api/*` and `/agui/*` on its own origin only. In production oauth2-proxy / the
ingress routes both to the orchestrator; there are no Next.js API routes, server-side fetches or
secrets. `MOCK_API_ORIGIN` (dev and e2e only) adds rewrites to the mock server; `API_ORIGIN` (the
system e2e build) adds the same rewrites to a real orchestrator.

## The chat layer

`@assistant-ui/react-ag-ui` drives an `AbstractAgent` from `@ag-ui/client`: it calls
`agent.runAgent(input)` for a run it starts, applies the events that come back and aggregates a run
into one assistant message. Our conversation is not shaped like that (the log is server-owned, it
has several actors, and runs start without the user), so the app gives the runtime an agent that
turns one long connect stream into the runs the runtime expects.

```mermaid
sequenceDiagram
  autonumber
  actor U as User
  participant R as Runtime<br/>(react-ag-ui, patched)
  participant L as LiveRuns
  participant T as ThreadAgent
  participant O as Orchestrator
  T->>O: GET /agui/threads/t/connect (Last-Event-ID = last delivered seq)
  O-->>T: replay of the thread's runs, then live frames, id: seq at resume points
  Note over T: frames are held until an id: closes the log event, then delivered whole
  T->>L: a run nobody here started (replay, another tab, a webhook): user message + frames
  L->>R: append the user message, thread.startRun (steerAway if an interrupt is open)
  R->>T: run(input)
  T-->>R: the external run's frames (no POST)
  U->>R: types a message
  R->>T: run(input) with the new user message
  T->>O: POST /agui/agents/a {threadId, runId, one new message | resume}
  O-->>T: RUN_STARTED (accepted: the response is released)
  T-->>R: RUN_STARTED, then the run's frames from the connect stream (matched by runId)
  Note over T,O: the connection drops: reconnect with Last-Event-ID, the run goes on
  U->>T: Cancel: POST /api/threads/t/cancel
  O-->>T: RUN_FINISHED cancelled, on the connect stream
  T-->>R: the run ends incomplete/cancelled (patched: the outcome is not "complete")
```

```mermaid
stateDiagram-v2
  [*] --> Connecting: start() (a thread page)
  Connecting --> Open: 200
  Connecting --> NotFound: 404 (no retry)
  Connecting --> Reconnecting: 401, 5xx, network
  Open --> Reconnecting: the stream ends or breaks (frames after the last id: are discarded)
  Reconnecting --> Connecting: after the backoff, with Last-Event-ID
  Open --> Paused: the thread is finished and everything is loaded
  Paused --> Open: a message starts the next job (the accepted run opens the stream again)
  Paused --> [*]: stop() (unmount)
  Open --> [*]: stop() (unmount)
```

- **`ThreadAgent`** (`src/features/chat/lib/agui/thread-agent.ts`) owns the connect stream of one
  thread. It hands frames on in whole groups (a frame with an `id:` closes a group, so a cut
  connection never leaves half a message in the runtime, and the reconnect resumes at the last
  group), drops groups it has delivered, and routes a run by `runId`: a run this browser started
  goes to that `run()`, any other becomes an `ExternalRun`. `run(input)` is the POST, accepted at
  `RUN_STARTED`. `abortRun()` only detaches (the runtime calls it on unmount and on thread switches,
  and a consumer that leaves has a truncated run, not a cancelled one); `cancel()` is the cancel
  endpoint. `stop()` (unmount, or the pause once the thread is finished) ends the connect stream
  only, never a send in flight: the connect stream can deliver a run to its end, and the page pause,
  before the POST's own `RUN_STARTED` arrives, and that run's reply is already in the agent. The runtime drops an event's `metadata`, so the agent moves `vymalo.actor` into the
  activity content and a marker part.
- **`LiveRuns`** (`live-runs.ts`, mounted in the shell) applies each external run through the
  runtime's public API only: it appends the run's user message, starts a run, and `ThreadAgent.adopt`
  makes that run read the external run's frames. The reply is aggregated by the runtime's own code,
  so a replayed run, a live one and one the user started render the same way. An open interrupt
  makes the runtime refuse a run, so then it goes through `steerAway`, which closes the interrupt
  and starts it.
- **Reload is a replay.** There is no history adapter: the connect stream from the start is the
  history, replayed through the same path as live frames, which keeps every activity (see
  [`patches/UPSTREAM.md`](patches/UPSTREAM.md#observed-not-patched)).
- **A thread never locks** ([ADR 0020](../docs/decisions/0020-a-thread-is-a-conversation.md)). The
  composer is never disabled. While a run is live the box is for drafting: Enter does not send
  (`submitMode: none`), the button says **Stop** (`POST /api/threads/{id}/cancel`) and the draft survives it; once the
  run has ended the button is Send, and a message on a `done`, `failed` or `cancelled` thread is the next job's first
  word, in the same transcript. The placeholder says what fits: "Describe a task for the agent…" (new), "Reply…" (the agent
  asked), "Tell the agent how to go on…" (failed or stopped), "Send a follow-up…" (otherwise). Nothing tells the
  person to start a new thread; the `vymalo.job` marker of a later job draws nothing. A replay that holds several jobs
  applies their runs one after the other (`live-runs.ts`, `quiesce`: the runtime's transcript lags a render, and a
  run applied before the earlier one showed would hang off the wrong message).
- **Thread state** (the header pill, whether the composer shows Stop or Send) is the newest
  `STATE_SNAPSHOT.thread` the agent delivered, else `GET /api/threads/{id}`. `verifying` counts as
  active, like `working`: the run is open and Stop is offered. The snapshot also carries `job` and,
  for a run that failed, the `RUN_ERROR` code (`ThreadSnapshot.job`, `.failure`).
- **Interrupts.** The run that ended in an interrupt leaves the runtime holding it
  (`useAgUiInterrupts`); the composer shows its `message` as the question and sends the answer as
  `resume` (`steerAway` with a `resolved` entry carrying `payload.text`). Never a plain message: the
  orchestrator refuses a message together with a `resume`.
- **A send the server refuses** (a problem before the stream) is a `SendError`, assistant-ui's
  `MessageNotSentError`: the composer takes its text back, the failed message is removed from the
  transcript (`dropFailedSend`) and the problem's `detail` is shown.
- **A turn** (one run, one assistant message) is drawn by `thread.aui.tsx` as a classical chat
  ([DESIGN.md](DESIGN.md), "A turn"): the agent's avatar and name once, one **summary line** for its steps, then its
  parts in order through `MessagePrimitive.GroupedParts`. Every stretch of step parts (`lib/steps.ts`: a status but a
  failure, an artifact, `.check`, `.ci`, `.rework`, `.action`, `.step`, the actor and job markers) draws **nothing**
  in the chat: the steps are the side panel's Activity tab ([The step tree](#the-step-tree)), and the line
  (`steps/turn-summary.tsx`) opens the panel on this turn; the agent's words (`TEXT_MESSAGE_*`, including the words
  of a `completed` or `input_required` status, `st-<seq>`) are prose; a failed status and `.error` are soft callouts
  and `.a2ui-surface` the A2UI renderer (`data-uis.tsx`, see [A2UI surfaces](#a2ui-surfaces)); the
  pull requests and files the agent shared follow as cards (`cards/turn-cards.tsx`). The statuses
  that come with words (`completed`, `input_required`) draw no step: the words and the state pill
  say it. A shape a renderer does not know renders nothing.
- **Agent markdown** (`markdown-text.tsx`) is untrusted: raw HTML stays text, every link opens in
  a new tab without an opener, and an image is **never fetched**: `![alt](url)` is its alt text and,
  for an http(s) URL outside a link, a link to it (a URL can carry what the agent read).
- **Thread ids** are UUIDv7 (`src/lib/uuid.ts`): the consumer mints them, and the orchestrator lists
  threads by id, newest first.

### Dependencies and patches

Pinned exactly. *Verified 2026-09-29* on the npm registry (`npm view`):

| Package | Version | Note |
|---|---|---|
| `@assistant-ui/react-ag-ui` | 0.0.62 (2026-09-24, latest) | depends on `@ag-ui/client` `^0.0.59`, `@assistant-ui/core` `^0.3.21`, `@assistant-ui/react-generative-ui` `^0.0.21` |
| `@assistant-ui/react-generative-ui` | 0.0.21 (2026-09-24, latest) | a direct dependency and a peer of the runtime: the app uses its reducer, its converter and `renderGenerativeUI` behind its own validator ([A2UI surfaces](#a2ui-surfaces)); the runtime's own A2UI path is bypassed |
| `@ag-ui/client` | 1.0.0 (published 2026-09-17) | a direct dependency, and the `overrides` entry below; 1.0.1 was published 2026-09-29 and is not adopted (`tools/agui-conformance` reads the goldens with 1.0.0) |
| `@cfworker/json-schema` | 4.1.1 (2025-01-31, latest; MIT) | validates each instance of the UI catalog against its component's JSON Schema, draft 2020-12 ([The UI catalog](#the-ui-catalog)). *Verified 2026-10-01* on the npm registry (`npm view`) and by running it: it interprets a schema (no `eval`, no `new Function`), reads `const`, `enum`, `pattern`, `additionalProperties` and `required`, and with `shortCircuit` its error list is the chain from the root to the broken rule |
| `mermaid` | 12.0.0 (2026-09-10, latest; MIT) | draws the `Mermaid` component of the UI catalog ([Cards and Mermaid](#cards-and-mermaid)), **loaded only when a graph is drawn** (`import()` in `lib/a2ui/mermaid-render.ts`, a test keeps every other file from importing it). *Verified 2026-10-01* on the npm registry (`npm view mermaid`: version, `license` MIT, `dist-tags.latest`, `time.modified`) and in the installed package (`dist/config.type.d.ts` for the options, `dist/chunks/mermaid.core/chunk-O7XYJQB3.mjs` for the default `secure` list `["secure", "securityLevel", "startOnLoad", "maxTextSize", "suppressErrorRendering", "maxEdges"]`), and by running it (`lib/a2ui/mermaid.security.test.ts`): a graph's own `%%{init}%%` directive or front matter cannot change `securityLevel` or `maxTextSize`, and **can** change `htmlLabels` and `theme`, which is why the app adds those, `themeCSS`, `look`, `layout` and more to `secure` |
| `@tanstack/react-virtual` | 3.14.13 (2026-09-14, latest; MIT) | draws a window of the rows of a very long level of the step tree (more than 50 children of one step: a scroll box, `components/steps/step-node.tsx`); owner decision 5 of plan 03. *Verified 2026-10-01* on the npm registry (`npm view @tanstack/react-virtual`: version, `license` MIT, `dist-tags.latest`, `time.modified`) |
| `rxjs` | 7.8.1 | the version `@ag-ui/client` pins; `ThreadAgent.run` returns an `Observable` |

#### Spike S5: the runtime on `@ag-ui/client` 1.0.0

*Result (2026-09-29): it works.* `pnpm-workspace.yaml` has `overrides: "@ag-ui/client": 1.0.0`, so the
runtime and the app share one copy (`pnpm why @ag-ui/client` shows one). The runtime imports a single
value from the client (`buildResumeArray`) and otherwise only types, all present in 1.0.0; it drives
1.0.0's `runAgent` (enforcement, verification, `defaultApplyEvents`) and passes the goldens. The one
thing 1.0 adds that the runtime does not know is the `cancelled` run outcome; that is the patch.
The runtime therefore keeps no second, pre-1.0 client, and CI needs no separate check with the 1.0
reference consumer (`tools/agui-conformance` also runs, on the goldens themselves).

#### Patches

Policy: a dependency change goes through `overrides`, never a patched `package.json`; a code change
is a `pnpm patch` (`pnpm patch @assistant-ui/react-ag-ui@0.0.62`, edit, `pnpm patch-commit`), stored in
`patches/` and registered under `patchedDependencies`. Every patch has an upstream twin, drafted in
[`patches/UPSTREAM.md`](patches/UPSTREAM.md) (**nothing is filed** without the owner) and named in a
header comment of the patch file. A patch is deleted when a pinned release contains the fix; an
upgrade re-applies the patches, and one that no longer applies fails `pnpm install`, which is the
signal to look upstream. A bump is a reviewed change that re-runs the goldens and the e2e suites.

| Patch | Why | Upstream draft |
|---|---|---|
| [`@assistant-ui__react-ag-ui@0.0.62.patch`](patches/@assistant-ui__react-ag-ui@0.0.62.patch) | AG-UI 1.0's `RUN_FINISHED` outcome `cancelled` was parsed as nothing and shown as a complete run; it now ends the message `incomplete`/`cancelled` (`src/` and `dist/`) | [Run outcome `cancelled`](patches/UPSTREAM.md#run-outcome-cancelled) |

[ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md#the-web) expected patches for two
more gaps (activities dropped on reload; no live subscription). Neither is needed here, because the app never restores through `fromAgUiMessages` and applies runs it did not
start through the runtime's public API; both are drafted as upstream issues anyway
([Observed, not patched](patches/UPSTREAM.md#observed-not-patched)).

## Verification (the gate)

A thread whose agent runs under the verification gate ([ADR 0018](../docs/decisions/0018-verification-gate-and-rework-loop.md),
[the stream](../docs/api/agui.md#verification-the-gate)) is not done when the agent says `completed`: the orchestrator
checks the work, sends the agent back with the findings while attempts are left, and only then finishes. One run covers
all of it, so the transcript is one assistant message that grows through the attempts. The web draws what the stream
says and decides nothing.

```mermaid
sequenceDiagram
  autonumber
  participant O as Orchestrator
  participant T as ThreadAgent
  participant R as Runtime (react-ag-ui)
  participant V as Renderers (steps/)
  participant H as Header and composer
  O-->>T: STATE_SNAPSHOT verifying, job {attempt 1, maxAttempts 3, gate, sha}
  T->>H: state = verifying, job (the pill, "Checking the work…")
  O-->>T: ACTIVITY_SNAPSHOT vymalo.check check-1-1-agent_checks (failed, findings), replace
  T->>R: the same, with the actor folded into the content
  R->>V: the data part, replaced in place by its message id
  V->>V: parseCheck: a payload it does not know draws nothing, findings are text
  O-->>T: ACTIVITY_SNAPSHOT vymalo.rework rework-2, then SUBAGENT_STARTED, STATE_SNAPSHOT queued (attempt 2)
  R->>V: the step "Checks failed — trying again (2/3)", then the next attempt's steps
  O-->>T: a check passed, STATE_SNAPSHOT done, RUN_FINISHED success
  T->>H: state = done (the attempts are in the steps, not in the header)
  Note over O,T: out of attempts: STATE_SNAPSHOT failed, then RUN_ERROR checks_failed
  T->>H: failure = checks_failed: "Checks failed after 3 attempts"
```

```mermaid
stateDiagram-v2
  [*] --> Queued
  Queued --> Working: the agent works
  Working --> Verifying: completed, and the run stays open
  Verifying --> Queued: a check failed, attempts left (a rework step)
  Verifying --> Done: every required check passed
  Verifying --> Failed: a check failed on the last attempt (Checks failed after N attempts)
  Verifying --> Cancelled: Stop
  Done --> Queued: a message (the next job)
  Failed --> Queued: a message
  Cancelled --> Queued: a message
```

The badge follows that lifecycle. A check step has a smaller pill of its own: `pending` becomes `passed` or `failed` by
the same message id (in place, never a second step), and an answer that arrived too late is a step of its own, stale.


| Piece | Where | What it does |
|---|---|---|
| State pill | `state-badge.tsx` | One pill, in words a person uses: queued "Starting…", working "Working…", verifying "Checking the work…" (its own colour, `--verifying`, a violet that keeps 4.5:1 in both schemes; spoken "Thread state: Checking the agent's work"), blocked "Your turn" when the agent asked (an interrupt is open) and "Needs attention" otherwise, done "Done", failed "Failed", cancelled "Stopped". There is no attempt counter: attempts show inside the turn, in the rework step |
| Check step | `steps/check-step.tsx` | `vymalo.check` as a step of the turn (a root after the agent's own in the panel's [step tree](#the-step-tree)), a `listitem` named "Check: <source>, attempt <n>, <status>[, stale]": what the source does or said ("Waiting for CI", "The verifier is reviewing the work", "The agent's checks failed"; an unknown source by its own name), a pill with the status in words and an icon (Passed, Failed, Pending), the attempt, the short commit (seven hex digits, else cut to 12; the full value in `title`), the CI check `name`, the summary, and the findings folded behind "Findings (n)". The verifier is named; the orchestrator is not. One step per source in one verification of one attempt (`check-<attempt>-<verification>-<source>`), replaced in place; a `stale` answer has its own id, a muted step marked "Stale" that says it decided nothing; a pending check whose run ended says no answer came |
| Findings | `parts/findings-list.tsx` | A list of **plain text**: React text nodes, never `dangerouslySetInnerHTML`, never the markdown renderer, so `<script>`, `**bold**`, `[x](javascript:...)` and `<img onerror>` show as the characters they are. A finding over 240 characters is cut (never in the middle of a surrogate pair) with "Show more" / "Show less" (`aria-expanded`); more than five findings are folded behind "Show all N findings" |
| Rework step | `steps/step-items.tsx` | `vymalo.rework`: a warning step "Checks failed — trying again (2/3)" ("CI failed", "The review found issues" when one source sent it back) and how many findings it took back ("· 1 finding"); the findings themselves are on the failed check step above, folded. The next attempt's steps are the agent's, in the same turn |
| Checks failed | `composer.tsx` | A failed thread whose run ended in `RUN_ERROR` `checks_failed` says "Checks failed after N attempts" (a destructive notice, with no link to a new thread: write a message to go on); any other finished thread says nothing above the box |
| Parsing | `lib/agui/vymalo.ts` | `parseCheck` (needs a `source`, an `attempt` >= 1 and a status of pending, passed or failed), `parseRework` (an `attempt` and `maxAttempts`) and `parseJob` return null for anything else, ignore unknown fields, keep only string findings (at most 100 read), and treat `stale` as true only when it is `true`. `lib/findings.ts` holds the shortening |

Not in this slice: the verifier as its own subagent comes with its slice; a check of a source this UI has not heard of is
already drawn from its own name. The CI step is the next section.

### CI results (`vymalo.ci`)

A CI system's report on a commit (ADR 0017, [`docs/api/agui.md`](../docs/api/agui.md#ci-results-vymalo-ci)) is a step of
its own, **for every report**, whether or not the gate counted it; the `vymalo.check` step of the source `ci` next to
it shows what the gate made of it. The orchestrator's order is check (pending), report, check (answered), rework.

| Piece | Where | What it does |
|---|---|---|
| CI step | `steps/ci-step.tsx` | A `listitem` named "CI: <name>, <conclusion>". The conclusion as a pill: words first ("Success", "Failure", "Cancelled", "Timed out", "Neutral", "Skipped", "Action required", "Stale", "Startup failure"), an icon of its own for each, colour last (success green; failure, timed out and startup failure red; action required amber; the rest muted). A conclusion this UI does not know is shown by its own name, cut short, with a generic icon, and `passed` picks green or red. Then the check `name`, the short sha (`shortSha`, the full `sha` in `title`), the branch when there is one, the provider ("GitHub", "Generic webhook") and repository in small muted text, the `summary` and a "View run" link. Its accessible name is "CI: <name>, <conclusion>", apart from "Check: ..." |
| Text | `parts/expandable-text.tsx` | `name`, `branch` and `summary` come from whoever runs the CI: plain text nodes, never markdown or HTML, never a link. A summary over 240 characters is cut with "Show more" / "Show less" (`aria-expanded`), like a finding; a name or branch over 120 is cut with the whole in `title` |
| Link | `steps/ci-step.tsx` | "View run" is drawn only when `url` passes `safeHttpUrl` (the A2UI rule: `http:` or `https:`, no control character, no user information); any other scheme (`javascript:`, `data:`, `file:`, a relative path) draws no link. `target="_blank"` and `rel="noopener noreferrer"`. `parseCi` drops such a url and the card checks it again |
| Parsing | `lib/agui/vymalo.ts` | `parseCi` needs `name`, `conclusion`, a boolean `passed`, `sha`, `shortSha`, `provider` and `repository`, and returns null for anything else (nothing is drawn); it ignores unknown fields. `lib/ci.ts` holds the conclusions and their words |

The web never reads the step's message id: like any activity, a step is kept under the id the wire gives it (one step
per report today, and a later snapshot with the same id and `replace: true` would replace it).

## The step tree

An agent's work has a shape: the agent, a sub-agent it delegated to (OpenCode), their commands
([ADR 0025](../docs/decisions/0025-nested-steps-events-carry-their-source-path.md),
[`steps/v1`](../docs/api/steps-v1.md), [the stream](../docs/api/agui.md#nested-steps)). The conversation keeps **one
line per agent turn**, which opens the right-hand panel, and the panel's **Activity** tab is the tree
([DESIGN.md](DESIGN.md), "Steps panel"). The step parts the runtime already holds are its only source (nothing is
fetched), so the chat's line and the panel can never disagree, on the live stream, on a replay and after a reload.

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant R as Runtime (messages)
  participant B as buildTurnSteps (lib/step-tree.ts)
  participant C as Chat: TurnSummary
  participant S as Shell: useStepsPanel
  participant P as Panel: StepsPane
  R->>B: the thread's messages and the thread's state
  B-->>C: each turn's line (summaryLine)
  B-->>P: each turn's tree
  U->>C: click the line of turn 3
  C->>S: openSteps(turn id): opens the panel, the Activity tab, focus {turn id, key + 1}
  S->>P: focus
  P->>P: open turn 3, scroll to it, focus its header (once per key)
  U->>P: a click on a step with children
  P->>S: the expansion state (above the panel, so closing the panel keeps it)
  Note over R,P: a new frame changes one message: its turn is rebuilt, the others are the same objects
```

```mermaid
stateDiagram-v2
  [*] --> Collapsed: a step with children
  Collapsed --> Latest3: click (the latest three and every failed one)
  Latest3 --> Latest13: Show 10 more
  Latest13 --> Latest23: Show 10 more
  Latest23 --> ScrollBox: shown rows past 50 (a window of the rows)
  Latest3 --> Collapsed: click
  Latest13 --> Collapsed: click
  Latest23 --> Collapsed: click
  ScrollBox --> Collapsed: click
  Collapsed --> [*]: the panel is closed, the state is kept above it
```

- **`buildTurnSteps(messages, view)`** (`lib/step-tree.ts`, pure) makes one tree per agent turn: the turn's agent as
  the root, holding in order of first appearance today's statuses (a status whose words are `$ …` is a command),
  artifacts and actions as leaves and every `vymalo.step` placed by its `path` (a step whose parent is not in the
  turn goes under the agent; a step that says itself again is updated in place), then the gate's checks, CI reports
  and reworks as roots of their own. A turn that is not running holds no running step (paused on a question, its
  steps wait; ended, they are stopped); a turn that ended on a question stays paused in the history. A turn is
  numbered as `panel/lib/sources.ts` numbers turns, and built once per message that changed (the runtime keeps a
  message that did not change as the same object, a test holds it to that), so a long thread costs the new frame
  and not the whole thread.
- **`summaryLine(turn)`** is the one line the chat keeps ("Running npm test · 14 steps", "Paused · 9 steps",
  "Verifying", "14 steps · 2m 10s", "Failed · 14 steps", "Stopped · 5 steps", and a "1 failed" chip even in a turn
  that went well; none for a turn of words that is not running). `TurnSummary` draws it as a button whose name says
  it all and which opens the panel on the turn (`TurnSummaryLine` wires it to `useStepsPanel().openSteps`).
- **`StepsPane`** (prop-driven: the turns, the shell's focus request, whether the thread runs, the expansion state)
  lists the turns that did something, one section each, opened one at a time (the one asked for, else the live one,
  else the last); inside a turn each step with children is one line a click opens to its latest three (and every
  failed one), "Show 10 more" lists ten earlier ones, a failure chip repeats at every collapsed level, and a level
  of more than 50 rows is a scroll box that draws only the rows in view (`@tanstack/react-virtual`). While the thread
  runs it keeps the step the agent is on in view until the person scrolls or chooses a turn. The leaves of today's
  activities are drawn by the renderers that always drew them (`step-items.tsx`, `check-step.tsx`, `ci-step.tsx`).
- **`StepsPanelContent`** is the connected form the panel's Activity tab renders: `useTurnSteps()` over the runtime,
  `useStepsPanel()` (the panel's contract: its id, whether it shows, the tab, the focus request, `openSteps`) and
  `useStepsExpansion()`. It must sit inside the thread page's `AssistantRuntimeProvider` and `ThreadViewProvider`.

## A2UI surfaces

An agent's interface (ADR 0013: A2UI end to end) reaches the page as an `a2ui-surface` activity whose
`content.a2ui_operations` are the operations of ONE surface, the whole surface in every snapshot
(`replace: true`, message id `a2ui-<seq>`). The surface is **untrusted input from an agent**: the
validator is the only thing between its JSON and the DOM, and it refuses the whole surface or draws it
whole.

```mermaid
sequenceDiagram
  autonumber
  actor U as Owner
  participant T as ThreadAgent
  participant R as Runtime (react-ag-ui)
  participant V as prepareSurface (validator)
  participant S as SurfaceView (shadcn vocabulary)
  participant H as SurfaceHostProvider
  participant O as Orchestrator
  O-->>T: ACTIVITY_SNAPSHOT a2ui-surface (whole surface, replace)
  T->>R: the same, as activity vymalo.a2ui-surface (content untouched, actor and surface id added)
  Note over R: a data part: the runtime converts nothing
  R->>V: a2ui_operations
  alt refused
    V-->>S: rule and reason
    S->>U: one error line, the reason, the raw operations as text
  else accepted
    V-->>S: spec (converted after the checks)
    S->>U: the surface, labelled by vymalo.actor only
    U->>S: click on a Button (a user gesture)
    S->>H: send(action) (canSend: thread blocked, none in flight)
    alt the agent's question (an interrupt) is open
      H->>T: stageA2uiAction(action)
      H->>R: submit the interrupt as cancelled (starts the run)
    else no interrupt
      H->>R: sendA2uiAction(action) (the runtime's own hook)
    end
    R->>T: run(input)
    T->>O: POST /agui/agents/{id}: no message, no resume, forwardedProps.a2uiAction.userAction
    O-->>T: RUN_STARTED, then the run on the connect stream
  end
```

```mermaid
stateDiagram-v2
  [*] --> Received: a2ui-surface activity
  Received --> Pending: no root yet, or a component not sent yet
  Received --> Refused: a limit, the vocabulary, a URL, an action or a function value
  Received --> Deleted: deleteSurface
  Received --> Drawn: every rule holds
  Pending --> Received: the next snapshot
  Drawn --> Received: the next snapshot replaces it in place
  Drawn --> Refused: a component throws (error boundary)
  Drawn --> Superseded: a later run holds a newer copy
  Drawn --> Inert: the thread is not blocked, or is finished
  Inert --> Drawn: the thread blocks again
  Refused --> Received: the next snapshot in the same run
  Refused --> [*]
  Deleted --> [*]
  Superseded --> [*]
```

**Why the app renames the activity.** `@assistant-ui/react-ag-ui` has an A2UI path of its own (an
`a2ui-surface` becomes a `present` tool part). It converts the surface in the run aggregator the moment
it arrives, so no host check can come first; it drops operations that say `v0.9.1`, the version our
orchestrator relays; and it has no `openUrl`. `ThreadAgent` therefore hands every `a2ui-surface` to the
runtime as the activity `vymalo.a2ui-surface` (an ordinary data part, drawn by `data-uis.tsx`), with the
content untouched plus the actor and the surface id. Nothing converts before the validator has run.
The library's own pure pieces are used after it: `applyA2uiOperations` (reducer),
`convertSurfaceToUISpec` and `renderGenerativeUI`.

*Verified 2026-09-29* (read from the installed `@assistant-ui/react-generative-ui` 0.0.21 and
`@assistant-ui/react-ag-ui` 0.0.62 sources, and run): the reducer accepts `v0.9` and `v1.0` only
(`isVersion`), so `v0.9.1` is rejected with a warning; the converter maps 13 basic-catalog components
to the library's IR, bounds itself at depth 32, 100 template items and 5000 nodes, reads template
children only as `{template: {componentId, path}}`, drops a `functionCall` action and an event's
`userMessage`, and evaluates no function value; `sendA2uiAction` throws while an interrupt is open
(`assertNoPendingInterrupts`) and otherwise runs `startResumeRun` with `forwardedProps.a2uiAction.userAction`
(`type` stripped, `timestamp` added). The A2UI page on assistant-ui.com describes more than that
(`openUrl` as `a2ui:functionCall`, `$field` references, `userMessage`, `{componentId, path}`
templates) and appears to describe the repository head, not 0.0.21. Drafts for upstream, none filed:
[`patches/UPSTREAM.md`](patches/UPSTREAM.md#a2ui-v091-operations-are-dropped-and-the-built-in-path-converts-before-any-host-check).

### The validator

`src/features/chat/lib/a2ui/prepare.ts`, pure TypeScript (no React, no I/O), unit-tested next to it.
`prepareSurface(operations, {catalog?, threadVersion?})` returns `surface` (what to draw), `pending` (valid so far, nothing to
draw yet: no `root`, or a child not sent yet), `deleted`, `newer` (a component of a newer catalog: see
[The UI catalog](#the-ui-catalog)), or `refused` with the rule and the reason,
never part of a surface. The checks run in this order, cheapest first, and the library's reducer and
converter only ever see operations that passed the ones before them:

| Rule (`refused.rule`) | Limit or requirement |
|---|---|
| `size` | at most **64 KiB** (65,536 bytes of UTF-8) of serialised operations; one byte more is refused |
| `shape`, `version`, `surfaces` | an array of objects, each with a `version` (`v0.9`, `v0.9.1`, `v1.0`; `v0.9.1` is read as `v0.9`) and exactly one operation (`createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`) with a surface id of 1 to 256 bytes; one surface per activity; anything the reducer had to skip (a component without an id, an update of a surface never created) refuses the surface |
| `catalog`, `schema` | the surface's catalog and, under ours, each instance's schema: see [The UI catalog](#the-ui-catalog) |
| `components` | at most **400** components once the operations are applied (unreferenced ones count) |
| `vocabulary` | under the basic catalog, only the components below; an unknown one refuses the surface wherever it is (never skipped) |
| `function` | a function value (`{call, ...}`, such as `formatString`) refuses: the pinned converter cannot run it |
| `action` | a Button needs an action; an event needs a name of 1 to 256 bytes that does not start with `vymalo:` (reserved for what the app lowers) and a context that is an object; a `userMessage` is text of 1 to 4000 characters; a function call other than `openUrl` is ignored (a button that does nothing) |
| `url` | see the links rule below |
| `field` | an input is bound to an absolute path, and is not inside a template |
| `depth` | at most **24** levels (the root is level 1; a template item is one level below its container) |
| `cycle` | a component that contains itself, directly or not |
| `template` | a template reads a list, of at most **100** items (more is refused, not cut off) |
| `expansion` | at most **2000** nodes **after** references and templates are expanded (a node is a component drawn, and one wrapper per template item). The walk stops at the limit, so a bomb costs 2000 steps, not its size |

Both bombs are tested (a 100 x 100 nested template, and a 24-level chain in which each level names the
next twice: 2^23 nodes), and each is refused in a few milliseconds. The limits are the ones of ADR 0013;
each is tested exactly at the limit (accepted) and one over (refused). Because the limits are lower than
the converter's own (5000 nodes, depth 32), the converter never truncates anything silently.

**Vocabulary** (`GenerativeUILibrary`, the format `JSONGenerativeUI({ library })` takes; drawn with
`renderGenerativeUI`, the function its `present` tool renders with, because no model runs in the browser):
`Text`, `Image`, `Row`, `Column`, `List`, `Card`, `Divider`, `Button`, `TextField`, `CheckBox`. Not
drawn, so they refuse the surface: `Icon`, `Tabs`, `Modal`, `Slider`, `DateTimeInput`, `ChoicePicker`
and the media components.

| Component | Drawn as |
|---|---|
| `Text` | a heading (`h1` to `h6`), a caption, or a paragraph of **plain text** (no markdown, no HTML: `<script>` shows as characters, and nothing in text becomes a link or an image) |
| `Image` | a placeholder with the alt text ("Image not shown: ..."); **never fetched** (no `<img>`, no request) |
| `Row`, `Column`, `List`, `Divider` | flex rows and columns, a list, a separator |
| `Card` | the shadcn `Card` |
| `Button` | the shadcn `Button` (`primary`, `borderless` and default variants) |
| `TextField`, `CheckBox` | a labelled input; the values stay in the surface until a Button sends them |

**Links.** `safeHttpUrl` (`lib/a2ui/url.ts`): the text must start with `http://` or `https://` (any case:
the result is the normalised `href`), contain no control character or space anywhere, no backslash, no
user information, and have a host. So `javascript:`, `data:`, `file:`, `blob:`, `vbscript:`, `mailto:`,
relative paths, `//host`, `http:host`, `java\tscript:` and a leading space are all refused, in every case.
It applies to `openUrl`'s URL (which must be a literal) and to every `url`, `href`, `src`, `uri`, `link`,
`iconUrl` and `imageUrl` prop of a component (read from the data model too, item by item in a template).
A refused URL refuses the surface. An `openUrl` button is drawn as a plain link
(`target="_blank" rel="noopener noreferrer"`), and, because it does not talk to the agent, it stays
usable on a finished thread.

**Labels.** A surface is labelled by the orchestrator's `vymalo.actor` and nothing else: the theme's
`agentDisplayName` and `iconUrl` are never read (the tests search the DOM for them).

**Actions** (ADR 0013 rule 7):

- A control acts only in its own click handler: rendering, an update, a data-model change, a timer and
  typing (Enter in a field included) never send anything. The tests advance a minute of fake timers and
  count the requests.
- A Button whose action is an `event` sends `forwardedProps.a2uiAction.userAction`
  (`name`, `surfaceId`, `sourceComponentId`, `context`) as a run with **no message and no `resume`**, only
  while the thread is `blocked` and no action is in flight (`SurfaceHostProvider` in
  `components/surface/surface-host.tsx`); otherwise the button is disabled and the surface says once why.
  A binding to an input in the context is replaced by the input's value at the click. A context over
  16 KiB is not sent (the orchestrator would answer 413).
- With the agent's question open (an interrupt, the usual case) the runtime refuses `sendA2uiAction`, so
  the app closes the interrupt through the runtime and stages the action on `ThreadAgent`, which sends
  it instead of the `resume` (`stageA2uiAction`). With no interrupt it is the runtime's own
  `useAgUiSendA2uiAction`. A refused action (`409`, `422`, `413`) is shown in the composer's error line;
  it carried no message, so nothing is taken back from the transcript, and the button works again.
- A `userMessage` puts its text in the message box, focused and **unsent**, and sends nothing (it is not
  posted as the owner's message, and no action goes with it); it needs a message box, so it is off on a
  finished thread.
- Only the **newest copy** of a surface is live: an update in a later run leaves the earlier message with
  "This interface was updated further down.", and its buttons are gone.
- A surface that fails while it draws is caught by an error boundary and shows the refusal line. The
  boundary guards one spec: a snapshot that replaces the surface in the same run gets a fresh try.

A refusal is one line in the `vymalo.error` style: "Interface not shown: <reason>", the rule
(`data-rule`), and the raw operations (first 4000 characters) in a disclosure, as text.

The mock plays the orchestrator's `ui` story (`mock/scripts.ts`): `ui pick one` sends a surface (a
title and a `Go` button, in two payloads, `v0.9.1`), asks "Pick one" and blocks; the action resumes it to
`answered: ui-action go`. `mock/golden.test.ts` requires its stream to be `a2ui.agui.json`, frame for
frame, and its action route refuses what the orchestrator refuses (`422` malformed, unknown surface, an
action with a message; `413` over the sizes; `409` a run open or the thread finished).

### The UI catalog

[ADR 0023](../docs/decisions/0023-ui-component-catalog-as-an-a2a-extension.md), contract
[`docs/api/ui-catalog-v1.md`](../docs/api/ui-catalog-v1.md). The web **defines** the components an agent may name
beyond the basic vocabulary, tells every thread about them once, and refuses visibly what it cannot draw.

| Piece | Where | What it is |
|---|---|---|
| The catalog | `src/features/chat/lib/a2ui/catalog/catalog.json` | An A2UI inline catalog, `{catalogId, components}`: one JSON Schema (draft 2020-12, self-contained, `additionalProperties: false`, values are literals) per component, validating the whole instance. `catalogId` is `https://agents.vymalo.com/a2ui/catalogs/chat`. Version 1 is `Text` and `Column`, version 2 adds `Choices`, version 3 adds `Cards` and `Mermaid` |
| The lock | `catalog.lock.json` | `{version, digest}`. **The version is bumped by hand** and orders the catalogs a thread has seen (the highest is the newest); the digest is `sha256:` of the canonical JSON (`digest.ts`: sorted keys, no whitespace, integers only, ASCII keys only; the contract pins a known-answer vector, and `catalog.test.ts` checks it). The app ships the lock's digest and never computes one |
| The script | `pnpm catalog:lock [<version>]` | Rewrites the lock. It refuses to put a new digest under the version it already had: change `catalog.json`, then `pnpm catalog:lock <next version>`, then add the pair to `RELEASED` in `catalog.test.ts` |
| The tests | `catalog/catalog.test.ts` | The lock is the digest of `catalog.json` (a change without a new lock fails); the lock's pair is in `RELEASED` (a digest cannot change under a version that shipped); the document obeys the orchestrator's rules (an inline catalog only, no `$ref`/`$id`/`$anchor`/`$schema`, at most 64 components and 64 KiB, names like `Choices`); the send rule |
| The validator | `catalog/validate.ts` | `@cfworker/json-schema`, each component schema compiled once at module load |

```mermaid
sequenceDiagram
  autonumber
  participant T as ThreadAgent
  participant O as Orchestrator
  participant P as prepareSurface
  participant V as Renderer
  O-->>T: STATE_SNAPSHOT thread.uiCatalog {catalogId, version, digest} (absent: the thread has none)
  Note over T: a run is sent
  alt no catalog on the thread, or ours is newer, or same version and another digest
    T->>O: forwardedProps["vymalo.uiCatalog"] = {catalogId, version, digest, catalog} (merged with the action, release or gate)
  else the thread has this digest, or a newer version (an older build)
    T->>O: nothing: an older build cannot move a thread back
  end
  O-->>P: a surface naming our catalog, with threadVersion from the snapshot
  alt every component is in our catalog and fits its schema
    P-->>V: drawn
  else a component this build lacks, and the thread's version is above ours
    P-->>V: newer: "This part of the answer needs a newer version of the app." and Reload
  else it lacks one, or breaks a schema
    P-->>V: refused (rule catalog or schema), the reason and the raw operations
  end
```

```mermaid
stateDiagram-v2
  [*] --> Named: the first createSurface names a catalog
  Named --> Basic: no id, or a basic id: the vocabulary of ten
  Named --> Ours: the id of our catalog
  Named --> Refused: any other id, an id that is not text, or two catalogs in one surface (rule catalog)
  Ours --> Newer: a component we lack, thread version above ours
  Ours --> Refused: a component we lack otherwise (catalog), or a broken schema (schema)
  Ours --> Drawn: every instance fits
  Basic --> Drawn: today's rules
  Basic --> Refused: a component of ours (catalog) or outside the vocabulary
  Newer --> [*]
  Drawn --> [*]
  Refused --> [*]
```

- **What is sent.** `ThreadAgent` keeps `STATE_SNAPSHOT.thread.uiCatalog` (`ThreadSnapshot.uiCatalog`; a snapshot without
  one clears it, like `job`) and merges `"vymalo.uiCatalog": OWN_CATALOG` into `forwardedProps` when
  `shouldSendCatalog` says so (`catalog/index.ts`): the thread has none (a new thread has no snapshot yet, so the run that
  creates it carries the catalog), or our version is higher, or the version is the same and the digest differs. An older
  build sends nothing. The orchestrator records a digest once, so a run that sends it needlessly costs bytes only.
- **Which catalog a surface names.** Every `createSurface.catalogId` of the surface; none is the basic catalog, as
  before. The three spellings of the basic catalog's id (`BASIC_CATALOG_IDS`) and no id are the basic catalog and
  follow the rules above; our `catalogId` is ours; **any other id refuses the surface** (rule `catalog`: "names the catalog
  ... which this app does not have"), and so does a surface that names two (a later `createSurface` starts it over).
- **Ours.** The allowed components are the keys of the catalog, not the vocabulary of ten. Each instance
  is validated against its schema and a violation refuses the surface (rule `schema`; "component "t" (Text) text:
  String is too long (4001 > 4000)": the id, the component and the first rule broken, cut and without control
  characters). A component of ours named under a basic id is refused (rule `catalog`).
- **A component this build lacks** comes before every other check. If the thread's catalog is newer than ours
  (`threadVersion > catalog.version`, from the snapshot, through `ThreadView.catalogVersion`), the whole surface is not an
  error of the agent's: `prepareSurface` returns `newer` and `SurfaceActivity` draws `SurfaceNewer`, a quiet bordered
  note ("This part of the answer needs a newer version of the app.", the component's name, **Reload**), never a part of
  the surface. Otherwise it is refused (rule `catalog`). A thread whose catalog is newer does not excuse a surface of the
  basic catalog.
- **`Choices`** (version 2, below) is the first component that is not a basic one: after its schema and the two rules a schema
  cannot say (question ids unique in a Choices, option values unique in a question; rule `schema`, and an action name that is
  not `vymalo:`-prefixed or over 256 bytes, rule `action`) the validator lowers it to `vymalo.Choices` with its own `componentId`,
  because the converter keeps an unknown component's properties but not its id (*verified 2026-10-01* against
  `@assistant-ui/react-generative-ui` 0.0.21 `convert.js`: with `keepUnknownComponents` an unknown component is kept as
  `{$type: <name>, ...props}`, and a test pins it). It counts as an event action, so the surface says once why it is off.
- **`Cards` and `Mermaid`** (version 3, [below](#cards-and-mermaid)) are output only. After their schemas they are lowered to
  `vymalo.Cards` and `vymalo.Mermaid` (no id needed: nothing is sent from them, so they are not event actions and the surface
  does not say its actions are off). A card's link must also pass `safeHttpUrl`, one level below where the walk looks
  (rule `url`: the schema only checks how a URL starts).
- **Not handled:** a component we know whose schema a newer build widened is a schema violation here (refused with its
  reason), because the web cannot tell it from the agent's mistake.

### Choices and the person's answers

`Choices` (catalog version 2, `components/surface/choices.tsx`; contract
[`ui-catalog-v1.md`](../docs/api/ui-catalog-v1.md#1-the-catalog)) asks up to 8 questions at once, each with 2 to 8 options,
one answer for each, **one action for all of them**.

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant C as Choices
  participant H as SurfaceHostProvider
  participant T as ThreadAgent
  participant O as Orchestrator
  participant B as AnswerBubble
  O-->>C: a surface of our catalog: Choices (3 questions), the thread blocks
  U->>C: choose (click the tile, arrows, space), or type in an "Other" box
  Note over C: nothing is sent, and Send waits for every required question
  U->>C: click "Send answers"
  C->>H: send({name: action.event.name or "answer", surfaceId, sourceComponentId: the Choices' id, context: {answers}})
  H->>T: stage the action, answer the interrupt (as for a Button)
  T->>O: POST run: no message, no resume, forwardedProps.a2uiAction.userAction
  O-->>T: RUN_STARTED, ACTIVITY_SNAPSHOT vymalo.action (the same context), the agent's reply
  T-->>B: the action part: context.answers has the shape of an answer
  B->>B: resolve the labels from the surface the action names (newest copy before it), else show the raw ids
  B-->>U: "Your answers" bubble above the agent's turn, not a step
```

```mermaid
stateDiagram-v2
  [*] --> Waiting: drawn, the thread is not blocked yet
  Waiting --> Open: the thread blocks (canSend)
  Open --> Open: choose, type
  Open --> Ready: every required question answered
  Ready --> Open: a choice is taken back
  Ready --> Sent: click Send (one action)
  Sent --> Open: the server refused it (the choices are kept)
  Sent --> ReadOnly: the thread moves on
  Open --> ReadOnly: a newer copy of the surface, or the thread finished
  ReadOnly --> [*]
```

| Piece | What it does |
|---|---|
| Controls | A `fieldset` per question named by its `legend` (`aria-labelledby`), role `radiogroup` for one choice and a plain group for several; native radio buttons and check boxes in tiles, the label's `::after` makes the whole tile the click target, the description is `aria-describedby`. "Other" is a choice of its own with a text box (`maxLength` 500, `aria-label` "Your own answer to: <question>"); typing turns it on, choosing a radio option turns it off (the words stay in the box and are not sent). No `<form>`: Enter sends nothing |
| Gating | Send is off until every question that is not `required: false` has an option, or an "Other" with words; the line beside it says how many are left (`aria-describedby` of the button). Inputs and the button are off unless the surface is the newest copy and the thread is `blocked` with no action in flight; what was chosen stays |
| The answer | `lib/a2ui/choices.ts` (pure): `answers` in question order, one per question, `values` in the order of the options, only values the question has, `other` trimmed and only when "Other" is on and not empty (at most 500 characters); sent with `name` = the component's `action.event.name` or `answer`, the Choices' id as `sourceComponentId`. A context over 16 KiB is not sent (it cannot happen with the limits: 8 x (8 x 64 + 500) bytes). State is a `Map`, because a question id may be `__proto__` |
| The bubble | `components/answer-bubble.tsx`, drawn by `AssistantMessage` **above the agent's mark** for a `vymalo.action` whose `context.answers` has the shape of an answer (`parseAnswers`, `isAnswerPart`); such an action is not a step (`lib/steps.ts`), any other action still is ("Chose go"). Labels are resolved from the newest copy of the surface the action names that precedes it in the transcript (`surfaceOperations`, `questionsOf`); a question or value it does not have, or a surface the transcript no longer holds, shows the id and the raw value. Every string is text. Skipped optional question: "No answer" |

The mock plays it: `choices` (see the table below) and its resume echoes `ui-action answer db=pg auth=none deploy=k8s,compose`.
The real orchestrator's fake agent (`orch-fake-agent`) has the same `choices` script, and `e2e-system/choices.spec.ts` runs it.

### Cards and Mermaid

Two output components of catalog version 3 ([`ui-catalog-v1.md`](../docs/api/ui-catalog-v1.md#version-3-slice-4-version-2-plus-cards-and-mermaid)),
for what an agent shows rather than asks: `Cards` (`components/surface/cards.tsx`, `lib/a2ui/cards.ts`) and `Mermaid`
(`components/surface/mermaid.tsx`, `lib/a2ui/mermaid.ts`, `lib/a2ui/mermaid-render.ts`). Neither sends anything: they are
not actions, hold no state the agent could read, and stay drawn on a finished thread. They sit in a surface beside text
(`Text`, `Column`) and beside each other: one answer can be words, cards and a graph (the mock's `cards-mermaid`).

```mermaid
sequenceDiagram
  autonumber
  participant P as prepareSurface
  participant V as SurfaceView
  participant M as MermaidDiagram
  participant R as mermaid-render
  participant L as mermaid (lazy chunk)
  participant I as img
  P->>P: schema of Cards and Mermaid, the links of the cards (rule url)
  P-->>V: vymalo.Cards, vymalo.Mermaid (lowered, whole)
  V->>M: code, title, caption
  M->>M: read the page's colour tokens and scheme
  M->>R: renderMermaid(code, config)
  R->>L: import("mermaid") the first time, then initialize + render, one call at a time
  L-->>R: SVG string (securityLevel strict, no HTML labels)
  R-->>M: svg
  M->>M: svgImage: parsed as a tree, refused for a script, a handler or CSS that loads, written as XML with the viewBox's size
  alt a picture
    M->>I: src = data:image/svg+xml, alt = title, caption, the source in a disclosure
  else a parse error, a refused SVG, a chunk that did not load, an image the browser cannot show
    M->>M: "The graph could not be drawn.", mermaid's first lines, the source in place
  end
```

```mermaid
stateDiagram-v2
  [*] --> Drawing: the surface passed the validator
  Drawing --> Drawn: an SVG that is a picture
  Drawing --> Failed: a parse error, a refused SVG, the library did not load
  Drawn --> Drawn: the scheme changes (drawn again, the old picture stays until the new one is ready)
  Drawn --> Failed: the browser cannot show the image
  Drawn --> Drawing: a later copy of the surface with another source
  Failed --> Drawing: a later copy with another source
  Drawn --> [*]
  Failed --> [*]
```

| Piece | What it does |
|---|---|
| `Cards` | `layout: "list"` (default) or `"grid"` (two columns from `sm`), at most 24 cards. A card is a 12 px-radius, 1 px `--border` tile: the **title** (14 px, weight 500; the link when the card has one, in `--brand`), the optional **subtitle** (12 px, muted), the **body** (14 px, line breaks kept), then the **tags** as chips and, on the right, the **host** the link goes to with an external-link icon. Every string is React text. **Nothing is fetched for a card**: no image (the schema has no image property: a card that names one breaks it), no favicon, no preview; the host is text. A link is a plain `<a href target="_blank" rel="noopener noreferrer">` to a URL checked by the validator and again by the card (`safeHttpUrl`), and its name says "(opens in a new tab)" |
| `Mermaid` | A figure: the **title** as a heading, the **picture**, the **caption**, and a closed disclosure "Diagram source" with the graph as text (the picture's text alternative; its `alt` is the title, else the caption, else "Diagram"). The picture is an `<img>` whose `src` is a data URL of the SVG mermaid drew: **never markup of the page**, so nothing in a graph can run a script, follow a link or load anything. mermaid is imported when a graph is first drawn, with `import()`, in chunks of its own: the code every thread loads does not grow (the build's first-load size is the same as without it) |
| Security options | `securityLevel: "strict"`; `htmlLabels: false`; `maxTextSize` 20,000 (the schema's limit); `suppressErrorRendering` (mermaid leaves no error drawing in the page); `startOnLoad: false`. A graph's own directive (`%%{init}%%`) or front matter cannot change them, nor the theme, `themeVariables`, `themeCSS`, the font, the look or the layout, because the app adds them to mermaid's `secure` list (`SECURE_KEYS`; a test requires it to hold everything mermaid's own list holds, so an upgrade that adds a key is caught). The tests run the pinned mermaid against hostile directives and front matter, and a browser test draws a graph that tries a script, an `onerror`, a click handler and a link |
| The picture | `svgImage` (`lib/a2ui/mermaid.ts`) parses mermaid's string as HTML (it was written for `innerHTML`: an image is parsed as XML, where `&nbsp;` is an error) and writes it out as XML. It refuses a script, a `foreignObject`, a frame, an embed, a stylesheet link, an event-handler attribute and CSS that loads (`@import`, a `url()` that is not `#id`); drops a link's `href` that leaves the picture; replaces the root's `width`, `height` and `style` by the size of the `viewBox`. All of that is a second line: an image runs nothing anyway |
| Colours | Read from the page's tokens when drawing (`--card`, `--foreground`, `--muted`, `--muted-foreground`, `--input`, `--border`, `--brand`, `--sidebar…`: DESIGN.md "Colour") into mermaid's `base` theme, `darkMode` set by `prefers-color-scheme`; a change of scheme draws again over the old picture. An image cannot use the page's CSS variables or its font, so the colours are values and the font is the system's (`ui-sans-serif, system-ui…`) |
| A graph that cannot be drawn | A destructive-toned callout in the surface (not an `alert`: a replay must not announce it again): "The graph could not be drawn.", the first three lines of mermaid's message (the place, the line, a caret) and the source, scrollable and focusable. It is the graph's failure, not the surface's: the cards and text beside it are drawn. An oversize source, a wrong property or a graph under the basic catalog is the schema's: the whole surface is refused as for any component (rule `schema` or `catalog`) |

Rendering in an `<img>` was checked in Chromium for ten kinds (flowchart, sequence, class, state, entity relationship, Gantt, pie,
mind map, timeline, git graph: `e2e/cards.spec.ts`, the mock's `mermaid-kinds`); `neo`, mermaid 12's default look, draws drop
shadows, so the app asks for `classic`. A future Content-Security-Policy ([ADR 0013](../docs/decisions/0013-a2ui-generative-ui.md), open
question 38) must allow `img-src data:` for it, or this component moves to `blob:` URLs.

## Layout

Every file name is kebab-case (`pnpm check` fails otherwise). Tests sit next to the code they test.

```
src/app/                       routes only: thin pages that compose features
src/components/ui/             shadcn primitives (generated by the shadcn CLI, then formatted)
src/components/brand/          the panda (`PandaMark`) and an agent's avatar (`AgentAvatar`), and the tests of the brand files
src/components/assistant-ui/   assistant-ui registry items, pruned to what a run's parts need
src/components/inline-status.tsx   shared empty, loading and error lines
src/features/chat/             the conversation: components (shell, top bar, composer, LiveRuns,
                               steps/ the step tree of the side panel (the check and CI steps among its
                               leaves): turn-summary (a turn's one line in the chat),
                               steps-pane (the turns, opened one at a time), step-node (a step, its
                               children, the scroll box of a long level), steps-panel-content (the
                               connected form), expansion (what the person opened),
                               cards/ the pull request and file cards, parts/ callouts and text,
                               surface/ the A2UI renderer), hooks (runtime,
                               thread details, use-turn-steps: the thread's turns and their trees),
                               lib/agui (ThreadAgent, SSE reader, live runs, the vymalo vocabulary),
                               lib/steps.ts (what a turn draws as a step, a card or prose),
                               lib/step-tree.ts (the messages in, one tree per agent turn out: the
                               summary, the line, which children a level lists),
                               lib/findings.ts (shortening untrusted text),
                               lib/a2ui (the validator: limits, URLs, pointers, preparing a surface;
                               catalog/ the UI catalog, its lock and its schemas; choices.ts, cards.ts,
                               mermaid.ts the pure parts of its components, mermaid-render.ts the only
                               file that imports mermaid)
src/features/panel/            the right-hand panel of a thread (DESIGN.md "Panel"): hooks/use-panel.tsx (open or
                               closed, tab, width, the Ctrl/⌘+Shift+. shortcut; remembered per browser),
                               hooks/use-steps-panel.tsx (`useStepsPanel()`, the contract the step tree plugs into),
                               hooks/use-sources.ts; components: thread-panel (docked `<aside>` with
                               its resize separator, or a sheet), panel-body (the tabs), sources-tab,
                               activity-tab (the step tree, `StepsPanelContent`), panel-toggle (the header's
                               button); lib/sources.ts (what the agents shared: pull requests, branches, CI
                               reports, files and the links in their words, from the messages the runtime holds),
                               lib/panel-state.ts (layout rules, limits, storage keys and the head script that
                               marks `<html>` before the first paint)
src/features/threads/          thread list: collapsible sidebar (desktop) and sheet (phone), grouped by
                               recency (lib/recency.ts, local calendar days of `updatedAt`), paging
                               hook; lib/sidebar-state.ts: the remembered open or closed sidebar and
                               the head script that hides a closed one before the first paint.
                               Paging goes by creation (`before=<id>`, UUIDv7) while the groups
                               go by the last change, so an old thread touched today can sit under
                               Today on a later page: known, and left as is
src/features/agents/           the new chat: greeting, suggestion chips; the agent picker (agent-menu.tsx, a menu
                               button in the top bar: the agents, then the chosen agent's releases; on a thread it
                               offers the other agents as a new chat); agents hook; lib/selection.ts (which agent
                               and release a new chat sends to, `?agent=`)
src/lib/                       api client and types (schema.d.ts is generated, never committed), uuidv7
public/brand/                  the panda (`panda.svg`) and the manifest icons; src/app/{icon.svg,favicon.ico,apple-icon.png,manifest.ts} are the tab, iOS and install icons
scripts/                       `brand-icons.mjs`: regenerates the icons from the panda
patches/                       pnpm patches of dependencies, and the drafts of their upstream twins
e2e/, e2e-system/, mock/       Playwright suites (mock / real orchestrator) and the mock orchestrator
scripts/                       `pnpm catalog:lock` (the UI catalog's lock)
```

The look is [DESIGN.md](DESIGN.md), the brief with the references it is drawn from; the theme is
`src/app/globals.css`: its tokens (light, and dark under `prefers-color-scheme`) mapped onto
shadcn's names, Inter self-hosted (`@fontsource-variable/inter`). There is no theme toggle.
The brand (the panda, the wordmark, the agent avatar, the colours) is DESIGN.md's "Brand". The
icons are generated from one drawing, `public/brand/panda.svg`: after changing it, run
`pnpm brand:icons`, which renders `src/app/icon.svg`, `src/app/favicon.ico`,
`src/app/apple-icon.png` and `public/brand/icon-{192,512,maskable-512}.png` with the Chromium that
Playwright already pins (no other tool needed), and commit the result.
`pnpm screens` writes a screenshot of every state, desktop and phone, light and dark, to
[`e2e/__screens__/`](e2e/__screens__/) (`e2e/screens.spec.ts`); look at them after a visual change. To add a primitive
(the CLI is pinned, see `components.json`; run `pnpm format` afterwards):

```sh
pnpm dlx shadcn@4.21.0 add <name>
```

The CLI writes `from "cn"` imports in generated files; change them to `@/lib/utils`.
End-to-end tests use the shared locators in `e2e/helpers.ts` (roles and accessible names first).

## Scripts

```sh
pnpm install
pnpm dev:mock      # mock contract server on :4010 + the app on :3000
pnpm dev           # the app only; put something that serves /api/* in front of it
pnpm check         # Biome (lint + format), CI mode
pnpm typecheck     # generated types + tsc
pnpm test          # vitest: SSE reader, ThreadAgent, the goldens through the runtime, the app in jsdom, the mock
pnpm build         # production build (standalone)
pnpm test:e2e      # Playwright + axe + Lighthouse (>= 95 accessibility) against the mock orchestrator
pnpm test:e2e:system   # Playwright against the REAL orchestrator, see "System tests"
pnpm catalog:lock [<version>]   # rewrite the UI catalog's lock after a change (see "The UI catalog")
pnpm screens       # the screenshots of e2e/__screens__/ (mock server, production build)
pnpm brand:icons   # regenerate the icons from public/brand/panda.svg (Playwright's Chromium)
```

Playwright uses the browser Playwright pins (`@playwright/test` is pinned exactly; CI runs
`playwright install --with-deps chromium`).

## Mock server

`mock/server.ts` is a small stateful mock of the orchestrator: the resource API (agents, threads,
rename, export, cancel) and the AG-UI routes (run, connect with `Last-Event-ID` and `?mode=run`, capabilities). It
keeps an event log per thread, plays a script against it, and shows it as AG-UI through
`mock/projection.ts`, its copy of the orchestrator's projection. `mock/server.contract.test.ts`
validates every response and every AG-UI frame against the contract and the vendored AG-UI schema.
The first word of the first message picks the script, the same words as the orchestrator's fake agent:

| First word | Behaviour |
|---|---|
| anything else (`echo`) | working, artifact `echo: <text>` with a PR link, done |
| `ask` | asks "Which branch?", blocks (the run ends in an interrupt); the `resume` answer runs to an `answered: <text>` artifact and done |
| `slow` | works until cancelled (`RUN_FINISHED` cancelled) |
| `fail` | a failed status with detail `scripted failure`, `RUN_ERROR` `agent_failed`, no error activity |
| `talk` | working, a status with text, one final agent message, the result, done |
| `ui` | working, an A2UI surface (a title and a `Go` button), the question "Pick one", blocked; the action on the surface (`forwardedProps.a2uiAction`, not a message) resumes it to `answered: ui-action go` and done |
| `title` | a rename (`PATCH /api/threads/{id}`, the `title` golden): the thread is renamed while it works, cancelled, and renamed again; a rename inside a run is a `STATE_SNAPSHOT`, one of a finished thread a run of its own that holds only a snapshot, which the web never hands to the runtime (the `connect-title` golden through the runtime adds no message). The mock's replay says the title the thread has when the viewer connects (the golden's starts from the first message's words) |
| `steps`, `steps-ask` | nested steps (ADR 0025, the `steps` and `steps-ask` goldens): a sub-agent step `OpenCode` (a subagent of the run, `sub-step-<seq>`) with a command `npm test` under it that fails (`1 failed`), the sub-agent's end, the agent's words and done; in `steps-ask` the command is `waiting` when the agent asks "Allow rm -rf build?" (the step's subagent suspends with the invocation) and the answer ends the command and the sub-agent. The mock's step ids carry the placeholder `T` where the orchestrator has the A2A task id. The web reads them into a tree per turn ([The step tree](#the-step-tree)) |
| `verify-pass`, `verify-red-once`, `verify-red` | the verification gate (3 attempts, the agent's own checks; the orchestrator's fake agent has the same words): checks pass at once; fail once, are sent back and pass on attempt 2 (`verify-green` golden); fail every time and end `RUN_ERROR` `checks_failed` (`verify-red` golden). `job` is on every snapshot and on `GET /api/threads/{id}` |
| `verify-reviewed`, `verify-reviewed-red` | the verifier agent of the gate (3 attempts, the `verifier` source, verifier `verifier`): it finds something in attempt 1, the agent is sent back, and the verifier passes attempt 2 (`verify-verifier-green` golden); `-red`: it never passes it and the run ends `RUN_ERROR` `checks_failed` (`verify-verifier-red`; the golden's message is `verify-reviewed`, the word only tells the two apart). The verifier is a subagent of its own, `sub-verify-<n>`, open from its pending card to its verdict (`result: {passed}`), and it is told to a client that joins |
| `verify-reviewed-wait` | mock only: the verifier is asked and never answers; its subagent stays open (and is in the preamble of a client that joins) until Cancel ends it as `canceled`. The mock does not play the holds of a timed-out or unreachable verifier (`SUBAGENT_ERROR`) |
| `verify-ci` | CI (ADR 0017, the `ci` golden): a gate on CI alone; the pending check, a red `vymalo.ci` report (`ci/build`, commit 1) that fails it and sends the agent back, then a green report for commit 2 and done |
| `verify-ci-stale` | mock only, **not produced by the current orchestrator**: a gate on CI and the agent's checks; the CI check is pending, then a late failed report of an older push (its own `vymalo.ci` card) with a stale answer, then a green report and the answer that passes (a check card replaced in place, a card that stands apart) |
| `verify-wait` | mock only: the same gate, CI never answers; the thread stays `verifying` (a pending card, no report card) until it is cancelled |
| `choices` | working, a surface of our catalog with a Choices of three questions (a database with an "Other", a login, where it runs: several, optional, with an "Other"), the question "Three questions", blocked; the answers (`forwardedProps.a2uiAction`, `context.answers`) resume it to `answered: ui-action answer db=pg auth=none deploy=k8s,compose` (what was chosen, in question order, `other:<words>` for their own) and done |
| `cards-mermaid` | mock only: one answer of the agent's words, a surface of our catalog with three `Cards` (two linked, one not) and a `Mermaid` flowchart, then done (no result artifact) |
| `cards-bad`, `cards-bad-url` | mock only: a `Cards` that breaks its schema (a card with no title and a `javascript:` link: rule `schema`), and one whose link passes the schema and not the URL rule (user information: rule `url`); the surface is refused and not drawn |
| `mermaid-bad`, `mermaid-hostile`, `mermaid-kinds` | mock only: a `Cards` beside a graph that does not parse (the graph says so, the cards are drawn); a graph that tries to switch its security off (front matter, a directive, HTML and a script in labels, a click handler and a link), drawn as a plain image; ten graphs, one of each common kind |
| `catalog-newer` | mock only: the thread was opened by a newer version of the app (its UI catalog is version 99, whatever the web sent) and the agent sends a surface of our catalog with a component this build lacks (`Gizmo`): the placeholder that asks for a newer version; the result and done |
| `catalog-unknown` | mock only: the same surface in a thread whose catalog is this build's: refused with the rule `catalog`; the result and done |
| `sources` | mock only: the coder's steps, a branch, a pull request, a CI report with a link, and a final answer with links in it (the pull request again, a doc, a run, a URL inside code, a `javascript:` one): what the panel's Sources tab lists; done |
| `partial` | mock only, **not produced by the current orchestrator**: a partial agent message replaced by its final version |
| `unreachable` | mock only: an error activity, `RUN_ERROR` `delivery_failed`, thread blocked |
| `Delegate`, `Investigate`, `steps-many` | mock only, nested steps at a scale the goldens do not have (`quick` steps play at once): the coder hands the work to OpenCode, a sub-agent step with fourteen steps under it (reads, a search, edits, commands, one test run that fails and is run again), then a push, a pull request and the answer (`Delegate`); the same still running a command until cancelled (`Investigate`); a sub-agent step with 120 reads under it, one of them failing, played at once: a level long enough to be a scroll box (`steps-many`) |
| `Fix`, `Refactor`, `Make`, `Upgrade`, `Deploy`, `Also`, `Migrate` | mock only, the coder scenarios of `pnpm screens` (plain words, so the titles read well): steps with commands, a push, the agent's checks, a pull request and a markdown answer (`Fix`); the same, still running a command (`Refactor`); a failed check, a rework and a pass (`Make`); nothing after the message (`Upgrade`); a question (`Deploy`); a short follow-up (`Also`); a failure with the agent's reason (`Migrate`). The wording of the steps is the mock's, not adam-coder's |

The mock tells the orchestrator's story: `mock/golden.test.ts` drives every scenario of
[`docs/api/examples`](../docs/api/examples/README.md) through the mock's run route and requires the
connect stream a viewer reads to be the golden `agui/<name>.agui.json`, frame for frame, so the mock
cannot drift from the orchestrator unnoticed. Test hooks for the e2e suite: `POST
/__mock/drop-streams` (cut every open stream), `POST /__mock/cut-next-connect?frames=n` (cut the
next connect stream after `n` frames, in the middle of a group) and `POST /__mock/reset`.

The mock also keeps what the web says about the UI catalog (ADR 0023): a run's
`forwardedProps["vymalo.uiCatalog"]` is checked as the orchestrator checks it, less the JSON Schema compilation (the
shape, an https `catalogId`, a version of 1 to 1,000,000, 64 KiB (`413`), at most 64 components with names like
`Choices`, and the digest **recomputed**: `400` otherwise, before anything is written), recorded as a `ui_catalog` event of the person,
first in the commit of the input and once per digest, when the run applies an input. The projection folds those events
with the core's rule (`CatalogLedger` in `mock/projection.ts`: the highest version is current, an older one is recorded
and never current, the same version with another digest replaces it), so a snapshot says `thread.uiCatalog` as of its
place in the log, a replay shows the catalog changing, and the event has no frame of its own.

Agents: `coder` (has `releases`) and `reviewer` (none).

## Tests

| Command | What | Count (2026-10-01) |
|---|---|---|
| `pnpm test` | vitest: the SSE reader; `ThreadAgent` (groups, reconnect with `Last-Event-ID`, dedupe by seq, acceptance at `RUN_STARTED`, problems, abort is not cancel); the AG-UI goldens through the patched runtime (messages, activities, interrupts, the cancelled outcome); the app in jsdom against the mock (new thread, replay, answer, refused send, cancel, not found, a surface and its action); **the A2UI validator and its security tests** (every bad URL scheme and trick, each limit at and one over, an expansion bomb and a reference bomb, the vocabulary, reserved names, function values, inputs), **the renderer** (the golden through the runtime, replace in place, delete, a refusal, a later run, no auto-send under fake timers, a click sends once, disabled states, `openUrl` as a link, `userMessage` in the composer unsent, no image ever) and **the app's side of an action** (what a click puts on the wire, with and without an open interrupt); **the verification gate** (the parsing of `vymalo.check`, `vymalo.rework` and `job`: malformed payloads draw nothing and unknown fields are ignored; the check and rework steps, the counter and the badge; findings that are markup or markdown shown as text, long ones cut and expandable; `verify-green` and `verify-red` through the runtime and through the whole `ChatShell` against the mock: the badge goes verifying, queued, working, verifying, done or failed, the counter 1/3 to 2/3, the check and rework steps in order, "Checks failed after 3 attempts", a reconnect mid-verification and a fresh page showing the same); **CI results** (`parseCi`: every required member needed, unknown fields ignored, a url that is not http(s) dropped; the CI step for every conclusion with its words, icon and tone, an unknown conclusion, a missing url, a `javascript:` url handed straight to the step, markdown and HTML in the name, branch and summary shown as text, a long summary and name cut; the `ci` golden through the runtime and through the whole `ChatShell` against the mock, a step replaced by whatever id the wire gives it, malformed payloads drawing nothing); **the turn** (what is a step, a card or prose, `lib/steps.ts`; the pull request and file cards; the thread list's recency groups); **Export JSON** (the file name the server gives and never a path, the download through the API client, a refused export saying why and downloading nothing, the mock's export against the contract); **Rename** (the title becomes a field that has the focus, Enter saves through the API and the header and the sidebar say it, Escape sends nothing, leaving the field saves once, the same or an empty title is no request, a refused rename says why and keeps the field; the mock's `PATCH` against the contract; a run that holds only a snapshot never reaches the runtime, in `ThreadAgent` and through the goldens); **the UI catalog** (the canonical JSON and the known-answer digest the orchestrator pins too, the lock against `catalog.json` and against the released versions, the catalog document's own rules, every send rule of `ThreadAgent`, the validator's catalog and schema rules, the placeholder for a newer catalog and the refusal when it is not newer); the mock against the contract and the goldens (`a2ui`, all four `verify-*` and `ci` included, the verifier's subagent among them) and its record of the UI catalog; **Choices** (the schema's limits at and one over, the two rules a schema cannot say, the reserved action name, the lowering through the converter, what is sent and in what order, `Other`, the gate on required questions, no send by drawing, choosing, typing, Enter or time, read-only when the thread does not wait or the copy is not the newest, a question id of `__proto__`; **the person's answers**: the shape of an answer, the labels resolved from the surface in the transcript and the raw ids and values without it, the bubble above the agent's mark and not a step, a button's action still a step, text only); **Cards and Mermaid** (both schemas' limits at and one over, a card's link by the schema and by `safeHttpUrl` with the bad-URL table, the validator's rules for both, the component for a newer thread; the cards drawn as text with their links and hosts and nothing fetched, a refusal that names the card; the graph drawn through a stand-in for mermaid: the strict configuration, the alt text, the source disclosure, loading, a parse error, a refused SVG, a broken image, a change of scheme, a later copy; **the pinned mermaid against hostile directives and front matter**, with a control that shows what mermaid's own `secure` list lets through; `svgImage` on scripts, handlers, loading CSS, entities and the viewBox; mermaid imported by one file only); **the brand** (`components/brand/`: the SVGs use only the three brand fills and no image, script or link, within 16 KB; the PNG and ICO sizes; the manifest's icons exist; an agent avatar's initial, stable tint, `aria-hidden` and 4.5:1 contrast in both schemes); **the agent picker** (`agent-menu.dom.test.tsx`: radio semantics, the check on the chosen agent, the releases as a group of the same menu, a refetch on every open, a failed refresh, the notice slot, the keyboard: Enter, Space, arrows, Escape returning the focus; on a thread the other agents as new-chat links; `lib/selection.test.ts`: which agent and release a new chat sends to, `?agent=`); **the step tree** (`lib/step-tree.test.ts`: the tree of a turn, numbered as the Sources tab numbers turns, nested by `path`, a step whose parent is not in the turn under the agent, a step said again updated in place (a retry too), an unknown kind or icon, a report that does not validate drawing nothing, today's activities as leaves with a command as a command step, the gate's nodes after the agent, a pending check spinning only while the run is open, the state of a turn (running, verifying, paused, failed, stopped, a turn that ended on a question staying paused), no running step in a turn that is not running, the turn's times, the summary's counts and the step it is on, `summaryLine` for every state, which children a level lists (the failed always), durations, and a turn rebuilt only when its message or the thread's state changed; `parseStep` in `vymalo.test.ts`; the `steps` and `steps-ask` goldens, and their connect variants, through the runtime and into a tree; `components/steps/turn-summary.dom.test.tsx`: the line, its name and chip for every state, the spinner only while running, a click opening the panel on the turn, `aria-expanded`; `steps-pane.dom.test.tsx`: collapsed by default, a click then "Show 10 more", the failed ones kept in view, the failure chip at every collapsed level, a level of 50 plain and one of 120 a window of rows, which turns are listed and open, following the live turn until the person chooses, a request to show a turn opening, scrolling to and focusing its header once per key; `steps-panel-content.dom.test.tsx`: the connected forms over the runtime playing the goldens; `chat-shell.dom.test.tsx`: a turn with nested steps is one line in the chat and no list, the line opens the panel on its turn, and each turn's line focuses its own); **the panel** (`features/panel/`: `lib/sources.test.ts`: every kind of source, only http(s) links, a URL in code never read, one item per URL with every turn that cited it, a typed source over the same link in words, the groups and their order, numbering the turns as the chat does; `lib/panel-state.test.ts`: where the panel docks and the widths it may have, what is remembered, and the head script run against the same rules, storage blocked; `hooks/use-panel.dom.test.tsx`: the defaults by window width, the shortcut on every combination of keys, the remembered choice, a sheet that is for the visit only, the `useStepsPanel()` contract of the step tree; `components/thread-panel.dom.test.tsx`: the landmark, the tabs and their arrows, the separator's keyboard and pointer, the focus on close, the sheet from the right and the bottom, a Turn button; `sources-tab.dom.test.tsx`) | 979 (49 files) |
| `pnpm test:e2e` | Playwright on the production build against the mock: axe (no serious or critical issue, light and dark; new, finished and blocked thread, and a thread with an A2UI surface waiting, finished and refused) and Lighthouse accessibility >= 95 in both schemes; create, follow-up, cancel, failure, releases, API errors; reconnect (a dropped stream, a cut in the middle of a message); two tabs; A2UI (a surface is drawn and only a click sends the action, read-only after the thread finishes and after a reload, a refusal, no remote content); the UI catalog (the first run of a thread carries it whole and a later one does not, the placeholder of a thread opened by a newer app, kept after a reload and never answered with a catalog, the refusal when the thread is not newer; axe on the placeholder, both schemes); Choices (three questions answered by clicking: the button waits, one action goes out with no message and no `resume`, the bubble with the labels, no step, the agent's echo, the Choices read-only and the same after a reload; "Other" as the person's own words; the keyboard alone, arrows and space, Enter in a text box sending nothing; on a phone too; axe unanswered, answered and after sending, both schemes); Cards and Mermaid with the real mermaid in Chromium (one turn of words, three cards and the picture of the graph, links in a new tab with their hosts, the source one click away, nothing requested outside the app, the same after a reload; mermaid loaded when a graph is drawn and not with every thread; the graph in the colours of the light page and of the dark one; a card that breaks the schema, and a link that breaks only ADR 0013's rule, each refused with its rule and the raw operations; a graph that does not parse beside cards that are drawn; a graph that tries to switch its security off, drawn as a plain image with no script run, no link and no request; ten kinds of graph all drawn; on a phone too; axe on the answer with the source open, on the graph error and on the refusal, both schemes); verification (sent back and done on attempt 2 of 3, `Checks failed after 3 attempts`, a pending check while verifying that survives a reload and ends with Cancel, a verifier agent's findings and pass, a pending check of the verifier that ends with Cancel, no counter without a gate; axe on a thread being verified, on one waiting for its verifier, on a stale check and on failed checks, both schemes); CI results (a red report and a green one as steps with their links, a late report of an older push, no card while CI has not answered; axe on the CI cards, both schemes); Export JSON, from the thread's menu (the thread downloads as `thread-<id>.json` with its events, and a failed export is said and leaves the page usable); nested steps (a delegation to OpenCode is one line in the chat, with its failure chip and its name for a screen reader, and the tree in the panel: depth 1 listed, the sub-agent one collapsed line with its count and its failure chip, a click then the latest three and the failed one, Show 10 more twice, closing it, the same tree after a reload; each turn's line focusing its own turn and again on a second ask; a level of 120 steps a scroll box that draws a window of the rows in Chromium; a running turn's line naming the step and spinning until Stop; axe on the tree open, on the scroll box and on a running turn, both schemes, on a desktop and on a phone's sheet); the turn (a coder's steps in the panel, one line in the chat, a command, the push, the checks, the pull request card and its link, the markdown answer said once; the live step and Stop; "is starting…" before the first event; a rework step and the folded findings; the question waiting for a reply; a suggestion that fills the box and sends nothing; the sidebar closed, remembered and opened); the brand (the icons, manifest and head tags served, the title, the panda in the sidebar and over the greeting, an agent's letter and not the panda in a turn); the agent picker (the menu button in the top bar, the agents with their descriptions and the check, the keyboard, a click outside, the release group, the choice the first message goes to, on a thread the other agents as a new chat with `?agent=`, every control of the top bar on the screen on a phone; axe with the menu open, both schemes); the panel (open by itself on a wide window and remembered, the shortcut from the message box, the tabs and their arrows, Sources listing the pull request, the branch, the run and the links in the agent's words once each and never a URL in code, a Turn button scrolling to and focusing the turn, the resize separator by keyboard and pointer, the sheet from the right and from the bottom with Escape and the focus back; axe with the panel open, both tabs and the empty one, both schemes) | 188 pass and 9 are skipped (the panel's desktop tests have no phone and its phone test no desktop, and one keyboard test) |
| `pnpm test:e2e:system` | the same UI against the real orchestrator, see below (Export JSON included: the downloaded file is the log the connect stream replays) | 23 |

## System tests

`pnpm test:e2e:system` runs the UI in Chromium against the **real orchestrator** (no mock of the
API): the `orchestrator` binary on Postgres, two scripted A2A agents from `orch-fake-agent`
(a test-only binary of the orchestrator workspace), and the production build of this app with
`/api/*` and `/agui/*` rewritten to the orchestrator. Identity plays oauth2-proxy: the browser
context sends `X-Auth-Request-Email` and the orchestrator runs without `AUTH_DEV_USER`.

```sh
(cd ../orchestrator && cargo build --locked -p orchestrator -p orch-testsupport \
  --bin orchestrator --bin orch-fake-agent)
createdb orch_system            # empty and dedicated: the tests TRUNCATE it
export ORCH_BIN=$PWD/../orchestrator/target/debug/orchestrator
export FAKE_AGENT_BIN=$PWD/../orchestrator/target/debug/orch-fake-agent
export DATABASE_URL=postgres://postgres:postgres@localhost:5432/orch_system
pnpm test:e2e:system
```

| Port | What |
|---|---|
| 3100 | the app (`pnpm build:system`, then `next start`) |
| 8080 | the orchestrator |
| 4020 | fake-agent control: `POST /__control/{coder,plain}/release-gate`, `GET /__control/{coder,plain}/calls` |
| 4021, 4022 | the `coder` (with release channels) and `plain` fake agents (`gated` is `plain` again, under the verification gate) |
| 3101 | `reconnect.spec.ts` only: a TCP forwarder in front of the app that cuts the connect stream |

`e2e-system/*.spec.ts` cover the identity through the rewrite (and 401, user isolation, and another
user running into a thread id), the create/echo lifecycle (and the log behind it, read as the
connect stream), agent text, ask and answer on the same A2A task (a `resume`), cancel reaching the
agent, the failure shape, the 409 on a finished thread, releases, a dropped stream, a SIGKILLed
orchestrator, history by URL (the connect stream closes on a finished thread), paging of the
thread list, and A2UI (the fake agent's `ui` surface drawn, the button answering the agent's open
question as an action with no message and no `resume`, delivered to the same A2A task; `ui-delete`; a
payload the orchestrator refuses), the UI catalog and Choices (`choices.spec.ts`: the fake agents list `ui-catalog/v1` through `FAKE_AGENT_EXTENSIONS` in `playwright.system.config.ts`; the web's catalog goes with the run that creates the thread, whole, and the agent is told it inline; the fake agent's `choices` draws a Choices under it, the answers reach the agent as one action with the catalog as a reference, and the log holds the `ui_catalog` event once, first), and the verification gate (`gated` is the `plain` fake agent under `gate: {require: [agent-checks]}` in `e2e-system/agents.yaml`: `verify-red-once` is sent back in a new task of the same context and ends done on attempt 2 of 3, `verify-red` ends `checks_failed` after three, `verify-pass` is green at once). Every test starts on an empty database; the orchestrator log of a run is
`e2e-system/.run/orchestrator.log`. CI runs it as the `system-e2e` job of
`.github/workflows/system.yml`.

## Image

`Dockerfile` (Node 24, non-root, standalone output, `EXPOSE 3000`); the build context is the
repository root:

```sh
docker build -f web/Dockerfile -t web .
```

The install stage copies `patches/` next to the lockfile: `patchedDependencies` must be on disk when pnpm
resolves the install.

CI builds it on every change and pushes `ghcr.io/vymalo/another-agentic-system/web:sha-<7>` and
`:latest` from `main` (`.github/workflows/web.yml`).
