# web — chat surface

Next.js (App Router, TypeScript strict), [shadcn/ui](https://ui.shadcn.com/) on Tailwind v4 and
[assistant-ui](https://www.assistant-ui.com/) on **AG-UI**: the conversation is the orchestrator's
[AG-UI 1.0](../docs/api/agui.md) projection of the event log, rendered by
`@assistant-ui/react-ag-ui`. It lets the owner pick an agent (and a release, when the agent offers
one), start threads, follow up, answer an interrupt, cancel and watch progress live (under a
verification gate: the attempt it is on, each check and its findings, and the rework), and it keeps
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
  ([DESIGN.md](DESIGN.md), "A turn"): the agent's mark and name once, then its parts in order through
  `MessagePrimitive.GroupedParts`. Every stretch of step parts (`lib/steps.ts`: a status but a
  failure, an artifact, `.check`, `.ci`, `.rework`, `.action`, the actor and job markers) is one
  compact step list (`steps/step-list.tsx`, always in view, the last step spinning while the run
  is open); the agent's words (`TEXT_MESSAGE_*`, including the words of a `completed` or
  `input_required` status, `st-<seq>`) are prose; a failed status and `.error` are soft callouts
  and `.a2ui-surface` the A2UI renderer (`data-uis.tsx`, see [A2UI surfaces](#a2ui-surfaces)); the
  pull requests and files the agent shared follow as cards (`cards/turn-cards.tsx`). The statuses
  that come with words (`completed`, `input_required`) draw no step: the words and the state pill
  say it. A shape a renderer does not know renders nothing.
- **Thread ids** are UUIDv7 (`src/lib/uuid.ts`): the consumer mints them, and the orchestrator lists
  threads by id, newest first.

### Dependencies and patches

Pinned exactly. *Verified 2026-09-29* on the npm registry (`npm view`):

| Package | Version | Note |
|---|---|---|
| `@assistant-ui/react-ag-ui` | 0.0.62 (2026-09-24, latest) | depends on `@ag-ui/client` `^0.0.59`, `@assistant-ui/core` `^0.3.21`, `@assistant-ui/react-generative-ui` `^0.0.21` |
| `@assistant-ui/react-generative-ui` | 0.0.21 (2026-09-24, latest) | a direct dependency and a peer of the runtime: the app uses its reducer, its converter and `renderGenerativeUI` behind its own validator ([A2UI surfaces](#a2ui-surfaces)); the runtime's own A2UI path is bypassed |
| `@ag-ui/client` | 1.0.0 (published 2026-09-17) | a direct dependency, and the `overrides` entry below; 1.0.1 was published 2026-09-29 and is not adopted (`tools/agui-conformance` reads the goldens with 1.0.0) |
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
| Check step | `steps/check-step.tsx` | `vymalo.check` as a step of the list, a `listitem` named "Check: <source>, attempt <n>, <status>[, stale]": what the source does or said ("Waiting for CI", "The verifier is reviewing the work", "The agent's checks failed"; an unknown source by its own name), a pill with the status in words and an icon (Passed, Failed, Pending), the attempt, the short commit (seven hex digits, else cut to 12; the full value in `title`), the CI check `name`, the summary, and the findings folded behind "Findings (n)". The verifier is named; the orchestrator is not. One step per source in one verification of one attempt (`check-<attempt>-<verification>-<source>`), replaced in place; a `stale` answer has its own id, a muted step marked "Stale" that says it decided nothing; a pending check whose run ended says no answer came |
| Findings | `parts/findings-list.tsx` | A list of **plain text**: React text nodes, never `dangerouslySetInnerHTML`, never the markdown renderer, so `<script>`, `**bold**`, `[x](javascript:...)` and `<img onerror>` show as the characters they are. A finding over 240 characters is cut (never in the middle of a surrogate pair) with "Show more" / "Show less" (`aria-expanded`); more than five findings are folded behind "Show all N findings" |
| Rework step | `steps/step-items.tsx` | `vymalo.rework`: a warning step "Checks failed — trying again (2/3)" ("CI failed", "The review found issues" when one source sent it back), the findings of every source folded under it. The next attempt's steps follow in the same list |
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
`prepareSurface(operations)` returns `surface` (what to draw), `pending` (valid so far, nothing to
draw yet: no `root`, or a child not sent yet), `deleted`, or `refused` with the rule and the reason,
never part of a surface. The checks run in this order, cheapest first, and the library's reducer and
converter only ever see operations that passed the ones before them:

| Rule (`refused.rule`) | Limit or requirement |
|---|---|
| `size` | at most **64 KiB** (65,536 bytes of UTF-8) of serialised operations; one byte more is refused |
| `shape`, `version`, `surfaces` | an array of objects, each with a `version` (`v0.9`, `v0.9.1`, `v1.0`; `v0.9.1` is read as `v0.9`) and exactly one operation (`createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`) with a surface id of 1 to 256 bytes; one surface per activity; anything the reducer had to skip (a component without an id, an update of a surface never created) refuses the surface |
| `components` | at most **400** components once the operations are applied (unreferenced ones count) |
| `vocabulary` | only the components below; an unknown one refuses the surface wherever it is (never skipped) |
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

## Layout

Every file name is kebab-case (`pnpm check` fails otherwise). Tests sit next to the code they test.

```
src/app/                       routes only: thin pages that compose features
src/components/ui/             shadcn primitives (generated by the shadcn CLI, then formatted)
src/components/assistant-ui/   assistant-ui registry items, pruned to what a run's parts need
src/components/inline-status.tsx   shared empty, loading and error lines
src/features/chat/             the conversation: components (shell, top bar, composer, LiveRuns,
                               steps/ the step list of a turn, the check and CI steps among them,
                               cards/ the pull request and file cards, parts/ callouts and text,
                               surface/ the A2UI renderer), hooks (runtime,
                               thread details), lib/agui (ThreadAgent, SSE reader, live runs, the
                               vymalo vocabulary), lib/steps.ts (what a turn draws as a step, a card
                               or prose), lib/findings.ts (shortening untrusted text),
                               lib/a2ui (the validator: limits, URLs, pointers, preparing a surface)
src/features/threads/          thread list: collapsible sidebar (desktop) and sheet (phone), grouped by
                               recency (lib/recency.ts), paging hook
src/features/agents/           the new chat: greeting, suggestion chips, agent and release pickers
                               (pills in the composer), agents hook
src/lib/                       api client and types (schema.d.ts is generated, never committed), uuidv7
patches/                       pnpm patches of dependencies, and the drafts of their upstream twins
e2e/, e2e-system/, mock/       Playwright suites (mock / real orchestrator) and the mock orchestrator
```

The look is [DESIGN.md](DESIGN.md), the brief with the references it is drawn from; the theme is
`src/app/globals.css`: its tokens (light, and dark under `prefers-color-scheme`) mapped onto
shadcn's names, Inter self-hosted (`@fontsource-variable/inter`). There is no theme toggle.
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
pnpm screens       # the screenshots of e2e/__screens__/ (mock server, production build)
```

Playwright uses the browser Playwright pins (`@playwright/test` is pinned exactly; CI runs
`playwright install --with-deps chromium`).

## Mock server

`mock/server.ts` is a small stateful mock of the orchestrator: the resource API (agents, threads,
export, cancel) and the AG-UI routes (run, connect with `Last-Event-ID` and `?mode=run`, capabilities). It
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
| `verify-pass`, `verify-red-once`, `verify-red` | the verification gate (3 attempts, the agent's own checks; the orchestrator's fake agent has the same words): checks pass at once; fail once, are sent back and pass on attempt 2 (`verify-green` golden); fail every time and end `RUN_ERROR` `checks_failed` (`verify-red` golden). `job` is on every snapshot and on `GET /api/threads/{id}` |
| `verify-reviewed`, `verify-reviewed-red` | the verifier agent of the gate (3 attempts, the `verifier` source, verifier `verifier`): it finds something in attempt 1, the agent is sent back, and the verifier passes attempt 2 (`verify-verifier-green` golden); `-red`: it never passes it and the run ends `RUN_ERROR` `checks_failed` (`verify-verifier-red`; the golden's message is `verify-reviewed`, the word only tells the two apart). The verifier is a subagent of its own, `sub-verify-<n>`, open from its pending card to its verdict (`result: {passed}`), and it is told to a client that joins |
| `verify-reviewed-wait` | mock only: the verifier is asked and never answers; its subagent stays open (and is in the preamble of a client that joins) until Cancel ends it as `canceled`. The mock does not play the holds of a timed-out or unreachable verifier (`SUBAGENT_ERROR`) |
| `verify-ci` | CI (ADR 0017, the `ci` golden): a gate on CI alone; the pending check, a red `vymalo.ci` report (`ci/build`, commit 1) that fails it and sends the agent back, then a green report for commit 2 and done |
| `verify-ci-stale` | mock only, **not produced by the current orchestrator**: a gate on CI and the agent's checks; the CI check is pending, then a late failed report of an older push (its own `vymalo.ci` card) with a stale answer, then a green report and the answer that passes (a check card replaced in place, a card that stands apart) |
| `verify-wait` | mock only: the same gate, CI never answers; the thread stays `verifying` (a pending card, no report card) until it is cancelled |
| `partial` | mock only, **not produced by the current orchestrator**: a partial agent message replaced by its final version |
| `unreachable` | mock only: an error activity, `RUN_ERROR` `delivery_failed`, thread blocked |
| `Fix`, `Refactor`, `Make`, `Upgrade`, `Deploy`, `Also`, `Migrate` | mock only, the coder scenarios of `pnpm screens` (plain words, so the titles read well): steps with commands, a push, the agent's checks, a pull request and a markdown answer (`Fix`); the same, still running a command (`Refactor`); a failed check, a rework and a pass (`Make`); nothing after the message (`Upgrade`); a question (`Deploy`); a short follow-up (`Also`); a failure with the agent's reason (`Migrate`). The wording of the steps is the mock's, not adam-coder's |

The mock tells the orchestrator's story: `mock/golden.test.ts` drives every scenario of
[`docs/api/examples`](../docs/api/examples/README.md) through the mock's run route and requires the
connect stream a viewer reads to be the golden `agui/<name>.agui.json`, frame for frame, so the mock
cannot drift from the orchestrator unnoticed. Test hooks for the e2e suite: `POST
/__mock/drop-streams` (cut every open stream), `POST /__mock/cut-next-connect?frames=n` (cut the
next connect stream after `n` frames, in the middle of a group) and `POST /__mock/reset`.

Agents: `coder` (has `releases`) and `reviewer` (none).

## Tests

| Command | What | Count (2026-09-30) |
|---|---|---|
| `pnpm test` | vitest: the SSE reader; `ThreadAgent` (groups, reconnect with `Last-Event-ID`, dedupe by seq, acceptance at `RUN_STARTED`, problems, abort is not cancel); the AG-UI goldens through the patched runtime (messages, activities, interrupts, the cancelled outcome); the app in jsdom against the mock (new thread, replay, answer, refused send, cancel, not found, a surface and its action); **the A2UI validator and its security tests** (every bad URL scheme and trick, each limit at and one over, an expansion bomb and a reference bomb, the vocabulary, reserved names, function values, inputs), **the renderer** (the golden through the runtime, replace in place, delete, a refusal, a later run, no auto-send under fake timers, a click sends once, disabled states, `openUrl` as a link, `userMessage` in the composer unsent, no image ever) and **the app's side of an action** (what a click puts on the wire, with and without an open interrupt); **the verification gate** (the parsing of `vymalo.check`, `vymalo.rework` and `job`: malformed payloads draw nothing and unknown fields are ignored; the check and rework steps, the counter and the badge; findings that are markup or markdown shown as text, long ones cut and expandable; `verify-green` and `verify-red` through the runtime and through the whole `ChatShell` against the mock: the badge goes verifying, queued, working, verifying, done or failed, the counter 1/3 to 2/3, the check and rework steps in order, "Checks failed after 3 attempts", a reconnect mid-verification and a fresh page showing the same); **CI results** (`parseCi`: every required member needed, unknown fields ignored, a url that is not http(s) dropped; the CI step for every conclusion with its words, icon and tone, an unknown conclusion, a missing url, a `javascript:` url handed straight to the step, markdown and HTML in the name, branch and summary shown as text, a long summary and name cut; the `ci` golden through the runtime and through the whole `ChatShell` against the mock, a step replaced by whatever id the wire gives it, malformed payloads drawing nothing); **the turn** (what is a step, a card or prose, `lib/steps.ts`; the pull request and file cards; the thread list's recency groups); **Export JSON** (the file name the server gives and never a path, the download through the API client, a refused export saying why and downloading nothing, the mock's export against the contract); the mock against the contract and the goldens (`a2ui`, all four `verify-*` and `ci` included, the verifier's subagent among them) | 514 (28 files) |
| `pnpm test:e2e` | Playwright on the production build against the mock: axe (no serious or critical issue, light and dark; new, finished and blocked thread, and a thread with an A2UI surface waiting, finished and refused) and Lighthouse accessibility >= 95 in both schemes; create, follow-up, cancel, failure, releases, API errors; reconnect (a dropped stream, a cut in the middle of a message); two tabs; A2UI (a surface is drawn and only a click sends the action, read-only after the thread finishes and after a reload, a refusal, no remote content); verification (sent back and done on attempt 2 of 3, `Checks failed after 3 attempts`, a pending check while verifying that survives a reload and ends with Cancel, a verifier agent's findings and pass, a pending check of the verifier that ends with Cancel, no counter without a gate; axe on a thread being verified, on one waiting for its verifier, on a stale check and on failed checks, both schemes); CI results (a red report and a green one as steps with their links, a late report of an older push, no card while CI has not answered; axe on the CI cards, both schemes); Export JSON, from the thread's menu (the thread downloads as `thread-<id>.json` with its events, and a failed export is said and leaves the page usable); the turn (a coder's steps in words, a command, the push, the checks, the pull request card and its link, the markdown answer said once; the live step and Stop; "is starting…" before the first event; a rework step and the folded findings; the question waiting for a reply; a suggestion that fills the box and sends nothing; the sidebar closed, remembered and opened) | 91 pass and 1 is skipped |
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
payload the orchestrator refuses), and the verification gate (`gated` is the `plain` fake agent under `gate: {require: [agent-checks]}` in `e2e-system/agents.yaml`: `verify-red-once` is sent back in a new task of the same context and ends done on attempt 2 of 3, `verify-red` ends `checks_failed` after three, `verify-pass` is green at once). Every test starts on an empty database; the orchestrator log of a run is
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
