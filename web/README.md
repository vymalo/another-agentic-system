# web — chat surface

Next.js (App Router, TypeScript strict), [shadcn/ui](https://ui.shadcn.com/) on Tailwind v4 and
[assistant-ui](https://www.assistant-ui.com/) on **AG-UI**: the conversation is the orchestrator's
[AG-UI 1.0](../docs/api/agui.md) projection of the event log, rendered by
`@assistant-ui/react-ag-ui`. It lets the owner pick an agent (and a release, when the agent offers
one), start threads, follow up, answer an interrupt, cancel and watch progress live (the agent's words as they are
written, [Live text](#live-text); under a verification gate: the attempt it is on, each check and its findings, and the rework), see what the
agents shared (pull requests, branches, CI runs, files, links) in a panel beside the conversation, and it keeps
following a thread that another tab or the orchestrator itself moves. Decisions:
[ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md) (AG-UI as the user-facing
protocol), [ADR 0006](../docs/decisions/0006-assistant-ui-external-store.md) (assistant-ui),
[ADR 0011](../docs/decisions/0011-web-shadcn-tailwind-feature-layout.md) (UI kit and layout);
release picker: [ADR 0008](../docs/decisions/0008-platform-integration-via-a2a-extension.md).

The UI renders what the server says. It never invents state: "running" is the thread state the
server sent, a refused send is taken back out of the transcript, and everything after a reload is
the log replayed.

## What it looks like

Screenshots of the real UI, taken by `pnpm screens` against the mock server ([Layout](#layout)), so the agents, titles and
wording are the mock's, not a real agent's. Every state, on a desktop and on a phone, in light and in dark, is in
[`e2e/__screens__/`](e2e/__screens__/); the pictures here follow your browser's colour scheme.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-done-pull-request.png">
  <img src="e2e/__screens__/desktop-light-done-pull-request.png" alt="A finished job in the chat. Left: the thread list. Middle: the coder’s answer under the line “9 steps · 4s”, with a code block and a pull request card with a View pull request button. Right: the Activity panel listing the steps, from reading the code to the passing checks and the opened pull request." width="720">
</picture>

*A finished job: the threads on the left, the answer and its pull request in the reading column, the steps in the panel on the right.*

| A new chat | The agent menu |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-empty-thread.png"><img src="e2e/__screens__/desktop-light-empty-thread.png" alt="A new chat: the panda mark over the greeting “What should we get done?”, a line on what the chosen agent, Coder, does, a message box and four suggestion chips. The thread list on the left is empty." width="400"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-agent-menu.png"><img src="e2e/__screens__/desktop-light-agent-menu.png" alt="The agent menu open over a new chat: three agents (Coder, Reviewer, Verifier) with one line each and a check on the chosen one, then the release channels production and staging, and three revisions." width="400"></picture> |

| The panel's Sources tab | A thread waiting for the person |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-panel-sources.png"><img src="e2e/__screens__/desktop-light-panel-sources.png" alt="A finished thread with the panel on its Sources tab: a branch and a pull request, a passed CI check and a link, each with a Turn 1 button." width="400"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-your-turn.png"><img src="e2e/__screens__/desktop-light-your-turn.png" alt="A thread that waits for the person. The agent asks “Should I deploy to staging or straight to production?”, with a “Waiting for your reply” chip under it. The pill in the top bar reads Your turn, the message box says Reply… and the panel shows two steps." width="400"></picture> |

On a phone the thread list and the panel are sheets:

| The thread list | A finished turn | The panel |
|---|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-sidebar.png"><img src="e2e/__screens__/mobile-light-sidebar.png" alt="A phone: the thread list as a sheet that slides in from the left, with the panda mark and wordmark, a New chat button and the titles of the threads." width="200"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-done-pull-request.png"><img src="e2e/__screens__/mobile-light-done-pull-request.png" alt="A phone: the end of a finished turn, with the answer’s code block, the pull request card and its View pull request button, and the message box. The pill in the top bar reads Done." width="200"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-panel-activity.png"><img src="e2e/__screens__/mobile-light-panel-activity.png" alt="A phone: the panel as a sheet from the bottom, on its Activity tab, over the dimmed chat. It lists the turn’s steps and ends in a passed CI check with a View run link." width="200"></picture> |

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
| `GET /api/agents`, `GET /api/registry`, `GET /api/threads`, `GET /api/threads/{id}` | the agent list (the deployment's agents and the platform's, [ADR 0022](../docs/decisions/0022-platform-provisions-agents-system-discovers-them.md): `source` and `tags` on each) and, read together with it, whether each source of agents could be read (the picker says when the platform's registry is unreachable); the thread list, a thread's title and target |
| `POST /api/threads/{id}/cancel` | Cancel (AG-UI has no consumer cancel) |
| `PATCH /api/threads/{id}` | **Rename** and **Add or Edit description** in the thread's overflow menu. Rename: the title in the top bar becomes a field (Enter or leaving it saves, Escape gives it up, the same title or an empty one is no request); a refused rename says why and keeps the field. The answer is the thread, so the header says the new title at once, and the sidebar's list is fetched again with it. A person's title is final (the orchestrator will never replace it). The description is the same with `{description}`: the line under the top bar becomes a field (limited to 500 characters), an empty text clears it, the same text is no request, and a person's is final too (the orchestrator's model never writes it again; [ADR 0035](../docs/decisions/0035-utility-model-tasks.md)); see [A thread's description](#a-threads-description) |
| `GET /api/config` | the public configuration, read once per page load ([ADR 0034](../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md)): `ui.showDescriptions` (default `true`) says whether a thread's description is drawn at all. A configuration that cannot be read leaves the defaults; descriptions wait for the answer so that one that is then switched off never flashes (`use-ui-config.ts`) |
| `POST /api/threads/{id}/fork` | **Fork from here** (a turn action) and **continue with another agent** (the agent menu, after a question): `{after: <an event of the turn>, target?}` makes a new chat that holds the conversation up to the end of that turn, and the page goes to it ([ADR 0029](../docs/decisions/0029-forking-a-thread-copies-its-log.md)); **Edit** under a message of the person is the same route with `{replace: <seq>, text, messageId}`: a new chat that holds what came before the message, the new words and the agent's answer, and the page goes to it at `#m-<seq>`. The page chooses the id of the fork, kept for a repeat of the same request. `409 turn_open` is shown under the top bar ("The agent is still working on this turn…"); the buttons are disabled while a turn runs, so it is the race only |
| `GET /api/threads/{id}/branches`, `GET /api/threads?branches=include` | the versions of a message: `‹ 2/3 ›` under a message that was edited (each version is a thread; the arrows go to it). The thread list leaves the edits out, and highlights the conversation's first thread while an edit is open |
| `GET /api/threads/{id}/export` | **Export JSON** in the thread's overflow menu (the `…` of the top bar): the whole thread (messages, agent statuses, artifacts, check, CI and verifier cards, reworks, the job) as `thread-<id>.json`, to send to a developer. The file is the server's: its `thread` carries the description and its log the `thread_described` events, whether or not the web shows descriptions |

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
  group), drops groups it has delivered (live text, which has no `id:`, is the exception: [Live text](#live-text)),
  and routes a run by `runId`: a run this browser started
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
  (`useAgUiInterrupts`); the question is the agent's last words in the conversation, with the chip "Waiting for your reply",
  and the composer sends the answer as `resume` (`steerAway` with a `resolved` entry carrying `payload.text`). Never a plain message: the
  orchestrator refuses a message together with a `resume`.
- **A send the server refuses** (a problem before the stream) is a `SendError`, assistant-ui's
  `MessageNotSentError`: the composer takes its text back, the failed message is removed from the
  transcript (`dropFailedSend`) and the problem's `detail` is shown.
- **A turn** (one run, one assistant message) is drawn by `thread.aui.tsx` as a classical chat
  ([DESIGN.md](DESIGN.md), "A turn"): the agent's avatar and name once, one **summary line** for its steps, then its
  parts in order through `MessagePrimitive.GroupedParts`. Every stretch of step parts (`lib/steps.ts`: a status but a
  failure, an artifact, `.check`, `.ci`, `.rework`, `.action`, `.step`, the actor and job markers) draws **nothing**
  in the chat: the steps are the side panel's Activity tab ([The step tree](#the-step-tree)), and the line
  (`steps/turn-summary.tsx`) opens the panel on this turn; the agent's **answer** (`TEXT_MESSAGE_*`, including the
  words of a `completed` or `input_required` status, `st-<seq>`) is prose, and the words it said while it worked are
  not drawn here but are notes in the panel ([The answer and the working text](#the-answer-and-the-working-text)); a failed status and `.error` are soft callouts
  and `.a2ui-surface` the A2UI renderer (`data-uis.tsx`, see [A2UI surfaces](#a2ui-surfaces)); the reply the agent is
  still writing is a **draft** after those parts ([Live text](#live-text)); the
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

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-rework.png">
  <img src="e2e/__screens__/desktop-light-rework.png" alt="The Activity panel of a thread whose agent was sent back: a failed check with its findings, “Checks failed — trying again (2/3)”, then “Verified the agent’s checks: Passed”. The chat holds the line “15 steps · 6s” with a “2 failed” chip, the answer and the pull request card." width="720">
</picture>

*A rework (the mock's `Make …`): the first check failed with a finding, the gate sent the agent back ("trying again (2/3)") and the second attempt passed. The gate's verdicts are steps of the panel's Activity tab.*

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

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-steps-opened.png">
  <img src="e2e/__screens__/desktop-light-steps-opened.png" alt="A delegation to OpenCode. In the chat one line, “20 steps · 3s” with a “1 failed” chip, then the answer and the pull request card. In the Activity panel the tree: OpenCode opened to its failed command and its latest three, with a “Show 10 more” link, then the push, the checks and the pull request." width="720">
</picture>

*The mock's delegation to OpenCode: one line in the chat ("20 steps · 3s", with the failure it holds), and in the panel the tree, a level opened to its latest steps and its failed one.*

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-step-io.png">
  <img src="e2e/__screens__/desktop-light-step-io.png" alt="The Activity panel of a turn of tool calls. “Web search”, tagged “search”, with the query “Stephane Segning” beside it, is opened: Input lists query, limit and a redacted api_key, Output is a monospace box with two results. Below, a failed command is opened to an Error box that holds a build's type error. The other steps are closed one-line rows." width="720">
</picture>

*A tool step opened (the mock's `steps-io`): what it was called with, what it returned, and the error of the one that failed.*

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
- **A tool step opens onto its input and output** (`steps/step-io.tsx`, [ADR 0030](../docs/decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md);
  `parseStep` reads `input`, `output` and `ioDropped` as the orchestrator logs them, one of the wrong shape dropped
  and the step kept). A step with any of them is a button with `aria-expanded`, like a sub-agent (which opens its
  children and its block together); one with none is a plain row. The block lists **Input** (a key/value list when
  the arguments are plain, pretty JSON when they nest, "Input not kept (n KiB)" for `{_cut, bytes}`), then
  **Output** (a monospace box, "n KiB more not kept" when cut) or **Error** in its place when the call failed, then
  a note for `ioDropped`. Everything an agent sent is a text node: nothing is parsed, nothing is markup, and no
  control name carries the agent's words. `lib/step-label.ts` humanizes `server__tool` ("Web search", from
  "search") and quotes one short argument beside it (`inputPreview`); the raw label is the tooltip.
- **The failed chip of the chat's line** is a button of its own beside the line's (`FailedChipButton`). It calls
  `openSteps(turnId, stepId)` with `firstFailed(turn)`: the pane opens the turn, the way to the step (`pathTo`,
  `withPathOpen`: each step on the way lists far enough back to include the next) and the step itself, scrolls to
  its row and focuses its button. The expansion state is the shell's, so it survives closing the panel. A
  `run_checks` that failed is counted once: its step and the red `checks` artifact beside it are one failure
  (`markEchoes`: the artifact keeps its row and its findings, its node says `echo`, and `countUnder`, the turn's
  summary and `firstFailed` skip it).
- **`StepsPanelContent`** is the connected form the panel's Activity tab renders: `useTurnSteps()` over the runtime,
  `useStepsPanel()` (the panel's contract: its id, whether it shows, the tab, the focus request, `openSteps`) and
  `useStepsExpansion()`. It must sit inside the thread page's `AssistantRuntimeProvider` and `ThreadViewProvider`.

## The answer and the working text

The owner, on the coder's chat (2026-10-02): the tool calls went to the right rail "while the working comments were
staying in the middle of the page", and what they wanted was "working tokens like thinking tokens and a final turn
answer, so that all other ones appear like thinking, but all hidden, not collapsed"
([ADR 0031](../docs/decisions/0031-working-text-and-the-turns-answer.md)).
So the chat's column holds **one answer per turn**. What the agent said while it worked (the sentence before a
tool call) is not in the column at all, neither collapsed nor behind a control: it is a **note** among the steps of
the Activity tab, in the place in time it was said.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-working.png">
  <img src="e2e/__screens__/desktop-light-working.png" alt="A finished coder chat. In the middle column the agent's one answer, a surface of two cards drawn on the way (a background and a sun) and the line “11 steps · 5s” with a “1 failed” chip. In the Activity panel the same turn: six notes, each a sentence the agent said before a tool call, in order among the tool steps they announced." width="720">
</picture>

*The mock's `coder-notes` (the owner's chat in other words): the answer and the surface in the column, the six sentences said on the way as notes in Activity.*

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-working-running.png">
  <img src="e2e/__screens__/desktop-light-working-running.png" alt="A coder turn that is still working. The line under the agent's name says what it is on, and under it one muted line, the last thing the agent said (a longer one would end in an ellipsis). There is no answer in the column yet; the Activity panel lists the notes said so far among the steps." width="720">
</picture>

*While the turn runs: its line, and under it the last working sentence as a quiet line (the ticker).*

- **Which text is the answer** (`lib/working.ts`, pure). The orchestrator marks the text of a stated stream by the
  status it came on (`metadata["vymalo.purpose"]` of the `START`: `working`, `answer`); `ThreadAgent` puts the mark in
  a marker part, `vymalo.purpose`, right before the text it marks (the runtime keeps the text and drops the
  metadata), and `textRoles(content, running)` gives each text part a role. **What the log says wins.** Text with no
  mark (a plain A2A agent, the status words, every log written before the mark) is read by the rule of ADR 0031
  point 5: in a turn that is over, the **last unmarked text is the answer** and the earlier text is working (when a
  text is marked `answer`, it is the answer and an unmarked one is working); in a turn that runs, an unmarked text
  shows as a draft of the answer **until a step starts after it**, then folds into Activity (an artifact, a check or
  a CI report after the words is not a step of the agent). So an old thread has the new view too, and the log is
  never rewritten.
- **The turn** (`thread.aui.tsx`) draws only the text parts whose role is not `working` (`TextLeaf` asks the
  runtime's part accessor for its index; the message the runtime holds is whole, nothing is removed from it). **Copy**
  copies the answer. A surface drawn on the way (`show`) stays in the column: it is an output. A question that ends the
  turn (`input_required`) is the answer, with its "Waiting for your reply".
- **The notes** (`lib/step-tree.ts`): a `note` node, at depth 1 of the turn, in part order among the steps. It is not
  a step: not counted ("11 steps" is the steps), not the step the turn is on, never failed. A turn that has notes is
  listed in the panel even with no step, and its line says "3 notes" when it has no steps. `NoteStep` draws the words
  whole in the page (a screen reader reads "Working note: …" and then all of it) with a three-line clamp and a
  Show more control for a long one, as a text node and never as markup.
- **Live text** (`lib/agui/live-drafts.ts`, [ADR 0027](../docs/decisions/0027-live-text-relayed-not-stored.md)). A
  draft is the answer unless its `END` says `{final: true, purpose: "working"}`: then the plain message the runtime
  reads says `vymalo.purpose: working` on its `START` and the draft draws nothing in the column. An `END` that says
  only `final` is the answer (or unmarked text) and the draft stays. A working sentence is therefore in the column
  for as long as its model turn streams, then goes to Activity.
- **The ticker** (`TurnSummary`): while a turn runs, `TurnSteps.ticker` is the last line of its last note, plain (code
  ticks and emphasis dropped, a link keeps its words), cut at 160 characters and by the line's width with an
  ellipsis, in muted 12 px text under the turn's line. It is not a control, takes no focus and is **not a live
  region**: a sentence every few seconds would talk over the state pill and the reply, the notes are in Activity in
  order for whoever wants them, and the line's own name does not change.
- **Not changed:** Export JSON (the events keep `purpose`), the log, and the runtime's message model.

## Live text

The agent's words are shown **while they are written** ([ADR 0027](../docs/decisions/0027-live-text-relayed-not-stored.md),
sys #65, [the stream](../docs/api/agui.md#live-text), [`text-stream/v1`](../docs/api/text-stream-v1.md)). They are not in the
log: the log holds the reply once, final, and the runtime's transcript is the log. So the words travel as frames of their
own, and the web keeps them out of the runtime as **drafts**.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-reply-writing.png">
  <img src="e2e/__screens__/desktop-light-reply-writing.png" alt="A reply being written. The agent’s words so far, an introduction and a list whose second item is still being typed, end in a thin bar. The top bar says Working… and the Activity panel shows one step." width="720">
</picture>

*A draft (the mock's `Write …`): the words so far, in the type of the finished reply, and a caret after the last character.*

```mermaid
sequenceDiagram
  autonumber
  participant O as Orchestrator (connect stream)
  participant T as ThreadAgent
  participant D as Drafts (lib/agui/live-drafts.ts)
  participant R as Runtime (messages)
  participant V as Turn (thread.aui.tsx)
  O-->>T: SUBAGENT_STARTED, status working, id: 2 (a group of the log)
  T->>R: the group, whole
  O-->>T: TEXT_MESSAGE_START {vymalo.live: {}} (no id:)
  T->>D: START: a draft {id, name, invocation, actor}, empty
  O-->>T: TEXT_MESSAGE_CONTENT {vymalo.live: {offset 0}} "Fib" (no id:)
  T->>D: text = text.slice(0, offset) + delta
  D-->>V: the draft's words, with a caret (the turn renders by itself)
  Note over T,R: a live frame is read when it arrives, never held for an id:, never a resume point, never in the runtime
  O-->>T: CONTENT {offset 18, final} + END {final}, id: 3 (the log's message, one group)
  T->>D: the draft up to offset + the delta, whole
  T->>R: TEXT_MESSAGE_START, CONTENT (the whole text), END: one plain message
  R-->>V: the reply, drawn where the draft was, and the draft draws nothing
  O-->>T: the next group: the draft is dropped
```

```mermaid
stateDiagram-v2
  [*] --> Writing: START (a draft, empty)
  Writing --> Writing: CONTENT {offset} grows it (a repeat or a gap changes nothing)
  Writing --> Gone: END {abandoned} (the model gave up, Stop, the invocation or the run closed)
  Writing --> Completed: the log's message, CONTENT {offset, final} + END {final}
  Completed --> Gone: the next group of the log
  Writing --> Gone: a cut connection (the new one says the text again from the start)
  Gone --> [*]
```

- **`lib/agui/live-drafts.ts`** (pure) holds the rules. A frame with `metadata["vymalo.live"]` that is not the log's own
  (`final`) is handled by `ThreadAgent.onFrame` at once, never grouped and never passed on. `START` opens a draft;
  `CONTENT{offset}` does `text = text.slice(0, offset) + delta` when that grows the text (`offset` is in UTF-16 code
  units, the unit of a JS string; an offset beyond what is held is a gap, which the sender's next refresh from 0 closes;
  a repeat says nothing; a draft stops at 256 Ki units, the contract's bound); an `END{abandoned}` removes the draft.
- **The log's message completes the draft.** Its frames come in the group of its event, as `CONTENT{offset, final}` and
  `END{final}` with the `id:`; `resolveGroup` turns them into the one message the runtime reads (`TEXT_MESSAGE_START`,
  `CONTENT` with the draft's text up to `offset` plus the rest, `END`, with the draft's name and invocation), so the runtime
  has exactly what it would have had without live text and the goldens through it are the same transcript. A final with
  `offset: 0` is the whole text (the sender replaced what it had said), so it needs no draft. A final that continues text
  this connection never held (a draft that is missing or shorter than `offset`) cannot be told whole: **the group is not
  delivered and the connection is reopened at the last resume point**, which says the message plainly because the new
  connection's overlay starts empty (not shown as an error). A message the log says after a given-up stream has another id
  (`<id>~final`) and is an ordinary one.
- **The runtime never sees a draft**, so `translate` never meets a message id the log lacks (an unknown assistant message in
  the history is a 422), a reconnect cannot duplicate one, and nothing about replay, resume or Export changes. A cut
  connection forgets its drafts, and so does the end of a run; the next connection is told the text so far again by the
  sender's refresh (within a second) or the final message.
- **The draft is drawn by the turn**, not by the transcript: `AssistantMessage` draws the drafts of the newest agent turn
  after its parts and before its cards (`LiveDraft`, `data-slot="agent-draft"`, markdown through the same renderer as the
  reply, a caret, `aria-busy` and `aria-live="off"`; [DESIGN.md](DESIGN.md) "A turn"). They reach it through
  `ThreadAgent.getDrafts()`, not the snapshot, and a `LiveDraftsProvider` of their own, so the text growing several times a
  second renders that turn and not the chat. A completed draft says the log's words until the transcript has them and
  then draws nothing (`drawnDrafts`), so the swap is one render: the words are never missing and never there twice.
- **The mock** relays live text as the orchestrator does (`mock/live.ts` is its copy of the overlay, held to the `stream`
  golden by `mock/golden.test.ts`, and the sender's refresh every second, `refreshMs`), with the scenarios `stream`,
  `stream-long`, `stream-hold`, `stream-gate` and `stream-abandon` ([Mock server](#mock-server)).

## A thread's description

*Added 2026-10-02 (S19; [ADR 0035](../docs/decisions/0035-utility-model-tasks.md)).* When a job ends the orchestrator's own
model says in a sentence or two what the conversation is about now, and the web draws it in three places. It is
`Thread.description` in the list and in `GET /api/threads/{id}`, in the export's `thread`, and in every
`STATE_SNAPSHOT.thread` of the stream (absent when there is none or a person cleared it).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-description-card.png">
  <img src="e2e/__screens__/desktop-light-description-card.png" alt="A finished chat titled “Plan the session expiry test”. Under the top bar one muted line, “The person wants a plan for adding a test to the session expiry check, so that a session idle for thirty minutes is refused with a 401. The agent read the repository…”, cut at the end of the line with a Show more link. Over the chat, beside the thread's row in the list on the left, a small card with the thread's title and the whole description." width="720">
</picture>

*The mock's `Plan` script, from the web's mock server: the description under the top bar, and the card the thread's row in the list opens.*

- **Under the top bar**, a muted one-line text in the chat's column (`thread-description.tsx`). When the text does not fit
  its line (measured, so a narrower window or a phone brings the control in) a **Show more** button opens the whole text
  and **Show less** closes it: a native button with `aria-expanded` and `aria-controls`, so Enter and Space work. The whole
  text is in the page for a screen reader either way. A thread with no description draws nothing there.
- **In the list**, a thread with a description has a **hover card** (`ui/hover-card.tsx`, Radix `HoverCard`): the title and the
  description, opened by resting the pointer on the row (after 0.4 s) or by focusing the link with the keyboard, closed by
  leaving, blurring or Escape. The card is for the eye (`aria-hidden`): a screen reader reads the description as the
  link's **description** (`aria-describedby` on the link, to a visually hidden copy), and the link's name stays the title.
  A touch screen has no hover, so on a phone the sheet shows no description; the line under the top bar has it.
- **Live.** The open thread follows the stream: a description the model writes after the job's end (a producer-initiated run
  that holds only a snapshot, which never reaches the transcript) bumps the thread's `lastSeq`, the resource is fetched again
  (debounced, as for a rename), and the line appears or changes, in the header and in the sidebar's row, with no reload.
  The model writes it after the thread is `done`, so **a finished thread keeps its stream for 45 s** (`FINISHED_GRACE_MS` in
  `use-chat-runtime.ts`, counted by `use-elapsed.ts`) before the page lets go of it; it used to let go the moment the thread
  was finished and caught up, which would have left a description that arrives a second later for the next reload.
- **Writing it.** **Add description** (or **Edit description**) in the thread's `…` menu turns the line into a field,
  as Rename does the title: the focus goes to it, Enter or leaving it saves, Escape gives it up, a refused save
  says why ("Could not save the description: …") and keeps the field and the words. Unlike a title, **an empty
  text clears the description** (`PATCH {description: ""}`), and the same text is no request. A person's description,
  a cleared one included, is final: the orchestrator's model never writes this thread's description again. Both edits share
  `use-thread-edit.ts` (`use-rename-thread.ts` and `use-describe-thread.ts` are its two uses).
- **Plain text.** The model wrote it, so it is untrusted: every place draws it as a text node, never as Markdown or markup
  (`**x**` is shown as typed), and the field is a single line with `maxLength` 500.
- **Switching it off.** `ui.showDescriptions: false` in the orchestrator's configuration (`GET /api/config`) hides it
  everywhere the web draws it: the line, the card, the link's description and the two menu items. The API returns it either
  way, and so does the export.

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
- **`Image`** (version 4, [below](#files-the-agent-made-and-image)) is output only too, and draws a file of the thread. After its
  schema (a hash, never a URL; `alt` required) the validator checks it against the thread's kept files (`PrepareOptions.files`:
  rule `artifact` when the hash is not one of them or not an image) and lowers it to `vymalo.Image`.
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

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-cards.png">
  <img src="e2e/__screens__/desktop-light-cards.png" alt="An answer of words and a list of cards under the heading “Three ways to keep a session”: a card that links to postgresql.org, one that links to owasp.org and a third without a link, each with a title, a line, a sentence and tags." width="640">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-cards-mermaid.png">
  <img src="e2e/__screens__/desktop-light-cards-mermaid.png" alt="The same answer scrolled to its graph: a flowchart titled “How a request meets a session”, drawn from Mermaid (a request arrives, is there a session cookie, look up or create a session, handle the request), with a collapsed “Diagram source” and a caption." width="640">
</picture>

*The mock's `cards-mermaid` answer, from the Reviewer: the cards, then the graph further down the same turn.*

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

### Files the agent made, and `Image`

[ADR 0032](../docs/decisions/0032-files-from-agents-live-in-an-artifact-store.md): an agent can hand over a file (a chart, an
export, a report), the orchestrator keeps it in its artifact store, and the log has a `vymalo.artifact` with `kind: "file"` and
the reference (`href`, `sha256`, `size`, `filename?`, `preview`: [`agui.md`](../docs/api/agui.md#typed-artifacts)). The web draws
it (`components/cards/kept-file-card.tsx`, `lib/files.ts`), lists it in the panel's Sources tab and lets an agent place an image
of it in a surface (`components/surface/image.tsx`, catalog version 4).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-files.png">
  <img src="e2e/__screens__/desktop-light-files.png" alt="An answer of the Reviewer, “I made three files: a chart, my notes and an export of everything”, under three cards: results.png with a bar chart of five green bars, notes.txt with its text shown in a box, and export.zip alone. Every card has a Download button beside its name, its size and its type. The Activity panel on the right lists three steps, “Shared results.png”, “Shared notes.txt” and “Shared export.zip”." width="720">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-file-image.png">
  <img src="e2e/__screens__/desktop-light-file-image.png" alt="An answer whose interface places the chart in the words: “The results at a glance”, the bar chart with the caption “Figure 1: the results of the run”, and under the answer the card of the same file with its Download button." width="720">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-panel-files.png">
  <img src="e2e/__screens__/desktop-light-panel-files.png" alt="The Sources tab of the panel for the thread of three files, with a Files section: results.png, notes.txt and export.zip, each with its size and type, a download button and a Turn 1 button." width="720">
</picture>

*The mock's `files` and `file-image` answers, from the Reviewer, and the panel's Sources tab (Files).*

| Piece | What it does |
|---|---|
| What is a kept file | `parseArtifact` reads `href`, `sha256`, `size`, `filename` and `preview` only when the `href` is exactly `/api/threads/<id>/artifacts/<sha256>` and says the same hash as `sha256`, and the size is a count of bytes; anything else is **not** a kept file and none of the reference is read. The agent's `uri` is never fetched. A file that was not kept (no `href`) is the old card, with its words and no download; the `error` that follows it (the file is too large, the job's limit, could not be kept) is the usual error line |
| The card | The **file name** (the artifact's name without one), the **size** (`1.5 KB`) and the type the worker sniffed, a **Download** link to `href?download=1`, and the preview. Every string is the agent's, drawn as text, with no control or direction characters. A file reported twice in a turn (the same hash) is one card |
| Preview `image` | `<img src=href>` with the file name as `alt` (else "File from the agent"), `loading="lazy"`, no referrer. **Never inline markup**, so an SVG runs no script and loads nothing (`e2e/files.spec.ts` draws one written to do both, from the mock's `file-svg`); an image the browser cannot decode is a line that says so, and the download stays |
| Preview `text` | `readTextPreview` fetches `href` and reads the first **64 KiB** only (the stream is cancelled after that), decoded as UTF-8, drawn in a scrollable `<pre>` (React text: markup shows as characters) with a line that says how much was cut. A file that cannot be read is a line, and the download stays |
| Preview `null` | The card alone: an archive, a PDF, anything else |
| The Sources tab | Every kept file of the thread once per hash, under **Files**: its name as a link that opens it (inline for a preview type, an attachment otherwise), its size and type, a download button and the turns that cited it |
| `Image` | `{id, artifact, alt, caption?}`: the `artifact` is the SHA-256 of a file of **this** thread (the schema has no member that can carry a URL; `alt` is required). `prepareSurface` is given the thread's kept files (`useThreadFilesList`, from the transcript) and refuses the whole surface, rule `artifact`, for a hash that is not one of them or not an image; with no files it refuses every `Image`. The component looks the file up again (`ThreadFilesContext`) and draws `<figure><img src=href alt=…><figcaption>`; a file that is missing is a line, never a broken image |

```mermaid
sequenceDiagram
  autonumber
  participant L as Log (vymalo.artifact)
  participant V as parseArtifact, filesOf
  participant C as File card, Sources tab
  participant P as prepareSurface
  participant I as Image
  participant A as API (getArtifact)
  L->>V: kind file, href, sha256, size, preview
  V->>V: href is the API's route for that hash? else not a kept file
  V->>C: the kept file
  C->>A: img src=href (preview image), or fetch href (preview text, 64 KiB)
  L->>P: a surface with Image {artifact: sha256, alt}
  V->>P: the thread's kept files
  P->>P: the hash is one of them, and an image? else refuse the surface (rule artifact)
  P->>I: the lowered component
  I->>A: img src=href of the file it found, never a URL
```

```mermaid
stateDiagram-v2
  [*] --> Reference: a vymalo.artifact of kind file
  Reference --> NotKept: no href (an error line says why)
  Reference --> Kept: the href is the API's route for its hash
  Kept --> Picture: preview image (img src)
  Kept --> Text: preview text (first 64 KiB, as text)
  Kept --> CardOnly: preview null
  Picture --> Broken: the browser cannot decode it (a line, the download stays)
  Text --> Unreadable: the fetch failed (a line, the download stays)
  Kept --> Placed: an Image of a surface names its hash
  Placed --> Refused: the hash is not an image of this thread (the whole surface)
```

The mock plays it: `file` (the golden's PNG), `file-image` (a chart placed in an answer), `file-image-foreign` (a hash the thread
does not hold), `files` (an image, a text file and an archive), `file-svg` (an SVG written to run a script) and `file-lost` (a file
the store did not keep): see the table below. The mock serves each as `getArtifact` does, with its headers.

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
                               thread-description.tsx (the description under the top bar and its field),
                               hooks/use-thread-edit.ts (renaming and describing the open thread: use-rename-thread,
                               use-describe-thread), hooks/use-ui-config.ts (`GET /api/config`, once per page load),
                               cards/ the pull request and file cards, parts/ callouts and text,
                               surface/ the A2UI renderer), hooks (runtime,
                               thread details, use-turn-steps: the thread's turns and their trees),
                               lib/agui (ThreadAgent, SSE reader, live runs, the vymalo vocabulary,
                               live-drafts: the words of a reply as they are written, pure),
                               components/live-drafts.tsx (the drafts' provider and a draft),
                               lib/steps.ts (what a turn draws as a step, a card or prose),
                               lib/working.ts (which text is the answer and which is working text, pure),
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
src/features/threads/          forking (ADR 0029): hooks/use-fork.ts (`POST /fork`, the page's own fork id, `turn_open`
                               in words), components/fork-provider.tsx (the open thread's fork actions: whether a
                               turn is open, "fork from here" on a run, "continue with" an agent; the error line),
                               components/fork-divider.tsx (the `vymalo.fork` marker as a divider), components/branches-provider.tsx and
                               hooks/use-branches.ts (the open thread's branch points), components/branch-picker.tsx
                               (`‹ n/m ›`), hooks/use-scroll-to-message.ts (`#m-<seq>`); the editor itself is
                               components/assistant-ui/elements/message-editor.tsx; thread list: collapsible sidebar (desktop) and sheet (phone), grouped by
                               recency (lib/recency.ts, local calendar days of `updatedAt`), a row's description in a hover card (ui/hover-card.tsx), paging
                               hook; lib/sidebar-state.ts: the remembered open or closed sidebar and
                               the head script that hides a closed one before the first paint.
                               Paging goes by creation (`before=<id>`, UUIDv7) while the groups
                               go by the last change, so an old thread touched today can sit under
                               Today on a later page: known, and left as is
src/features/agents/           the new chat: greeting, suggestion chips; the agent picker (agent-menu.tsx, a menu
                               button in the top bar: the agents, then the chosen agent's releases; on a thread another
                               agent or release asks "Continue with … in a new chat?" and forks); agents hook; lib/selection.ts (which agent
                               and release a new chat sends to, `?agent=`); the agents hook reads `/api/agents` and
                               `/api/registry` together, on mount, when the picker opens and when the window gets the
                               focus back (at most every 5 s), keeping the list on screen while it refreshes;
                               registry-notice.tsx is the line "The agent registry is unreachable; showing the configured
                               agents only." (with Retry) under the greeting of a new chat and in the picker's menu
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
[`e2e/__screens__/`](e2e/__screens__/) (`e2e/screens.spec.ts`); look at them after a visual change.
**The docs embed these files** (the repository README, this file, [DESIGN.md](DESIGN.md) and
[`dev/README.md`](../dev/README.md), each as a `<picture>` of the `-light-` and `-dark-` file of a state). A screen that is
renamed or dropped breaks a doc: `tools/docs-check` fails on an image path that does not resolve, in `![](…)` and in the
`src` and `srcset` of an HTML tag, so rename a screen with a search of the repository for its name. A screen is a still of the mock,
never of a real agent: say so in a caption. To add a primitive
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
validates every response and every AG-UI frame against the contract and the vendored AG-UI schema. The mock plays `forkThread` and `listBranches` (and the list's `branches=include`), and its projection reads the fork goldens (`fork`, `fork-blocked`: the copied events, `thread_forked`, the fork's own life) and tells the golden stream (`mock/golden.test.ts`).
The first word of the first message picks the script, the same words as the orchestrator's fake agent:

| First word | Behaviour |
|---|---|
| anything else (`echo`) | working, artifact `echo: <text>` with a PR link, done |
| `ask` | asks "Which branch?", blocks (the run ends in an interrupt); the `resume` answer runs to an `answered: <text>` artifact and done |
| `slow` | works until cancelled (`RUN_FINISHED` cancelled) |
| `fail` | a failed status with detail `scripted failure`, `RUN_ERROR` `agent_failed`, no error activity |
| `talk` | working, a status with text, one final agent message, the result, done |
| `ui` | working, an A2UI surface (a title and a `Go` button), the question "Pick one", blocked; the action on the surface (`forwardedProps.a2uiAction`, not a message) resumes it to `answered: ui-action go` and done |
| `describe`, `describe-long`, `Plan` | mock only, descriptions (ADR 0035, the `description` golden): the `talk` script, then, a step after the thread is done, the orchestrator's model describing it (`thread_described`, `source: model`): one sentence (`describe`), or three, longer than a line (`describe-long`, and `Plan …`, which is it in plain words for the screenshots and the e2e). A description a person wrote is never replaced by it. `PATCH /api/threads/{id}` takes `description` as well as `title` (both are checked before either is written; an empty one clears it, which the golden's last event is), `GET /api/config` answers `{ui: {showDescriptions}}` (`POST /__mock/config?showDescriptions=false&session=<name>` switches it for one session, named by the `mock-registry` cookie as the registry's is), and a fork has its parent's description |
| `title` | a rename (`PATCH /api/threads/{id}`, the `title` golden): the thread is renamed while it works, cancelled, and renamed again; a rename inside a run is a `STATE_SNAPSHOT`, one of a finished thread a run of its own that holds only a snapshot, which the web never hands to the runtime (the `connect-title` golden through the runtime adds no message). The mock's replay says the title the thread has when the viewer connects (the golden's starts from the first message's words) |
| `steps`, `steps-ask` | nested steps (ADR 0025, the `steps` and `steps-ask` goldens): a sub-agent step `OpenCode` (a subagent of the run, `sub-step-<seq>`) with a command `npm test` under it that fails (`1 failed`), the sub-agent's end, the agent's words and done; in `steps-ask` the command is `waiting` when the agent asks "Allow rm -rf build?" (the step's subagent suspends with the invocation) and the answer ends the command and the sub-agent. The mock's step ids carry the placeholder `T` where the orchestrator has the A2A task id. The web reads them into a tree per turn ([The step tree](#the-step-tree)) |
| `verify-pass`, `verify-red-once`, `verify-red` | the verification gate (3 attempts, the agent's own checks; the orchestrator's fake agent has the same words): checks pass at once; fail once, are sent back and pass on attempt 2 (`verify-green` golden); fail every time and end `RUN_ERROR` `checks_failed` (`verify-red` golden). `job` is on every snapshot and on `GET /api/threads/{id}` |
| `verify-reviewed`, `verify-reviewed-red` | the verifier agent of the gate (3 attempts, the `verifier` source, verifier `verifier`): it finds something in attempt 1, the agent is sent back, and the verifier passes attempt 2 (`verify-verifier-green` golden); `-red`: it never passes it and the run ends `RUN_ERROR` `checks_failed` (`verify-verifier-red`; the golden's message is `verify-reviewed`, the word only tells the two apart). The verifier is a subagent of its own, `sub-verify-<n>`, open from its pending card to its verdict (`result: {passed}`), and it is told to a client that joins |
| `verify-reviewed-wait` | mock only: the verifier is asked and never answers; its subagent stays open (and is in the preamble of a client that joins) until Cancel ends it as `canceled`. The mock does not play the holds of a timed-out or unreachable verifier (`SUBAGENT_ERROR`) |
| `verify-ci` | CI (ADR 0017, the `ci` golden): a gate on CI alone; the pending check, a red `vymalo.ci` report (`ci/build`, commit 1) that fails it and sends the agent back, then a green report for commit 2 and done |
| `verify-ci-stale` | mock only, **not produced by the current orchestrator**: a gate on CI and the agent's checks; the CI check is pending, then a late failed report of an older push (its own `vymalo.ci` step) with a stale answer, then a green report and the answer that passes (a check replaced in place, a check that stands apart) |
| `verify-wait` | mock only: the same gate, CI never answers; the thread stays `verifying` (a pending check, no CI report) until it is cancelled |
| `choices` | working, a surface of our catalog with a Choices of three questions (a database with an "Other", a login, where it runs: several, optional, with an "Other"), the question "Three questions", blocked; the answers (`forwardedProps.a2uiAction`, `context.answers`) resume it to `answered: ui-action answer db=pg auth=none deploy=k8s,compose` (what was chosen, in question order, `other:<words>` for their own) and done |
| `cards-mermaid` | mock only: one answer of the agent's words, a surface of our catalog with three `Cards` (two linked, one not) and a `Mermaid` flowchart, then done (no result artifact) |
| `cards-bad`, `cards-bad-url` | mock only: a `Cards` that breaks its schema (a card with no title and a `javascript:` link: rule `schema`), and one whose link passes the schema and not the URL rule (user information: rule `url`); the surface is refused and not drawn |
| `mermaid-bad`, `mermaid-hostile`, `mermaid-kinds` | mock only: a `Cards` beside a graph that does not parse (the graph says so, the cards are drawn); a graph that tries to switch its security off (front matter, a directive, HTML and a script in labels, a click handler and a link), drawn as a plain image; ten graphs, one of each common kind |
| `file` | the `file` golden ([`file.events.json`](../docs/api/examples/file.events.json)): working, one artifact that is a file the store keeps (`chart.png`, the golden's one-pixel PNG, as `file`), done; served at `getArtifact` like the others |
| `file-image` | mock only: the agent's words, a kept file (`results.png`, a 480 x 240 bar chart) and a surface of our catalog with an `Image` of it by its hash, with `alt` and a caption, then done (the card of the file and the picture in the answer) |
| `file-image-foreign` | mock only: an `Image` that names a hash this thread does not hold: the whole surface is refused, rule `artifact`, and nothing is fetched |
| `files` | mock only: the agent's words and three kept files, an image (`results.png`), a text file (`notes.txt`, with markup in it) and an archive (`export.zip`): a card each, the picture and the text shown, the panel's Files list |
| `file-svg` | mock only: a kept SVG (`diagram.svg`) written to run a script, a handler and to load a stylesheet, an image and a frame from another origin (the real API cleans it; the mock serves the original): drawn as an `<img>`, it does and asks for nothing |
| `file-lost` | mock only: an artifact the store did not keep (no `file`) and the error "the file is too large to keep": the old card, no download, the error line |
| `catalog-newer` | mock only: the thread was opened by a newer version of the app (its UI catalog is version 99, whatever the web sent) and the agent sends a surface of our catalog with a component this build lacks (`Gizmo`): the placeholder that asks for a newer version; the result and done |
| `catalog-unknown` | mock only: the same surface in a thread whose catalog is this build's: refused with the rule `catalog`; the result and done |
| `sources` | mock only: the coder's steps, a branch, a pull request, a CI report with a link, and a final answer with links in it (the pull request again, a doc, a run, a URL inside code, a `javascript:` one): what the panel's Sources tab lists; done |
| `partial` | mock only, **not produced by the current orchestrator**: a partial agent message replaced by its final version |
| `unreachable` | mock only: an error activity, `RUN_ERROR` `delivery_failed`, thread blocked |
| `stream` | the `stream` golden ([`stream.feed.json`](../docs/api/examples/stream.feed.json), live text, ADR 0027): working, the reply `Fibonacci in Rust.` as three live pieces (`Fib`, `onacci `, `in Rust.`: frames with `vymalo.live`, no `id:`, not in the log), then the log's message under the same id, the status that repeats the words, done. A viewer must be connected while the pieces are written to hear them, as with the orchestrator |
| `stream-long`, `stream-hold`, `stream-gate`, `stream-abandon` | mock only, live text: a reply in Markdown (a paragraph and a list) written in eight pieces, then the log's message and done (`stream-long`); the same, five pieces and then nothing until cancelled, so a draft stays on the screen (`stream-hold`, and `Write …`, which the screenshots use); five pieces and then nothing until the test releases it (`POST /__mock/release?thread=<id>`), then the other three, the log's message and done (`stream-gate`); a stream the model gives up halfway, once the test has released it, then the words the agent says next under another id (`stream-abandon`). The mock says the text so far again from its start every second (`refreshMs`, as the orchestrator's sender does), so a page that opens or reconnects mid-reply is told the draft a moment later: **a test that must see a draft after a reload or a cut holds the reply (`stream-gate`, `stream-abandon`) rather than race a script**, because a draft is on the screen only for as long as the script has left (about four seconds for `stream-long`, less than the page's reload takes on a slow phone run) |
| `stream-words` | the `working` golden (ADR 0031): the words before a tool call arrive piece by piece and are stated as an `agent_message` with `purpose: working`, a command `npm test`, then the reply the same way with `purpose: answer`, the status that repeats it, and done. The projection says it on each `START` (`metadata["vymalo.purpose"]`) and the live overlay ends the working message's draft with `{final: true, purpose: "working"}`; the web keeps the reply in the chat and files the working sentence in Activity ([The answer and the working text](#the-answer-and-the-working-text)): `chat-shell-live.dom.test.tsx` and `e2e/answer-view.spec.ts` |
| `coder-notes`, `coder-notes-legacy`, `coder-notes-hold`, `coder-notes-running` | mock only, working text and the answer (ADR 0031), the owner's coder chat of 2026-10-02 in its shape and in other words: six sentences said before tool calls, ten tool steps (a test run fails, one is `show`), a surface drawn on the way (two cards) and one answer, done (`coder-notes`; `Draw` is the same in plain words, for the screenshots); the same turn with no word marked, as an older log or a plain A2A agent says it, which the screen reads by its rule (`coder-notes-legacy`); the first five sentences and the steps between them, working until cancelled, so the line shows its ticker (`coder-notes-hold`, `Sketch` for the screenshots); one unmarked sentence and then nothing until the test releases the run, which shows as a draft of the answer, folds when a step starts after it, and ends with the words that are the answer (`coder-notes-running`). `e2e/answer-view.spec.ts`, `chat-shell-answer.dom.test.tsx` |
| `turn-output` | the `turn-output` golden (ADR 0031, the amendment): working, a sentence stated as an `agent_message` with `purpose: working`, a command `npm test`, then the answer the agent announced with the `turn_output` tool (`purpose: answer, via: turn_output`), the closing line as a `purpose: working` message (the core writes the words of a status that ends a turn that announced its answer as working text), the status that keeps it, and done. The projection says each in `vymalo.purpose` and `vymalo.via` on the `START`. The rule that **the answer of a turn is the last message marked `answer`** (a later `turn_output` replaces the earlier) is the screen's, not the mock's |
| `steps-io` | mock only, what a tool step can carry (ADR 0030): a `search__web_search` step with its input (on its start) and its output (on its end), a cut output (`truncated`, `bytes`), an input too big to keep (`{_cut, bytes}`), a failed command whose output is its error (`error`), a step the job's budget had no room for (`ioDropped`), and a step with none; the answer and done. The `steps` golden's `npm test` and the `Delegate` scenarios' reads, search and failing test run carry input and output too; the mock's projection says them as the orchestrator's does (the input again on the step's end). The web opens them ([The step tree](#the-step-tree)): `e2e/step-io.spec.ts` |
| `Delegate`, `Investigate`, `steps-many` | mock only, nested steps at a scale the goldens do not have (`quick` steps play at once): the coder hands the work to OpenCode, a sub-agent step with fourteen steps under it (reads, a search, edits, commands, one test run that fails and is run again), then a push, a pull request and the answer (`Delegate`); the same still running a command until cancelled (`Investigate`); a sub-agent step with 120 reads under it, one of them failing, played at once: a level long enough to be a scroll box (`steps-many`) |
| `Fix`, `Refactor`, `Make`, `Upgrade`, `Deploy`, `Also`, `Migrate` | mock only, the coder scenarios of `pnpm screens` (plain words, so the titles read well): steps with commands, a push, the agent's checks, a pull request and a markdown answer (`Fix`); the same, still running a command (`Refactor`); a failed check, a rework and a pass (`Make`); nothing after the message (`Upgrade`); a question (`Deploy`); a short follow-up (`Also`); a failure with the agent's reason (`Migrate`). The wording of the steps is the mock's, not adam-coder's |

The mock tells the orchestrator's story: `mock/golden.test.ts` drives every scenario of
[`docs/api/examples`](../docs/api/examples/README.md) through the mock's run route and requires the
connect stream a viewer reads to be the golden `agui/<name>.agui.json`, frame for frame, so the mock
cannot drift from the orchestrator unnoticed. Test hooks for the e2e suite: `POST
/__mock/drop-streams` (cut every open stream; with `?thread=<id>`, only that thread's, which is what a
test that runs in parallel with others uses), `POST /__mock/release?thread=<id>` (let the run of that
thread go on from a `{ pause: "release" }` step, `stream-gate` and `stream-abandon`; 409 when it does not
wait), `POST /__mock/cut-next-connect?frames=n` (cut the next connect stream after `n` frames, in the
middle of a group) and `POST /__mock/reset`. The mock also plays
the platform's agent registry (ADR 0022): `GET /api/registry`, `POST /__mock/registry?down=true` makes it
unreachable (its agents leave `/api/agents`, a run or a capabilities request for one is a 503 with `Retry-After`) and
`POST /__mock/registry/agents` with an agent as the body adds one (`source: "registry"`). Its state is kept per
session, named by the cookie `mock-registry` (or `?session=` on a hook; none is the `default` session), so that tests
running in parallel against one mock do not see each other's registry; `POST /__mock/reset` forgets every session's.

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

| Command | What | Count (2026-10-02) |
|---|---|---|
| `pnpm test` | vitest: the agents hook (`use-agents.dom.test.tsx`, against the mock: the list and the registry's state read together, an agent the platform adds shown on the next read, a registry that is down flagged with the configured agents kept, an older orchestrator without `/api/registry` losing nothing, the list kept on screen while it refreshes and when that fails, a refresh on focus held back to every five seconds) and the registry notice in the picker (`agent-menu.dom.test.tsx`: nothing while every source answered, the line and a Retry item when one did not, on a new chat and on a thread, an agent's tags); the SSE reader; `ThreadAgent` (groups, reconnect with `Last-Event-ID`, dedupe by seq, acceptance at `RUN_STARTED`, problems, abort is not cancel); the AG-UI goldens through the patched runtime (messages, activities, interrupts, the cancelled outcome); the app in jsdom against the mock (new thread, replay, answer, refused send, cancel, not found, a surface and its action); **the A2UI validator and its security tests** (every bad URL scheme and trick, each limit at and one over, an expansion bomb and a reference bomb, the vocabulary, reserved names, function values, inputs), **the renderer** (the golden through the runtime, replace in place, delete, a refusal, a later run, no auto-send under fake timers, a click sends once, disabled states, `openUrl` as a link, `userMessage` in the composer unsent, no image ever) and **the app's side of an action** (what a click puts on the wire, with and without an open interrupt); **the verification gate** (the parsing of `vymalo.check`, `vymalo.rework` and `job`: malformed payloads draw nothing and unknown fields are ignored; the check and rework steps and the badge; findings that are markup or markdown shown as text, long ones cut and expandable; `verify-green` and `verify-red` through the runtime and through the whole `ChatShell` against the mock: the badge goes verifying, queued, working, verifying, done or failed, no attempt counter in the header, the check and rework steps in order ("Checks failed — trying again (2/3)"), "Checks failed after 3 attempts", a reconnect mid-verification and a fresh page showing the same); **CI results** (`parseCi`: every required member needed, unknown fields ignored, a url that is not http(s) dropped; the CI step for every conclusion with its words, icon and tone, an unknown conclusion, a missing url, a `javascript:` url handed straight to the step, markdown and HTML in the name, branch and summary shown as text, a long summary and name cut; the `ci` golden through the runtime and through the whole `ChatShell` against the mock, a step replaced by whatever id the wire gives it, malformed payloads drawing nothing); **the turn** (what is a step, a card or prose, `lib/steps.ts`; the pull request and file cards; the thread list's recency groups); **Export JSON** (the file name the server gives and never a path, the download through the API client, a refused export saying why and downloading nothing, the mock's export against the contract); **A thread's description** (`chat-shell-description.dom.test.tsx` against the mock: the model's description under the header and as the sidebar link's description, one that arrives after the thread is open drawn and cleared with no reload, plain text whatever it holds, Add description with the field focused and Enter saving through the API, an empty text clearing it, Escape and the same text sending nothing, a refused save keeping the field, Rename unchanged beside it, `ui.showDescriptions` off leaving no line, no description and no menu item; `thread-description.dom.test.tsx`: the Show more control only when the line cuts the text and following the width, `aria-expanded`, Markdown and markup as typed, the field's keys; `thread-sidebar.dom.test.tsx`: the link's description, the hover card by focus and by pointer, in plain text; `use-ui-config.dom.test.tsx`: read once, defaults, off until known, tried again after a failure; the mock's `PATCH` description, `GET /api/config`, the model's description not replacing a person's and the `description` golden through the server, in `mock/`); **Rename** (the title becomes a field that has the focus, Enter saves through the API and the header and the sidebar say it, Escape sends nothing, leaving the field saves once, the same or an empty title is no request, a refused rename says why and keeps the field; the mock's `PATCH` against the contract; a run that holds only a snapshot never reaches the runtime, in `ThreadAgent` and through the goldens); **the UI catalog** (the canonical JSON and the known-answer digest the orchestrator pins too, the lock against `catalog.json` and against the released versions, the catalog document's own rules, every send rule of `ThreadAgent`, the validator's catalog and schema rules, the placeholder for a newer catalog and the refusal when it is not newer); the mock against the contract and the goldens (`a2ui`, all four `verify-*` and `ci` included, the verifier's subagent among them) and its record of the UI catalog; **Choices** (the schema's limits at and one over, the two rules a schema cannot say, the reserved action name, the lowering through the converter, what is sent and in what order, `Other`, the gate on required questions, no send by drawing, choosing, typing, Enter or time, read-only when the thread does not wait or the copy is not the newest, a question id of `__proto__`; **the person's answers**: the shape of an answer, the labels resolved from the surface in the transcript and the raw ids and values without it, the bubble above the agent's mark and not a step, a button's action still a step, text only); **Cards and Mermaid** (both schemas' limits at and one over, a card's link by the schema and by `safeHttpUrl` with the bad-URL table, the validator's rules for both, the component for a newer thread; the cards drawn as text with their links and hosts and nothing fetched, a refusal that names the card; the graph drawn through a stand-in for mermaid: the strict configuration, the alt text, the source disclosure, loading, a parse error, a refused SVG, a broken image, a change of scheme, a later copy; **the pinned mermaid against hostile directives and front matter**, with a control that shows what mermaid's own `secure` list lets through; `svgImage` on scripts, handlers, loading CSS, entities and the viewBox; mermaid imported by one file only); **the brand** (`components/brand/`: the SVGs use only the three brand fills and no image, script or link, within 16 KB; the PNG and ICO sizes; the manifest's icons exist; an agent avatar's initial, stable tint, `aria-hidden` and 4.5:1 contrast in both schemes); **the agent picker** (`agent-menu.dom.test.tsx`: radio semantics, the check on the chosen agent, the releases as a group of the same menu, a refetch on every open, a failed refresh, the notice slot, the keyboard: Enter, Space, arrows, Escape returning the focus; on a thread another agent or release asking "Continue with … in a new chat?" before it forks, cancelled by Cancel, disabled with its reason while a turn runs; `lib/selection.test.ts`: which agent and release a new chat sends to, `?agent=`); **the step tree** (`lib/step-tree.test.ts`: the tree of a turn, numbered as the Sources tab numbers turns, nested by `path`, a step whose parent is not in the turn under the agent, a step said again updated in place (a retry too), an unknown kind or icon, a report that does not validate drawing nothing, today's activities as leaves with a command as a command step, the gate's nodes after the agent, a pending check spinning only while the run is open, the state of a turn (running, verifying, paused, failed, stopped, a turn that ended on a question staying paused), no running step in a turn that is not running, the turn's times, the summary's counts and the step it is on, `summaryLine` for every state, which children a level lists (the failed always), durations, and a turn rebuilt only when its message or the thread's state changed; `parseStep` in `vymalo.test.ts`; the `steps` and `steps-ask` goldens, and their connect variants, through the runtime and into a tree; `components/steps/turn-summary.dom.test.tsx`: the line, its name and chip for every state, the spinner only while running, a click opening the panel on the turn, `aria-expanded`; `steps-pane.dom.test.tsx`: collapsed by default, a click then "Show 10 more", the failed ones kept in view, the failure chip at every collapsed level, a level of 50 plain and one of 120 a window of rows, which turns are listed and open, following the live turn until the person chooses, a request to show a turn opening, scrolling to and focusing its header once per key; `steps-panel-content.dom.test.tsx`: the connected forms over the runtime playing the goldens; `chat-shell.dom.test.tsx`: a turn with nested steps is one line in the chat and no list, the line opens the panel on its turn, and each turn's line focuses its own; **a tool step's input and output**: `parseStep` reading `input`, `output` and `ioDropped`, a member of the wrong shape dropped and the step kept, `inputCut`; `lib/step-label.test.ts`: `server__tool` in words and any other label left alone, the argument a row quotes, sizes; `step-tree.test.ts`: the input kept across a step's reports, a failed `run_checks` counted once and a red artifact with no failed step counted, `pathTo` and `firstFailed`; `components/steps/step-io.dom.test.tsx`: the native button and its `aria-expanded` and `aria-controls`, Input then Output, a list or pretty JSON, an input that was cut, a cut output, Error in place of Output, the `ioDropped` note, no button for a step with nothing, markup in an argument, a result or an error shown as text and never run, a sub-agent opening its children and its block together, a request to show a step opening the way and focusing it; `expansion.test.ts`: opening the way to a step; the chat's failed chip in `turn-summary.dom.test.tsx`: a button beside the line's, calling `openSteps(turn, step)`); **live text** (`lib/agui/live-drafts.test.ts`: which frames are live, a draft opened by a `START` and grown by the pieces of the `stream` golden, an overlap, a repeat, a gap, an emoji's two UTF-16 units, an id that is not open, the bound, an abandoned `END`; the log's message taking a draft over: the plain message the runtime reads, made of the draft up to `offset` and the rest, a final that replaced the text, `offset: 0` with no draft, a final that continues text the connection never held, the other frames of the group in their place; what a completed draft draws until the transcript has its words. `ThreadAgent`: a piece shown at once with no `id:` to wait for and never in the run or a resume point, the log's message completing the draft and the next group dropping it, the runtime reading the same events with or without live text, a group it cannot tell whole not delivered and the connection reopened at the last resume point without an error, a cut connection forgetting its drafts, a given-up reply, an abandoned `END` inside the group that closes the invocation, the end of a run clearing the drafts, a live frame that carries an `id:` anyway. The `stream` golden through the runtime: nothing of the reply in the transcript while it is a draft, one reply after it, no draft left. `components/chat-shell-live.dom.test.tsx`: the whole app against the mock, a draft that grows as Markdown, `aria-busy`, the one reply that replaces it, Stop, a given-up stream, a connection cut mid-reply, a page opened mid-reply told the text so far by the sender's refresh, a thread opened after the reply reading it plainly. `mock/live.test.ts`: the mock's overlay on the rules of the real one, and `mock/golden.test.ts` the `stream` golden); **the panel** (`features/panel/`: `lib/sources.test.ts`: every kind of source, only http(s) links, a URL in code never read, one item per URL with every turn that cited it, a typed source over the same link in words, the groups and their order, numbering the turns as the chat does; `lib/panel-state.test.ts`: where the panel docks and the widths it may have, what is remembered, and the head script run against the same rules, storage blocked; `hooks/use-panel.dom.test.tsx`: the defaults by window width, the shortcut on every combination of keys, the remembered choice, a sheet that is for the visit only, the `useStepsPanel()` contract of the step tree; `components/thread-panel.dom.test.tsx`: the landmark, the tabs and their arrows, the separator's keyboard and pointer, the focus on close, the sheet from the right and the bottom, a Turn button; `sources-tab.dom.test.tsx`); **forks** (`ThreadAgent`: the event each person's message came in and the last event of each run, across jobs, a cut connection and a fork's own run; the actor marker carrying its `runId`; the fork goldens, and a blocked thread's, through the runtime: the marker one message of its own and the copied question closed; `chat-shell-fork.dom.test.tsx`: Fork from here making a thread that ends where the turn does and going to it, disabled while a turn runs and in the agent menu with its reason, enabled once stopped, an earlier turn forked while a later one runs, `turn_open` and another refusal in words with Dismiss, the divider linking to the parent and saying "continued with" another agent, a fork of a thread that waited for an answer taking an ordinary message, the fork marked in the list and an edit not listed; **branches** (`message-editor.dom.test.tsx`: the editor opens with the caret at the end, Escape cancels, Ctrl or ⌘ with Enter sends, a plain Enter and a composing Enter send nothing, an empty message is not sent, Send waits while the fork is made; `branch-picker.dom.test.tsx`: the count from 1, the first ‹ and the last › disabled and never wrapping, the live region in words; `chat-shell-branches.dom.test.tsx`: the pencil, the editor in the bubble and Escape, Ctrl+Enter making an edit and going to `#m-<seq>`, a refusal as "Could not edit the message" with the words kept, 2/2 and 1/2 with the arrows going to the other thread, no picker on a thread never edited or one whose branches cannot be read, the edit left out of the list and the conversation's first thread highlighted); `mock/server.contract.test.ts`: the mock's `forkThread` and `listBranches` against the contract, the cut, the target, the repeat by id, `turn_open`, every documented problem, the list flag); **the answer and the working text** (`lib/working.test.ts`: the mark wins; unmarked text in a turn that is over, the last is the answer and a marked answer wins over a later unmarked text; in a turn that runs an unmarked text is a draft of the answer until a step starts after it, a status that says what the agent does folds it, an artifact does not; the ticker line plain, cut and empty for words that say nothing. `lib/step-tree.test.ts`: notes in time order among the steps, not counted, not the step a turn is on, "3 notes" for a turn with none, the ticker, the unmarked rule. `lib/agui/live-drafts.test.ts`: the purpose an `END` says, the plain message's `START` saying it, an `END` that says only `final` leaving the draft the answer, a working draft drawing nothing. `runtime-goldens.dom.test.tsx`: the `working` golden through the runtime with the marker before each text, and the `stream` golden ended as working. `components/steps/working-notes.dom.test.tsx`: the note rows (a screen reader's "Working note:", whole in the page, clamped with Show more, text and never markup, a turn of notes listed, a turn of its answer only not) and the ticker (the last line, muted, one line, not a live region, not a control, gone when the turn is over). `components/chat-shell-answer.dom.test.tsx`: the whole app against the mock on the owner's chat, marked and unmarked: one answer in the column and none of the six sentences, the surface kept, the notes in Activity in order, Copy copying the answer, a question as the answer, a plain agent's one message, an unmarked sentence a draft until a step starts and then a note, the ticker while a turn is held. `chat-shell-live.dom.test.tsx`: a live draft that ends as working text leaving the column) | 1241 (68 files) |
| `pnpm test:e2e` | Playwright on the production build against the mock: the files an agent made (`files.spec.ts`, desktop and phone: the cards of an image, a text file and an archive with their names, sizes and downloads, the picture decoded and the text drawn as text, the download saving the file under its name, the Sources panel's Files, an SVG written to run a script drawn as a picture that does and asks for nothing, the catalog's `Image` placing the file in the answer, a hash the thread does not hold refused, a file that was not kept with its error and no download); the answer and the working text (`answer-view.spec.ts`, desktop and phone, on the mock's `coder-notes` scenarios: one answer in the column and none of the six sentences said on the way, the surface kept, the sentences as notes in Activity in the order they were said, the same for a log with no word marked, one answer for every turn of a conversation, nothing focusable behind anything hidden in the column, the ticker of a running turn quiet and one line and never a live region, the line and its failed chip not squeezed by it, a streamed sentence filed as a note; axe on the finished turn with its notes and on the running turn with its ticker, both schemes, reduced motion); the agent registry (`registry.spec.ts`, desktop and phone: a registry that cannot be read is said under the greeting and in the menu with the configured agents still there, and the line goes on Retry or when the picker opens after it came back; an agent the platform adds is in the picker the next time it opens, without a reload, with its tags, after the configured ones; a message goes to it; axe with the notice, light and dark; each test has a registry of its own in the mock, by cookie); axe (no serious or critical issue, light and dark; new, finished and blocked thread, and a thread with an A2UI surface waiting, finished and refused) and Lighthouse accessibility >= 95 in both schemes; create, follow-up, cancel, failure, releases, API errors; reconnect (a dropped stream, a cut in the middle of a message); two tabs; A2UI (a surface is drawn and only a click sends the action, read-only after the thread finishes and after a reload, a refusal, no remote content); the UI catalog (the first run of a thread carries it whole and a later one does not, the placeholder of a thread opened by a newer app, kept after a reload and never answered with a catalog, the refusal when the thread is not newer; axe on the placeholder, both schemes); Choices (three questions answered by clicking: the button waits, one action goes out with no message and no `resume`, the bubble with the labels, no step, the agent's echo, the Choices read-only and the same after a reload; "Other" as the person's own words; the keyboard alone, arrows and space, Enter in a text box sending nothing; on a phone too; axe unanswered, answered and after sending, both schemes); Cards and Mermaid with the real mermaid in Chromium (one turn of words, three cards and the picture of the graph, links in a new tab with their hosts, the source one click away, nothing requested outside the app, the same after a reload; mermaid loaded when a graph is drawn and not with every thread; the graph in the colours of the light page and of the dark one; a card that breaks the schema, and a link that breaks only ADR 0013's rule, each refused with its rule and the raw operations; a graph that does not parse beside cards that are drawn; a graph that tries to switch its security off, drawn as a plain image with no script run, no link and no request; ten kinds of graph all drawn; on a phone too; axe on the answer with the source open, on the graph error and on the refusal, both schemes); verification (sent back and done on attempt 2 of 3, `Checks failed after 3 attempts`, a pending check while verifying that survives a reload and ends with Cancel, a verifier agent's findings and pass, a pending check of the verifier that ends with Cancel, no counter without a gate; axe on a thread being verified, on one waiting for its verifier, on a stale check and on failed checks, both schemes); CI results (a red report and a green one as steps with their links, a late report of an older push, no card while CI has not answered; axe on the CI cards, both schemes); Export JSON, from the thread's menu (the thread downloads as `thread-<id>.json` with its events, and a failed export is said and leaves the page usable); nested steps (a delegation to OpenCode is one line in the chat, with its failure chip and its name for a screen reader, and the tree in the panel: depth 1 listed, the sub-agent one collapsed line with its count and its failure chip, a click then the latest three and the failed one, Show 10 more twice, closing it, the same tree after a reload; each turn's line focusing its own turn and again on a second ask; a level of 120 steps a scroll box that draws a window of the rows in Chromium; a running turn's line naming the step and spinning until Stop; axe on the tree open, on the scroll box and on a running turn, both schemes, on a desktop and on a phone's sheet); opening a tool step (`step-io.spec.ts`, the mock's `steps-io`, on a desktop and a phone: a tool named by its tool, its server and its query; a click, Enter and Space opening Input then Output with `aria-expanded` and closing it; the same block after a reload; a cut output saying "n KiB more not kept" and an input too big to keep saying so; a failed step showing Error, a step the record budget missed saying so, a step with nothing not a button; the chat's failed chip opening the panel on the failed step, inside a sub-agent too, with the focus on its button; axe with every step open, both schemes); the turn (a coder's steps in the panel, one line in the chat, a command, the push, the checks, the pull request card and its link, the markdown answer said once; the live step and Stop; "is starting…" before the first event; a rework step and the folded findings; the question waiting for a reply; a suggestion that fills the box and sends nothing; the sidebar closed, remembered and opened); the brand (the icons, manifest and head tags served, the title, the panda in the sidebar and over the greeting, an agent's letter and not the panda in a turn); the agent picker (the menu button in the top bar, the agents with their descriptions and the check, the keyboard, a click outside, the release group, the choice the first message goes to, on a thread the other agents as a new chat with `?agent=`, every control of the top bar on the screen on a phone; axe with the menu open, both schemes); live text (the words grow while the agent writes them, as Markdown, then the reply stays, once, with no draft left; Stop ends a draft; a stream the model gave up; a connection cut and a page reloaded mid-reply; the caret blinks, and stays on, still, under reduced motion; axe with a draft on the screen, both schemes, on a desktop and on a phone; Lighthouse >= 95 with a draft on the page, both schemes, with a desktop and a phone form factor); the panel (open by itself on a wide window and remembered, the shortcut from the message box, the tabs and their arrows, Sources listing the pull request, the branch, the run and the links in the agent's words once each and never a URL in code, a Turn button scrolling to and focusing the turn, the resize separator by keyboard and pointer, the sheet from the right and from the bottom with Escape and the focus back; axe with the panel open, both tabs and the empty one, both schemes); forks (`fork.spec.ts`, desktop and phone: Fork from here makes a new chat that ends where the turn does, with the divider, the parent untouched and its link back, the fork's marked row in the list; an earlier turn forks to its own end; the button disabled with its reason while a turn runs (a turn the mock holds until Stop) and enabled once stopped; the keyboard; another agent or another release in the menu asks first and Cancel changes nothing; a thread that waited for an answer forked and replied to; a `turn_open` refusal shown as a message; axe on the turn's actions, the question and the fork, both schemes); descriptions (`description.spec.ts`, desktop and phone: the model's description arriving on the open thread with no reload, one muted line that opens to the whole text and closes again with the keyboard, the description in the export, the sidebar row's hover card by pointer and by keyboard and the link's description, writing it from the menu with the keyboard and an empty text clearing it, a refused save keeping the field, `ui.showDescriptions` off leaving it nowhere, plain text whatever it holds; axe on the line closed and open, the field and the card, both schemes; each test that switches `ui` has a session of its own in the mock, by cookie); branches (`branches.spec.ts`, desktop and phone: edit the second message and the new chat shows 2/2 with the new answer and the focus on the message, the list showing the conversation once; ‹ goes back to 1/2 and the original, › to the edit; edit the first message; the keyboard alone (Escape gives the words back and the focus to the pencil, a plain Enter is a new line, Ctrl+Enter sends, Enter on an arrow); an empty message not sent and a refused edit saying so with the words kept; axe on the pencil, the editor and the picker, both schemes) | 304 pass and 10 are skipped (the panel's desktop tests have no phone and its phone test no desktop, one keyboard test, and the description's hover card on a phone); the Lighthouse test with a draft on the page needs a quiet machine and fails on a loaded one, with or without descriptions |
| `pnpm test:e2e:system` | the same UI against the real orchestrator, see below (Export JSON included: the downloaded file is the log the connect stream replays; forks: the fork's agent is told the conversation in a context of its own, `fork.spec.ts`; an edited message likewise, with the versions as siblings, `branches.spec.ts`) | 26 |

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

`e2e-system/*.spec.ts` cover forking (`fork.spec.ts`: Fork from here and another agent in the menu, then the fake agent's `recall`: its call journal shows one message that starts with the conversation, in the fork's own A2A context; `branches.spec.ts`: an edited message, whose agent is told the conversation before it and nothing after, and the two versions as siblings in `/branches`), the identity through the rewrite (and 401, user isolation, and another
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
