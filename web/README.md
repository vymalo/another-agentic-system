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
| `GET /api/me` | who the person is and what their roles let them do ([ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)): `user`, `roles`, `permissions` (each with its `scope` where it has one), the agents `agent.read` and `agent.invoke` cover. Read once per page load (`features/me/hooks/use-me.ts`), and **never a check**: the orchestrator enforces every request, the web only stops offering what would be refused; [Who you are and what you may do](#who-you-are-and-what-you-may-do) |
| `GET /api/config` | the public configuration, read once per page load ([ADR 0034](../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md)): `ui.showDescriptions` (default `true`) says whether a thread's description is drawn at all, and `ui.history` (present only where the orchestrator serves the history route) whether, and how, a thread is opened from its newest turns ([Opening from the history](#opening-from-the-history)). A configuration that cannot be read leaves the defaults; descriptions wait for the answer so that one that is then switched off never flashes (`use-ui-config.ts`) |
| `GET /api/tool-servers`, `PUT /api/threads/{id}/tools` | the MCP servers a person may attach to a conversation ([ADR 0024](../docs/decisions/0024-mcp-tools-attached-per-conversation.md)): the deployment's list in its own order (name, what it is for, an icon as a `data:` URI, the agents it is offered for), read live each time the chat mounts and the picker opens, and **the whole set** a thread should have, on every toggle. Both take `thread.write`; [MCP servers attached to a conversation](#mcp-servers-attached-to-a-conversation) |
| `forwardedProps["vymalo.tools"]` on `POST /agui/agents/{agentId}` | the ids a **new chat** attaches, carried by the run that creates the thread and by no other; `GET /agui/agents/{id}/capabilities` is read live for `thread-tools/v1` in `custom`, so an agent that cannot use them is flagged before the person sends |
| `forwardedProps["vymalo.send"]` on `POST /agui/agents/{agentId}` | how a message sent **while a run is open** is delivered ([ADR 0036](../docs/decisions/0036-sending-while-an-agent-works.md)): `steer` (Send) or `interrupt` (Stop and send), on a run that carries the one new message and on no other; without it a run on an open thread is a 409. `GET /agui/agents/{id}/capabilities` is read live for `steer/v1` in `custom`, which words the menu ("reads it at its next step" or "after this turn"); [Sending while the agent works](#sending-while-the-agent-works) |
| `forwardedProps["vymalo.mentions"]` on `POST /agui/agents/{agentId}` | the agents a **message mentions** ([ADR 0026](../docs/decisions/0026-agent-mentions-as-structured-references.md), [`mentions-v1.md`](../docs/api/mentions-v1.md)): `[{agentId, label, start, end, cardUrl?}]`, offsets in **UTF-16 code units** into the message text, on the run that carries the message (a new chat, a follow-up, a message sent while the agent works) and on no other; a 400 for a bad shape, a 422 for a label that is not the text, an unknown or moved agent, one the roles may not invoke or the thread's own, a 503 for a registry that cannot answer, each with the orchestrator's words shown above the box. `metadata["vymalo.mentions"]` of a user message's `START` brings them back (a reload, another tab); `GET /agui/agents/{id}/capabilities` is read live for `mentions/v1` and `thread-tools/v1` in `custom` |
| `vymalo.ask` activity, `SUBAGENT_STARTED sub-ask-<n>` | an agent **the thread's agent asked** ([ADR 0026](../docs/decisions/0026-agent-mentions-as-structured-references.md), `ask_agent` of [`thread-tools-v1.md`](../docs/api/thread-tools-v1.md#ask_agent), [the stream](../docs/api/agui.md#asked-agents-as-subagents)): the activity `ask-<n>` (`replace: true`: running, then its end) is **a step of the tree**, "Asked Adam", nested under the step or the ask that asked (`by`, `parentStepId`); the steps it relayed carry the path `ask-<n>`. The subagent events are not read for it: the activity says everything a line draws. `{ask, agent, by, depth, text, stepId, parentStepId?, state, startedAt, at}` and, once it ended, `answer?`, `question?`, `artifacts?`, `error?`; `state` is `running`, `completed`, `input_required`, `auth_required`, `failed`, `rejected`, `canceled` or `timed_out` |
| `CUSTOM` `vymalo.usage`, `vymalo.usage_total` on the connect stream | **token usage** ([ADR 0056](../docs/decisions/0056-token-usage-per-model-call.md), [`usage-v1.md`](../docs/api/usage-v1.md), [the frames](../docs/api/agui.md#token-usage)): the tokens of each model call of an agent that lists `usage/v1`, and a task's totals; folded by `ThreadAgent` into the ring beside Send and never handed to the runtime; [Token usage](#token-usage) |
| `POST /api/threads/{id}/fork` | **Fork from here** (a turn action) and **continue with another agent** (the agent menu, after a question): `{after: <an event of the turn>, target?}` makes a new chat that holds the conversation up to the end of that turn, and the page goes to it ([ADR 0029](../docs/decisions/0029-forking-a-thread-copies-its-log.md)); **Edit** under a message of the person is the same route with `{replace: <seq>, text, messageId}`: a new chat that holds what came before the message, the new words and the agent's answer, and the page goes to it at `#m-<seq>`. The page chooses the id of the fork, kept for a repeat of the same request. `409 turn_open` is shown under the top bar ("The agent is still working on this turn…"); the buttons are disabled while a turn runs, so it is the race only |
| `GET /api/threads/{id}/branches`, `GET /api/threads?branches=include` | the versions of a message: `‹ 2/3 ›` under a message that was edited (each version is a thread; the arrows go to it). The thread list leaves the edits out, and highlights the conversation's first thread while an edit is open |
| `GET /api/threads/{id}/export` | **Export JSON** in the thread's overflow menu (the `…` of the top bar): the whole thread (messages, agent statuses, artifacts, check, CI and verifier cards, reworks, the job) as `thread-<id>.json`, to send to a developer. The request carries the header `X-Web-Revision`, this build's `NEXT_PUBLIC_BUILD_REVISION` (a Docker build argument, the commit sha in the workflow; none in a local build), and the file says it as `versions.web.revision` beside the orchestrator's and the agents' builds ([ADR 0053](../docs/decisions/0053-a-thread-export-says-which-builds-made-it.md)). The file is the server's: its `thread` carries the description and its log the `thread_described` events, whether or not the web shows descriptions |

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
- **Reload is a replay, or the newest turns of one.** There is no history adapter: the connect stream from the start is the
  history, replayed through the same path as live frames, which keeps every activity (see
  [`patches/UPSTREAM.md`](patches/UPSTREAM.md#observed-not-patched)). The transcript is not drawn
  while it replays, and is drawn whole at its end ([Opening a long thread](#opening-a-long-thread)). Where the orchestrator
  serves `ui.history` and `ui.history.windowed` is on (the default), the thread is not replayed: its newest turns are read as one
  page, shown at the bottom, and the stream follows from where the page ends ([Opening from the history](#opening-from-the-history)).
- **A thread never locks** ([ADR 0020](../docs/decisions/0020-a-thread-is-a-conversation.md)). The
  composer is never disabled. While a run is live the box stays open: the button says **Stop** (`POST /api/threads/{id}/cancel`)
  and the draft survives it, and with text a split **Send** joins it, which sends the message while the agent works
  ([Sending while the agent works](#sending-while-the-agent-works)); once the
  run has ended the button is Send, and a message on a `done`, `failed` or `cancelled` thread is the next job's first
  word, in the same transcript. The placeholder says what fits: "Describe a task for the agent…" (new), "Reply…" (the agent
  asked), "Tell the agent how to go on…" (failed or stopped), "Send a follow-up…" (otherwise). Nothing tells the
  person to start a new thread; the `vymalo.job` marker of a later job draws nothing. A replay that holds several jobs
  applies their runs one after the other (`live-runs.ts`, `untilShown`: the runtime's transcript lags a render, and a
  run applied before the earlier one showed would hang off the wrong message, so each run waits until the transcript
  shows the messages the runs before it left, a count the replay knows, never a time).
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
  transcript (`dropFailedSend`, with `thread.reset`: `thread.import` empties the runtime's repository before the thread stops listing
  the messages, and a render in between throws "Entry not available in the store") and the problem's `detail` is shown.
- **A turn** (one run, one assistant message) is drawn by `thread.aui.tsx` as a classical chat
  ([DESIGN.md](DESIGN.md), "A turn"): the agent's avatar and name once, one **summary line** for its steps, then its
  parts in order through `MessagePrimitive.GroupedParts`. Every stretch of step parts (`lib/steps.ts`: a status but a
  failure, an artifact, `.check`, `.ci`, `.rework`, `.action`, `.step`, the actor and job markers) draws **nothing**
  in the chat: the steps are the side panel's Activity tab ([The step tree](#the-step-tree)), and the line
  (`steps/turn-summary.tsx`) opens the panel on this turn; the agent's **answer** (`TEXT_MESSAGE_*`, including the
  words of a `completed` or `input_required` status, `st-<seq>`) is prose, and the words it said while it worked are
  not drawn here but are notes in the panel ([The answer and the working text](#the-answer-and-the-working-text)); a failed status and `.error` are soft callouts (`parts/error-callout.tsx`: the **first line** of the reason is the message and the rest, a type checker's code frames or a long finding, is behind **Show details**, a native `<details>` whose block is preformatted, `white-space: pre-wrap`, monospace, at most 16rem high and scrolling both ways inside itself, focusable so the keyboard can scroll it: `lib/failure-text.ts` splits the text, and a first line longer than 240 characters is cut with the whole text behind the control; the mock's `fail-long` scenario is the proof)
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
| (same file, `package.json`) | the thread core of the runtime has no export of its own; the subpath `./runtime/core` lets the history seed make the messages of a page of turns without a render ([ADR 0059](../docs/decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md)) | [Export the thread core](patches/UPSTREAM.md#export-the-thread-core) |

[ADR 0012](../docs/decisions/0012-ag-ui-user-facing-protocol.md#the-web) expected patches for two
more gaps (activities dropped on reload; no live subscription). Neither is needed here, because the app never restores through `fromAgUiMessages` and applies runs it did not
start through the runtime's public API; both are drafted as upstream issues anyway
([Observed, not patched](patches/UPSTREAM.md#observed-not-patched)).

## Opening a long thread

A thread that is opened is its log, replayed: the connect stream gives the runs one after the other and `LiveRuns` hands each to the runtime
([The chat layer](#the-chat-layer)). Every run is a render of every turn so far, so the cost of opening a thread grows with the square of its
turns, and the page draws the turns as they come: the transcript grows in front of the person and the viewport chases its own bottom, smoothly,
run after run. This section is what that costs, measured (ADR 0059, slice 0), and what was done about it.

### What it costs

`e2e/open-long-thread.spec.ts` opens a thread that the mock made (`POST /__mock/long-thread`, [Mock server](#mock-server)) in a fresh browser context and
records, from outside the app (nothing in it is instrumented, `e2e/open-probe.ts`): a sampler on every animation frame (the viewport's `scrollTop`,
`scrollHeight`, the turns in the page, whether a turn is painted), the protocol's network events for the connect stream, and the main thread's script,
layout and style time (`Performance.getMetrics`). All times are milliseconds since the navigation started, and the median of the runs.

```sh
OPEN_TURNS=200 OPEN_RUNS=2 pnpm exec playwright test open-long-thread --project=chromium --workers=1
```

`OPEN_HISTORY` is what the mock's `ui.history` says: `windowed` (the default) opens the thread from its history, `on` replays the log; `OPEN_SHOWN` is
the turns the page waits for (12 from the history, the thread's own otherwise). The tables below are the replay until [Opening from the
history](#opening-from-the-history), which has both.

*Measured 2026-10-09* on one shared 4-core machine, headless Chromium of Playwright 1.56.1, no CPU throttling, the mock on the loopback: the
absolute times are this machine's, the ratios are the finding. A turn is nine log events and a 15-line answer with a code block.

| The replay, before anything was changed | 40 turns (5 runs) | 200 turns (2 runs) |
|---|---|---|
| The connect stream received (first byte, last byte), KiB | 0.56 s, 0.56 s, 231 | 0.67 s, 4.3 s, 1,164 |
| First turn in the page | 6.0 s | 4.3 s |
| First turn **painted** (frames are few while the main thread is busy) | 9.2 s | 75.9 s |
| Every turn in the page | 9.0 s | 138.7 s |
| The viewport at its final position | 9.7 s | 77.0 s |
| From the first turn in the page: scroll events, frames in which the viewport moved, px travelled | 58, 58, 29,224 | 158, 156, 147,916 |
| The biggest step of the viewport between two frames | 372 px (after the first paint) | 14,582 px (after the first paint) |
| Main thread: long tasks, time over 50 ms in them | 18, 5.7 s | 290, 110.6 s |
| Main thread: script, layout, style, all tasks | 7.1 s, 0.38 s, 0.32 s, 9.6 s | 118.2 s, 2.8 s, 1.7 s, 139.4 s |

Where the time goes:

- **The wire and the parse are not it.** The stream's 4,439 frames (200 turns) are fetched and parsed by the SSE reader in 98 ms in Node; in a
  profile of a 40-turn open, `ThreadAgent` (grouping, routing, emitting) is about 35 ms of 10.8 s.
- **Layout and style are 3 %.** The rest of the main thread is script, and nearly all of that is React rendering and committing: the runtime's
  store reconciles a client per message (`AuiProvider`), the transcript renders every turn again (`WithTime`'s tooltip, Markdown, the turn's summary
  line), the composer's autosizing textarea measures its box on every render. Run *R* adds one render of *R* turns.
- **It is quadratic.** The last turn is in the page after 4.1 s for 20 turns, 12.4 s for 50 and 36.4 s for 100 (a plain wait for the text, no probe),
  and 139 s for 200 (the table). The owner's "3 to 4 seconds for long chats" is a thread of about twenty such turns.

### The transcript is not drawn while the log replays

`useChatRuntime` says when the replay is applied (`revealed`: `isSettled` in `lib/reveal.ts`, kept by `useRevealed`, so once true it stays true): the log
is caught up (`loaded`) and the runtime shows every run the stream delivered (`!snapshot.replaying`). Until then `Thread` (`thread.aui.tsx`) draws the
skeleton and **no turn**, and the log is `aria-busy`; then it draws all of them at once and, in the layout phase of that render, before the first paint,
puts the viewport at its end with `behavior: "instant"`. `scroll-smooth` on the viewport and the library's scroll at the start of a run
(`scrollToBottomOnRunStart`) are for the live conversation, so they begin when the transcript is drawn: a message the person sends later, or a run another
tab starts, scrolls as it always did, and is never held back (`useRevealed` is sticky). A connection that is down with part of the log in shows that
part, as the page did before it held anything back (`isSettled`). The link of a version (`#m-<seq>`, `useScrollToMessage`) waits for the transcript to be drawn.

```mermaid
sequenceDiagram
  autonumber
  actor P as Person
  participant C as ChatShell
  participant A as ThreadAgent
  participant L as LiveRuns
  participant T as Thread
  P->>C: opens /threads/T
  C->>T: loading: the skeleton, no turn drawn, the log is aria-busy
  A-->>L: the runs of the log, one after the other (replaying)
  L->>L: applies each run to the runtime, which draws nothing
  A-->>C: the log is caught up (loaded) and no run is left to show
  C->>T: loading is false (revealed, sticky)
  T->>T: draws every turn, scrolls to the end, instantly, before the first paint
  Note over T: scroll-smooth and the scroll at a run's start are on from here
  P->>C: sends a message
  C->>T: a live run: the transcript is not held back again
```

```mermaid
stateDiagram-v2
  [*] --> Held: the thread is opened
  Held --> Shown: the log is caught up and the runtime shows every run (isSettled)
  Held --> Shown: the connection is down with part of the log in
  Shown --> Shown: a live run, a run another tab starts
  Shown --> [*]: the page is left
```

Not drawing the turns while they replay is not only for the eye. Hiding the transcript but leaving it in the page (out of the flow, `invisible`) removed
the scrolling and none of the cost, a 40-turn thread opened in 11.2 s; not drawing it at all opens the same thread in 6.3 s, because no run renders the
turns before it. *Measured 2026-10-09*, as above, with `e2e/thread-opens-at-end.spec.ts` as the test (the first paint is the whole conversation, at its
bottom, and nothing moves after it; the skeleton stands in while it replays; a message sent later still scrolls to the end):

| | Before | The transcript not drawn while it replays |
|---|---|---|
| 40 turns: first turn painted, final position | 9.2 s, 9.7 s (turns in the page at the first paint: 40, 785 px from the end) | 6.3 s, 6.3 s (all 40, 0 px from the end) |
| 40 turns: from the first turn in the page, scroll events, frames in which the viewport moved, px | 58, 58, 29,224 | 0, 0, 0 |
| 40 turns: long tasks, time over 50 ms, script | 18, 5.7 s, 7.1 s | 5, 4.2 s, 4.4 s |
| 200 turns: first turn painted, final position | 75.9 s (51 turns in the page), 77.0 s | 39.1 s, 39.1 s (all 200, 0 px from the end) |
| 200 turns: scroll events, frames in which the viewport moved, px | 158, 156, 147,916 | 0, 0, 0 |
| 200 turns: long tasks, time over 50 ms, script | 290, 110.6 s, 118.2 s | 34, 23.8 s, 28.2 s |

What is left of the cost (28 s of script for 200 turns) is not the transcript: it is the runtime's store, which reconciles a client per message on every
run, the side panel and the composer, and it is still quadratic in the runs. Applying the runs where nothing is subscribed is the next thing to try.

### Opening from the history

The cost above is per run applied, so a thread that is opened should apply few runs. Where `GET /api/config` says `ui.history` (the
orchestrator serves `GET /agui/threads/{id}/history`, [`history.md`](../docs/api/history.md)) and `windowed` is on, which it is by default
([ADR 0059](../docs/decisions/0059-a-thread-opens-at-its-end-and-older-turns-load-on-scroll-up.md), option C), `ThreadAgent` does not replay the log:

- **The newest page.** It reads `limit=<initialTurns>` (12): the frames of the newest settled turns, from the same projection the connect stream
  writes, with the `end` of the page, a settled `Last-Event-ID`, and the `carry` of what the turns before it contribute to the readouts that cover the
  whole thread. Its runs are the **seed** (`ThreadAgent.openAtEnd`, `takeSeed`); the connect stream is held until the seed is in, then follows
  from the page's `end`. A page that cannot be had (a 5xx, a network error), a seed that cannot be made, and an orchestrator without
  `ui.history` open the thread the old way, from event 1; so does a 404 of the history route (an orchestrator that does not serve it answers 404
  too), and the replay's stream says whether the thread is there ("Thread not found").
- **The seed** (`lib/agui/seed.ts`, `components/history-seed.tsx`). The messages of the page are made by the runtime's own thread core,
  `AgUiThreadRuntimeCore` (exported by the pnpm patch, [`patches/UPSTREAM.md`](patches/UPSTREAM.md#export-the-thread-core)), driven the way
  `LiveRuns` drives the runtime and without a view: 4 to 10 ms a run against a quarter of a second, since nothing waits for a render. They go into the visible
  runtime in one `thread.import`. `seed.dom.test.tsx` holds the result equal to a replay's on every golden, and `windowed.dom.test.tsx` that two
  pages put together (the log cut at every place a chain starts, every golden) are the replay of the whole.
- **Older turns** (`hooks/use-earlier.ts`, the row above the first turn in `thread.aui.tsx`). Within a screen of the top an
  `IntersectionObserver` asks for the next page, one at a time: `before=<start of the oldest page held>`, with 20 turns, then 40, 80, up to `server.history.maxTurns`
  (the sum of reading back is then *L log L* on the server, [`history.md`](../docs/api/history.md) rule 3). The messages are made the same way and put in
  front of the transcript in one import, `older ++ current`; a message id both hold is kept once, the newest copy (`joinMessages`), and the question an
  older page ended on is settled, as a replay does when the next run answers it. The turn being read stays where it was: its offset from the top is noted
  before the import and put back in the layout phase after it, instantly (`useKeepAnchor`; the browser's own scroll anchoring is not relied on: `overflow-anchor` reached Safari in
  27, *verified 2026-10-09* in MDN's browser-compat-data). A page that fails says so with Retry; a page that does not join the ones held (a gap, an overlap, another projection's frames: reload) is refused. **The account of the pages
  (the window, the usage, the turns before it, the files) takes a page in only after `thread.import` has succeeded** (`ThreadAgent.readEarlier` reads and checks a page and hands back
  `commit`): a page that waits for a moment to import changes nothing on the screen, and one whose import fails is read again by Retry.
- **When the transcript may be replaced.** An import in the middle loses the message of the open run and a staged A2UI click (*measured 2026-10-09*), so
  `useEarlier` waits ("Earlier messages will load when the agent is done") until `ThreadAgent.idleForImport()`: no run open or on its way to the runtime,
  nothing being sent, no action staged, the runtime not running, and looks again once `pauseRuns()` holds the runs the stream delivers (released in order after the import). A
  question that waits for the person does not hold it back: the transcript that is imported contains its message, which is where the runtime reads a pending
  interrupt from (`use-earlier.dom.test.tsx`, which also answers it afterwards).
- **What the turns that are not held contribute.** The token ring is the thread's, not the page's: the `carry` is the initial state of the usage fold, the loaded
  pages' usage frames are added, and an older page replaces the carry (`usageFromCarry`); the turns are numbered by the whole thread ("Turn 49" stays 49 when
  older pages load: `carry.turns` offsets the position, `TurnView.turnsBefore`); an `Image` of a surface may name any file the thread kept
  (`carry.files`, the newest 500, unioned with the files of the turns held). Sources are those of the turns held and say so ("Sources from the last 12
  turns"; the tab's count has a `+`), with a button that loads older turns, at most five pages (about 300 turns) a click, and shows the note again while there are more; they are never loaded unasked (question 72).
- **A reader with no session replays.** `ui.history` comes from `GET /api/config`, which is behind the identity layer, so the page of a public link opened signed out
  reads no configuration (`useChatRuntime` asks `useUiConfig(false)`: the defaults at once; the signed-in client would hold the refused request for a sign-in, up to ten minutes) and
  does not wait for one: the log is replayed through the public connect route as before (`sharing.dom.test.tsx`, with the edge's sign-in built in). A signed-in reader of a link gets the
  link's history route (`SharedChat`).
- **A link to a message** (`/threads/<id>#m-<seq>`). The first page asks `since=<seq>`: the turns back to it, no further than `maxTurns`. The page is
  scrolled to the message, instantly, two frames after it is drawn (`useScrollToMessage`), and the viewport is kept from following the end of
  the transcript for two seconds, because the library pulls a page that has been moved away from the end back whenever what it holds changes size and it
  has not seen the person scroll up (*verified 2026-10-09* in `@assistant-ui/react` 0.15.22, `useThreadViewportAutoScroll.js`; `Thread`'s `pinned`). A link further back than the server allows opens at the end and says so.

Tests: `e2e/thread-history.spec.ts` (the newest 12 of 60 turns at the bottom, the place kept as older pages come, growing pages to the first turn, a failed
page and Retry, a send after the open, older turns that wait for an open run, Sources, a link to a message, and the three ways back to the replay),
`e2e/thread-opens-at-end.spec.ts` (first paint at the bottom with no scroll after it, the skeleton, a run that starts later, in both ways of opening),
`e2e/share.spec.ts` (the reader's route; no stream is held open), and the vitest files named above.

```mermaid
sequenceDiagram
  autonumber
  actor P as Person
  participant C as ChatShell (HistorySeed, useEarlier)
  participant A as ThreadAgent
  participant R as Runtime (visible)
  participant O as Orchestrator
  P->>C: opens /threads/T
  C->>A: start()
  A->>O: GET /agui/threads/T/history?limit=12 (since=seq for a link)
  O-->>A: HistoryPage: frames, start, end, earlier, carry
  A->>A: collectRuns(page): the seed, the usage and the turns before it from the carry
  A-->>C: takeSeed()
  C->>C: buildMessages(runs): the runtime's thread core, no view
  C->>R: thread.import(messages), the transcript is drawn at the bottom
  C->>A: seeded()
  A->>O: GET /agui/threads/T/connect, Last-Event-ID: end of the page
  P->>C: scrolls to within a screen of the top
  C->>A: readEarlier(): GET history?before=start&limit=20
  A-->>C: the runs of the older page, not held yet
  C->>A: idleForImport() then pauseRuns(), idleForImport() again
  C->>R: thread.import(older ++ current), the turn being read stays put
  C->>A: commit(): the page, its usage and its carry are held
  C->>A: release the runs
```

```mermaid
stateDiagram-v2
  [*] --> Reading: the thread is opened
  Reading --> Seeding: the newest page is in
  Reading --> Replaying: no ui.history, a page that cannot be read (a 404 too), or a seed that cannot be made
  Seeding --> Following: imported, drawn at the bottom
  Following --> Loading: the top is near and there are older turns
  Loading --> Waiting: a run is open, or a send or an action is pending
  Waiting --> Loading: nothing is open
  Loading --> Following: imported, the place kept
  Loading --> Following: failed, with Retry
  Following --> [*]: the page is left
  Replaying --> NotFound: the stream answers 404
  Replaying --> [*]: as before
```

*Measured 2026-10-09* with the same spec (`OPEN_HISTORY=on` replays the log, `windowed` opens the thread from its history), on the same shared 4-core
machine, headless Chromium, no throttling, the mock on the loopback, medians of 3 to 5 opens, after the commit that made `windowed` the default. Times are
milliseconds since the navigation started; "page" is the history page (`GET /agui/threads/{id}/history?limit=12`), "heap" the JavaScript heap the tab retains
at the end (after a garbage collection).

| | 40 turns | 200 turns | 1 000 turns |
|---|---|---|---|
| The replay, nothing changed (above) | 9.2 s first turn painted | 75.9 s | not measured |
| The replay, transcript held while it replays (the previous section; `OPEN_HISTORY=on`) | 6.5 s, 4.9 s of script, heap 32 MiB | 44.8 s, 34.0 s of script, heap 110 MiB | the tab died |
| Opened from the history (`windowed`): page arrived, first turn painted (12 turns, at the bottom) | 0.65 s, 1.14 s | 0.65 s, 1.13 s | 0.75 s, 1.17 s |
| Opened from the history: bytes of the page, script, heap | 71 KiB, 0.71 s, 18 MiB | 72 KiB, 0.76 s, 18 MiB | 72 KiB, 0.72 s, 18 MiB |
| Scroll events and frames in which the viewport moved, after the first paint | 0, 0 | 0, 0 | 0, 0 |

- **The open no longer grows with the thread.** A thread of 1 000 turns opens in the time of one of 40: the page is 12 turns whatever the thread, and the
  connect stream that follows from its `end` has nothing to replay. The 0.65 s before the page arrives is the app loading, `GET /api/config` and the
  request itself (not split further); the 0.4 to 0.5 s after it is the seed of 12 turns, the import and the first draw.
- **A replay of 500 turns, or of 1 000, did not open at all** in this browser: the tab died (a trap in Chromium's compositor thread, `dmesg`, about two minutes
  in; the probe reports "the tab crashed"). The cause is not established (*unverified*; the heap at 200 turns is 110 MiB, nowhere near a limit). The windowed
  page does not show that thread's older turns until the person asks for them, so it does not meet the same size.
- **The orchestrator's part** is small: the newest 12 turns of a 12 000-event thread fold in 61 ms in a release build
  ([`history.md`](../docs/api/history.md) rule 3); the mock folds in TypeScript, so the page above says nothing more about it.


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
| State pill | `state-badge.tsx` | One pill, **an icon of a shape of its own** with its words as its name (`aria-label` "Thread state: …", and visually hidden text) and as its tooltip: queued "Starting…" a clock, working "Working…" a spinner, verifying "Checking the work…" a shield with dots (not the shield with a check, which is a check that passed; its own colour, `--verifying`, a violet that keeps 4.5:1 in both schemes; spoken "Thread state: Checking the agent's work"), done "Done" a check, failed "Failed" a cross, cancelled "Stopped" the stop of the Stop button, not a ban. Every state has a shape of its own, still (the spinner is the only one that moves). **Blocked keeps its words**, "Your turn" when the agent asked (an interrupt is open) and "Needs attention" otherwise: it is the one state that asks the person to act ([Icons, and the words that stay](#icons-and-the-words-that-stay)). There is no attempt counter: attempts show inside the turn, in the rework step |
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

A step's icon is one of the names of `steps/v1` (`STEP_ICONS` in `lib/agui/vymalo.ts`, drawn by `steps/step-icons.ts`). `opencode`, the step that hands work to OpenCode over ACP ([ADR 0049](../docs/decisions/0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md)), is a terminal in a frame, **not OpenCode's logo** (its terms are unverified); the glyph's tooltip and a screen reader say OpenCode unless the step's label does. It appears once adam-rs sends it.

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

## Thinking

What the agent's model thought before it answered is shown as a **closed "Thinking" block above the words of the turn**, and grows while
the person has it open and the model is still writing it ([ADR 0044](../docs/decisions/0044-a-models-reasoning-is-shown-beside-the-answer-and-logged-once.md),
[the frames](../docs/api/agui.md#reasoning)). It is the second kind of [draft](#live-text): the runtime's transcript is the log, so the live reasoning
is kept out of it and drawn by the turn, and the log's reasoning becomes a reasoning part of the runtime's message.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-thinking-open.png">
  <img src="e2e/__screens__/desktop-light-thinking-open.png" alt="A turn in which the model is still thinking. Under the line Started working, a Thinking line with a chevron turned down shows the model's text so far, The user wants Fibonacci in Rust. I should write the iterative version, in a muted type with a thin rule on its left. No reply has started. The top bar says Working…." >
</picture>

*The mock's `think-gate`: the model is still thinking, and the person opened the block.*

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-thinking.png">
  <img src="e2e/__screens__/mobile-light-thinking.png" alt="A finished turn on a phone. Above the reply Fibonacci in Rust. a line Thinking with a chevron pointing right is folded away. The top bar says Done." >
</picture>

*The same turn when it is done: the block is closed above the one reply.*

```mermaid
sequenceDiagram
  autonumber
  participant O as Orchestrator (connect stream)
  participant T as ThreadAgent
  participant D as Drafts (lib/agui/live-drafts.ts)
  participant R as Runtime (messages)
  participant V as Turn (thread.aui.tsx)
  O-->>T: REASONING_START {vymalo.live} (no id:)
  T->>D: a draft {kind: reasoning, id}, empty (REASONING_MESSAGE_START opens nothing more)
  O-->>T: REASONING_MESSAGE_CONTENT {vymalo.live: {offset}} (no id:)
  T->>D: text = text.slice(0, offset) + delta
  D-->>V: drawnReasoning: Thinking, closed, shimmer while writing
  Note over V: the person opens it, the text is drawn and grows, an opened id is remembered
  O-->>T: CONTENT {offset, final} + MESSAGE_END {final} + END {final}, id: n (the log's reasoning, one group)
  T->>R: REASONING_START, MESSAGE_START, CONTENT (the whole text), MESSAGE_END, END: one reasoning part
  R-->>V: the part, drawn by Thinking with the same id (open stays open), the draft draws nothing
```

```mermaid
stateDiagram-v2
  [*] --> Writing: REASONING_START (a draft, empty)
  Writing --> Writing: REASONING_MESSAGE_CONTENT {offset} grows it
  Writing --> Gone: the two ends {abandoned}, a cut connection
  Writing --> Completed: the log's group (CONTENT {offset, final}, the two ends {final})
  Completed --> Gone: the transcript has the part
  [*] --> Logged: no live frame heard: the log's five events alone (a reload, an export)
  Logged --> [*]
  Gone --> [*]
```

- **`lib/agui/live-drafts.ts`** treats a live `REASONING_START` as it does a live `TEXT_MESSAGE_START`: a draft with `kind: "reasoning"`; the
  `REASONING_MESSAGE_START` that follows says nothing more; `REASONING_MESSAGE_CONTENT{offset}` grows it by the same rule as a reply's; the two
  ends with `abandoned` remove it. The log's reasoning arrives as `CONTENT{offset, final}` and the two ends `{final}` (the overlay dropped its two
  `START`s), and `resolveGroup` rebuilds the five events from the draft, so the runtime reads the reasoning as the log wrote it, whatever the
  connection heard, and a final that continues a draft this connection never held reopens the connection as a reply's does. A reply's draft and a
  reasoning's are told apart by `kind` and live side by side (the log's reasoning can still be on its way when the words begin).
- **The block** (`components/thinking.tsx`, `data-slot="thinking"`, `data-state` `open` or `closed`, `data-streaming`) is a button with
  `aria-expanded` and the text below it as plain text in its own scroll (`max-h-72`): the reasoning is untrusted, so no markup is read from it. It is
  **closed by default**, the label is "Thinking" with the shimmer of the starting line while the model is writing, and the people who open it keep it open:
  the state is kept **by the reasoning's id** (`useSyncExternalStore`), so the log's text taking the draft's place does not close it. The id is
  `providerMetadata.agui.reasoningId` of the runtime's part (`reasoningIdOf`), and the draft's id: the same string.
- **Where it is drawn.** In `AssistantMessage` (`thread.aui.tsx`) a `reasoning` part of the turn is a `Thinking` in its place before the turn's
  text, and the live reasoning is a `Thinking` after the turn's parts and before the words being written (`drawnReasoning`: a merged draft says the
  log's words until the transcript has the part, then nothing, as `drawnDrafts` does). A turn with only reasoning is not an empty turn
  (`steps.ts`). The reasoning is not an answer, a step or working text: the panel does not list it, and Export JSON has it as the log's
  `agent_reasoning` event (the file is the log).
- **The mock** has `think` (the `reasoning` golden: pieces, then the log's `agent_reasoning`, then the reply), `think-gate` (held while the model is
  still thinking until `POST /__mock/release`) and `think-cut` (a log whose reasoning was cut at its bound, which says so in its text);
  `mock/projection.ts` and `mock/live.ts` are held to the `reasoning` goldens by `mock/golden.test.ts` and `mock/live.test.ts`.
- **Tests:** `lib/agui/live-drafts.test.ts` (the rules above), `components/chat-shell-thinking.dom.test.tsx` (the mock through `ThreadAgent` and the
  runtime into the turn: closed by default, grows while open, stays open when the log's text comes, above the reply, a cut reasoning says so, a
  reload), `e2e/thinking.spec.ts` (desktop and phone, axe in both schemes), and the screens `thinking-open` and `thinking` of `pnpm screens`.

## Token usage

How full the model's context is, and what a thread spent, is a **ring beside Send** in the composer's bottom row
([ADR 0056](../docs/decisions/0056-token-usage-per-model-call.md), [the frames](../docs/api/agui.md#token-usage)). It is there only for an
agent whose card lists `usage/v1` and once it has reported a call: a thread with no usage has no ring.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-usage-ring.png">
  <img src="e2e/__screens__/desktop-light-usage-ring.png" alt="A finished thread, Summarize the release notes for the next version. Beside the round Send button of the message box, a small ring filled about four fifths in amber; above it the Token usage popover says the agent's last call filled 82 % of the context, 107,500 of 131,072 tokens, then a table of the thread by model, glm-5.3 and glm-5.3-mini with their input, cached, output and reasoning tokens, and a table by who spent it: Adam the agent with 2 calls, Researcher a sub-agent with 1 call, Reviewer an asked agent with 1 call." width="640">
</picture>

*The mock's `Summarize …`: the agent's last call at 82 % (amber), a sub-agent and an asked agent, from the web's mock server.*

- **The fold** is `src/features/chat/lib/usage.ts`, pure: `ThreadAgent` folds each `CUSTOM` `vymalo.usage` and `vymalo.usage_total` into
  `ThreadSnapshot.usage` and hands neither to the runtime (a run that holds only usage is not offered to it). A call is kept once per task and
  call id, totals replace a task's earlier ones, so a replay, a reconnect and the live stream give the same ring.
- **The ring** (`components/usage-ring.tsx`, a button named "Token usage: context 82 % full, 107,500 of 131,072 tokens") fills with
  `inputTokens / contextWindow` of the **latest call of the thread's agent** (`by.kind` `agent`; a sub-agent's or an asked agent's call
  never moves it): muted below 80 %, `--warning` from 80 %, `--destructive` from 95 % (`data-level`: `normal`, `warn`, `danger`). A call
  with no `contextWindow` leaves the track with no fill (`data-level="none"`) and the name says the window is not known.
- **The details** are a popover (`components/ui/popover.tsx`, Radix, a `dialog` named "Token usage"; Enter or a click opens it, Escape
  closes it and the focus goes back to the ring): one line on the last call, then two tables, **This thread, by model** (each task's
  latest totals plus the calls after them, else its calls: input, cached, output, reasoning) and **By who spent it** (the agent, each
  sub-agent by its step's label and each asked agent by name, from their call reports, with the number of calls). A task's totals can
  be lower than its calls' sum (adam leaves out what it could not keep), so the second table can add up to more than the first.
- **The mock** has `usage` (the `usage` golden's turn), `Summarize` (the screenshot: amber, a sub-agent and an asked agent), `usage-full`
  (red), `usage-nowindow` (no window) and `usage-hold` (one call, then held until `POST /__mock/release`); `mock/projection.ts` projects
  them as the orchestrator does (`mock/usage.ts` is its run accounting, `RUN_FINISHED.usage`), held to the `usage` golden by
  `mock/golden.test.ts`.
- **Tests:** `lib/usage.test.ts` (the golden stream folded, a replay equal to the live fold, totals and the calls after them, the latest call
  of the agent, the levels, what does not parse), `lib/agui/thread-agent.test.ts` (the usage of the stream folded across a reconnect and
  never handed to the runtime), `e2e/usage.spec.ts` (desktop and phone: the fill, the details by the keyboard, a reload, red, no window,
  the ring moving while the agent works, no ring without usage, axe), and the screen `usage-ring` of `pnpm screens`.

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
  The model writes it after the thread is `done`, so **a thread the page watched finish keeps its stream for 45 s**
  (`FINISHED_GRACE_MS` in `use-chat-runtime.ts`, counted by `use-elapsed.ts`) before the page lets go of it; it used to let
  go the moment the thread was finished and caught up, which would have left a description that arrives a second later for
  the next reload. A thread that was already finished when it was opened still lets go at once (the system e2e
  `history.spec.ts` holds that), so its late description shows on the next visit.
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

## MCP servers attached to a conversation

*Added 2026-10-02 ([ADR 0024](../docs/decisions/0024-mcp-tools-attached-per-conversation.md), MVP slice 8; PR-8 of plan 11).* A person
attaches MCP servers (a web search, a documentation index) to a conversation from the composer, and the agent uses them from the
next message on. The deployment lists what may be attached (`toolServers` of its configuration, [`config.md`](../docs/api/config.md#toolservers));
a person cannot enter a URL. The web holds ids only: no URL, no header and no credential of a server is ever in the page. The
code is `src/features/tools/`, the card check is `features/agents/hooks/use-agent-capabilities.ts`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-tools-picker.png">
  <img src="e2e/__screens__/desktop-light-tools-picker.png" alt="A new chat with the Tools menu open above the message box: Web search, GitHub and Team docs, each with an icon and one line of what it is for, Web search and Team docs checked. Under the box, the plug icon button named Tools and two chips, Team docs and Web search, each with a button that takes it off." width="720">
</picture>

*The picker on a new chat, from the web's mock server: the servers offered for the coder, two chosen.*

| The calls on the steps | On a phone |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-tools-steps.png"><img src="e2e/__screens__/desktop-light-tools-steps.png" alt="A finished thread that started with two servers attached. The conversation holds the line “Team docs and Web search attached” above the coder’s turn. The Activity panel lists a web search with the search server’s own icon, a GitHub call and a Team docs call that failed." width="400"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-tools-picker.png"><img src="e2e/__screens__/mobile-light-tools-picker.png" alt="A phone: the Tools menu open above the message box with three servers, two checked, and the two chips under the box." width="200"></picture> |

*The same, from the web's mock server (`relay`): the line in the conversation and the server's icon on each call in Activity; and the picker on a phone.*

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant W as Web
  participant O as Orchestrator
  W->>O: GET /api/tool-servers (when the chat mounts, and again when the picker opens)
  O-->>W: the servers in the deployment's order, each with id, name, an icon as a data URI and its agents
  W->>O: GET /agui/agents/adam/capabilities (live, never cached)
  O-->>W: custom has thread-tools/v1, or it has not
  Note over W: no thread.write for the person: nothing is asked, no picker
  U->>W: new chat: chooses Web search (kept in the page)
  U->>W: sends the first message
  W->>O: POST /agui/agents/adam, forwardedProps["vymalo.tools"] = ["websearch"]
  O-->>W: RUN_STARTED, then STATE_SNAPSHOT thread.tools and the vymalo.tools card
  U->>W: later, on the thread: chooses Team docs
  W->>O: PUT /api/threads/{id}/tools with the whole set, docs and websearch
  O-->>W: 200 with the set (or 422 with the server's id in words, 403 forbidden, 404)
  O-->>W: the connect stream: tools_attached as a snapshot and a card
```

```mermaid
stateDiagram-v2
  [*] --> Chosen: a new chat's picker (kept in the page)
  Chosen --> Chosen: toggle, or the agent changes (what it may not have is dropped)
  Chosen --> Attached: the run that creates the thread carries the ids
  [*] --> Attached: an open thread has them
  Attached --> Attached: PUT of the whole set (a toggle)
  Attached --> Detached: PUT without it
  Detached --> Attached: PUT with it
  Detached --> [*]
```

- **The picker** (`tools-picker.tsx`) is a plug icon button named "Tools" (its words are its tooltip), in the composer's toolbar slot (`Composer.toolbar`), and the
  attached servers are chips beside it, each with a button, "Remove Web search". The menu is a group of **checkbox items** (icon,
  name, what it is for, a check), one per server **offered for this agent** (`ToolServer.agents` absent: every agent;
  `lib/servers.ts` `offeredFor`), in the deployment's order. A choice does not close the menu; Escape does, and the focus goes
  back to the button. Nothing is drawn when the deployment offers nothing for the agent and nothing is attached, and nothing for a
  person whose roles hold no `thread.write` (`GET /api/tool-servers` takes it, so the page does not even ask: it waits for
  `GET /api/me`, and an orchestrator that cannot say is asked and decides; a 403 or a 404 of the list is "no picker", not an error).
  A server a thread was given that the deployment no longer lists keeps its chip, by its id, so it can be taken off.
- **A new chat** keeps the choice in the page and sends it as `forwardedProps["vymalo.tools"]` on the run that creates the thread
  (`Target.tools` of `ThreadAgent`), and on no other run (the orchestrator applies it only then). Choosing another agent drops what
  that agent may not have (the orchestrator would answer 422 for it), and a list read again that no longer has a server does too.
- **An open thread** changes with `PUT /api/threads/{id}/tools` and the **whole set**, on every toggle, in any state of the thread,
  finished ones included (a thread is a conversation, [ADR 0020](../docs/decisions/0020-a-thread-is-a-conversation.md): the set
  applies to the next message) and while the agent works. The truth is the log: `STATE_SNAPSHOT.thread.tools` from the stream, else
  `Thread.tools` of the resource, whichever has read further (`lib/servers.ts` `currentTools`), and the answer of the `PUT` stands
  until something newer than it says otherwise, so a toggle shows at once and does not flicker back. A refused change is the
  problem's `detail` above the box (`the server files is not offered for the agent adam`), and the chip is not drawn; more than
  16 is said before asking.
- **The flag.** An agent whose card does not list `thread-tools/v1` is sent no tools. The web reads the capabilities document live
  (`custom[<uri>]` is the signal, [ADR 0008](../docs/decisions/0008-platform-integration-via-a2a-extension.md)), on a new chat for
  the chosen agent and on a thread for its own, and says **before anything is sent**, as a warning line above the box: "Reviewer
  cannot use attached tools, so they will not be sent to it." The choice is kept (the thread keeps its servers; a person can take
  them off). A card that cannot be read is "Could not check whether Adam can use attached tools.", never "can" (fail closed); the
  menu says "This agent does not use attached tools." Opening the menu reads the card again.
- **The line.** The `vymalo.tools` card is one muted line with a plug, "Web search attached" or "GitHub detached", drawn above the
  turn it came in (or alone, when the thread was finished: a run of its own), with the person's other acts, never among the agent's
  steps. The names are the deployment's list's; a server it no longer lists is its id. It is not a live region.
- **The icon** is the deployment's, a `data:image/(svg+xml|png|webp);base64,` URI of at most 8 KiB, drawn as an `<img>` (an SVG is
  never inlined) in the menu, in the chips and in the slot of a step that calls the server. **An icon that is anything else is no
  icon**: an http(s) URL, a protocol-relative or relative one, another kind of `data:` URI, base64 with a stray character, one over
  8 KiB. The generic plug (or the tool's wrench, on a step) is drawn, and **nothing is fetched** (open question 38: a request for
  an icon would tell whoever runs the host that this person looked at this conversation). `lib/icon.ts` is the one place that
  decides, with a test for each way it says no; Playwright intercepts every request of a page whose list holds an http(s) icon and
  finds none to it.
- **A step that is a call of an attached server** says `icon: "mcp-server:<id>"` ([`thread-tools-v1.md`](../docs/api/thread-tools-v1.md#the-step-of-a-call),
  an agent cannot claim it). `parseStep` reads the id into `StepContent.server` (the vocabulary's `icon` stays as it was: it is
  none for this one) and `step-node.tsx` draws the server's icon in the step row's slot once the call has ended; while it runs the
  spinner is there, as for every running step. The orchestrator does not relay yet (the second half of slice 8), so no real log has such a step; the mock
  plays the story (`relay`), and the slot is drawn by the server's id from the list, as the contract says it will arrive.
- **Read only.** A thread the person may read and not change has no picker and no chips (the composer is the read-only line, and
  `features/me` decides). The line and the icons still name what was attached, from the list the page may read.

## Sending while the agent works

*Added 2026-10-02 ([ADR 0036](../docs/decisions/0036-sending-while-an-agent-works.md), PR-15 of plan 11; the screens are in
[DESIGN.md](DESIGN.md#sending-while-the-agent-works)).* While a thread is `queued`, `working` or `verifying` the box stays open: **Stop**
is always there, and with text a split **Send** joins it: **Send** (Enter) is `steer`, **Stop and send** (Ctrl/⌘+Shift+Enter, or the menu
beside Send) is `interrupt`. The orchestrator logs the message at once, with the `delivery` the core decides, and the projection ends
the run that was open at the message and opens a run of its own for it ([`agui.md`](../docs/api/agui.md#sending-while-an-agent-works)).

| Stop and send | On a phone |
|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-steer-stopped.png"><img src="e2e/__screens__/desktop-light-steer-stopped.png" alt="A finished thread. The person's first message, the coder's first turn with one step, then the person's second message, “echo do X instead”, with the note “Stopped Adam · it starts again from here” under it, and the coder's second turn with its pull request. The Activity panel lists the second turn as Stopped, Started working, Opened pull request #1." width="400"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/mobile-dark-steer-menu.png"><img src="e2e/__screens__/mobile-light-steer-menu.png" alt="A phone: a message is typed in the box and the menu of the split Send button is open above it, with Send and Stop and send, each with a line that says what it does and its keys." width="200"></picture> |

*Stop and send, after the agent restarted (the cancelled task is the first line of the second turn), and the menu on a phone; from the web's mock server.*

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant C as Composer
  participant T as ThreadAgent
  participant O as Orchestrator
  participant L as live-runs.ts and the runtime
  Note over T,L: run-1 is open: the runtime shows the agent's turn
  U->>C: types, presses Send or Ctrl+Shift+Enter
  C->>T: sendWhileWorking(text, steer or interrupt)
  T->>O: POST run-2: one new message, forwardedProps["vymalo.send"]
  O-->>T: RUN_STARTED run-2 (the response is released here)
  O-->>T: the connect stream: SUBAGENT_FINISHED suspended, RUN_FINISHED success for run-1
  T-->>L: the end of run-1 (the turn is complete, not cancelled)
  O-->>T: the connect stream: RUN_STARTED run-2, the message with metadata vymalo.delivery
  T-->>L: an ExternalRun with the message and its delivery
  L->>L: waits until the runtime shows run-1 and is idle, appends the message, starts run-2
  Note over C,L: the bubble carries its note, from metadata.custom.delivery
```

```mermaid
stateDiagram-v2
  [*] --> Idle: the thread is not running
  Idle --> Working: a message (the runtime's own send)
  Working --> Working: Send or Stop and send (a run of its own, the box never locks)
  Working --> Held: the conversation is not on screen yet (replaying)
  Held --> Working: the transcript holds the runs the stream delivered
  Working --> Idle: done, failed, cancelled
  Working --> Answering: the agent asks
  Answering --> Working: the answer (an interrupt's resume)
```

- **Why not the runtime's `append`.** While a run is open the runtime's own send supersedes it: `abortActiveRun()` dispatches
  `RUN_CANCELLED` to the run's message (`incomplete: cancelled`) and detaches from the run, so its later events never arrive
  (*verified 2026-10-02*, `@assistant-ui/react-ag-ui` 0.0.62; the first test of `thread-agent.dom.test.tsx` pins it, for both modes). The
  transcript keeps every message once, but the turn of an agent that is still working would read "Stopped" (`step-tree.ts` takes
  `cancelled` for a stop), and a refused send would leave the rest of the run unseen. `ThreadAgent.sendWhileWorking(text, mode)` is
  the plain `POST` (the same `post` as a run, read to `RUN_STARTED`); the message comes back through the connect stream as a run
  nobody here started, which `live-runs.ts` appends once the runtime is idle (it now also waits for a run the runtime made itself).
- **The guard.** `ThreadSnapshot.replaying` is on from the moment the stream hands a run for the transcript until `live-runs.ts` sees
  the runtime hold what the run leaves (`ThreadAgent.applied`, also when the run failed); with `loaded` it is the composer's
  `sending.ready`. While it is not ready Send and its menu are disabled and Enter does nothing; the text is kept. A plain send (Send, Enter or a
  form submit on a thread that does not work) is not disabled but held: the box keeps the message, the button is `aria-busy` and a live region
  says so, and what the box holds goes out once `ready`, because the import of a thread opened at its end (`HistorySeed`) replaces what the
  runtime holds, a message just sent with it (ADR 0059).
- **The note.** `metadata["vymalo.delivery"]` of the user message's `TEXT_MESSAGE_START` (`steer` or `interrupt`) is read in
  `ThreadAgent.userText` into `ExternalUserMessage.delivery` and put on the runtime's message as `metadata.custom.delivery`; the bubble
  (`delivery-note.tsx`) says "Sent while Adam was working · read at its next step" (the agent's card lists `steer/v1`) or "· read after
  this turn", and "Stopped Adam · it starts again from here". The words are `lib/send.ts`.
- **The mock** plays both modes (`sendWhileRunning` in `mock/server.ts`): `gate …` holds a run until `POST /__mock/release?thread=<id>`,
  `slow …` works until stopped. It lists `steer/v1` for adam only, so both wordings can be tested. A steered message reaches the
  running task at its next step only under the `steerable …` script (the task says `steered: <text>` in the same job, as the
  orchestrator's dispatcher does for an agent that lists `steer/v1`); under every other script it reaches the agent after its turn.
- **Tests.** `thread-agent.dom.test.tsx` (the supersede behaviour, both modes; `sendWhileWorking` on a run opened by another tab and
  by this page; a refused message), `live-runs.dom.test.tsx` (the guard), `composer.dom.test.tsx` (Send, Stop and send, the keys, the
  guard, a refused send, an idle thread, a plain send held until the conversation is shown), `chat-shell-early-send.dom.test.tsx` (the same through the
  app, with the seed held back), `lib/agui/seed-send.dom.test.tsx` (a refused send taken back from a seeded transcript leaves it as it was, on every golden, and
  every message the runtime lists can be looked up; the import replaces a message sent before it), `chat-shell-steer.dom.test.tsx` (the app against the mock), `lib/send.test.ts`, and
  `e2e/steer.spec.ts` with axe, light and dark, on a desktop and a phone.

## Mentioning agents

*Added 2026-10-03 ([ADR 0026](../docs/decisions/0026-agent-mentions-as-structured-references.md), PR-18 of plan 11; the screens are in
[DESIGN.md](DESIGN.md#mentions)).* Typing an **@** that begins a word in the box opens the agents the person may mention; a pick writes
the agent's label (`@<id>`) into the text and a structured reference into the message that goes out
([`mentions-v1.md`](../docs/api/mentions-v1.md)). The orchestrator checks the references before it writes anything; the addressed agent
is told them when its card lists `mentions/v1`.

```mermaid
sequenceDiagram
  autonumber
  actor U as Person
  participant C as Composer (useMentions)
  participant S as MentionsStore
  participant T as ThreadAgent
  participant O as Orchestrator
  U->>C: types "@re"
  C->>C: triggerAt(text, caret), matching(agents): the listbox opens
  U->>C: ArrowDown, Enter
  C->>S: set(text with "@reviewer ", the mention at its UTF-16 offsets)
  U->>C: edits the text around it
  C->>S: sync(text): reconcile(before, after, mentions), a mention whose label changed is dropped
  U->>C: Enter (Send)
  C->>T: the runtime appends the message, or sendWhileWorking(text, mode)
  T->>S: take(text, messageId): the mentions standing in the text that goes out
  T->>O: POST run: message, forwardedProps["vymalo.mentions"]
  alt accepted
    O-->>T: RUN_STARTED
    T->>S: accepted(): what the box held is the log's
    O-->>T: connect stream: the user message with metadata["vymalo.mentions"] (other tabs, a reload)
  else refused (400, 422, 503)
    O-->>T: a problem with its words
    T->>S: refused(messageId)
    S-->>C: the text comes back, and its mentions with it
  end
```

```mermaid
stateDiagram-v2
  [*] --> Typing: an "@" begins a word
  Typing --> Open: agents match the query
  Typing --> [*]: nothing matches, or the word is left
  Open --> Closed: Escape (until the word changes)
  Closed --> Open: the word changes
  Open --> Picked: Enter, Tab or a click
  Picked --> Kept: the label is in the text, the reference in the store
  Kept --> Kept: an edit elsewhere: the offsets move
  Kept --> Dropped: the label is edited, glued to, or removed
  Kept --> Sent: the message goes out with the references that still stand
  Sent --> Kept: refused: the text and its mentions come back
  Sent --> Recorded: accepted: user_message.mentions
  Dropped --> [*]
  Recorded --> [*]
```

- **Who can be mentioned.** The agents `GET /api/agents` lists and the person's roles let them invoke (`agent.invoke`, the same
  filter as the agent menu), **not** the agent that reads the message: the thread's own, or the picker's choice on a new chat (the
  orchestrator answers 422 for it). One that is already mentioned in the box is not offered again. The list is read live; when a send
  is refused with a 422 or a 503 it is read again, and the box's mentions take the card URL of what it says now.
- **The label** is `@` and the agent's **id**, not its name: an id has no space (the word ends where the person's next one begins),
  never collides with another agent's, and is at most 64 UTF-16 code units by construction. The name is on the chip.
- **Offsets are UTF-16 code units** (`lib/mentions.ts`), what a JavaScript string indexes: `text.slice(start, end)` is the label, an
  emoji before it moves it by two. `reconcile(before, after, mentions)` finds the edit by comparing the two texts (common prefix, then
  common suffix, never splitting a surrogate pair), moves the mentions after it, and drops one the edit touches or that no longer
  begins and ends a word (`stands`: "@coderx" is another word). The same function handles typing, a paste over a selection, an undo and
  the runtime trimming what it sends. It is stricter than the orchestrator (which checks the label at the offsets), so what passes is
  never refused for it.
- **Where they ride.** `MentionsStore` is the one place that knows: the composer's hook (`use-mentions.ts`) keeps it in step with the
  box (`sync` on every change of the text) and writes a pick (`set`); `ThreadAgent.post` asks for the text it is about to post (`take`),
  so the answer does not depend on whether the runtime already cleared the box. The same `post` carries the runtime's own sends and
  `sendWhileWorking` (a steered or interrupting message keeps its mentions).
- **A refused send** shows the orchestrator's words above the box (the problem's `detail`), and **the text and its mentions come back**:
  the runtime puts the text back by itself (it is a `MessageNotSentError`) and the store gives the mentions back to the same text; for
  a message sent while the agent works the composer puts them back in front of anything written since (`restoreInFront`).
- **The bubble.** A user message that mentions agents is drawn as plain text with each mention a chip (`mentioned-text.tsx`), because
  the offsets are into the words as typed and Markdown would move them; a message that mentions nobody keeps its Markdown. The mentions
  come from the log (`metadata["vymalo.mentions"]` of the message's `START`, on the runtime's message as `metadata.custom.mentions`: a
  reload, another tab, a message sent while the agent worked) or, for a message this page sent through the runtime (a run does not hear
  its own message), from the store by the message's id. A reference the text does not bear out is not drawn.
- **The warning** (`mentions-warning.tsx`), above the box when it holds a mention, from the addressed agent's live card
  (`useAgentCapabilities`, [ADR 0008](../docs/decisions/0008-platform-integration-via-a2a-extension.md)): no `mentions/v1`, "X does not
  use mentions, so it will not be told who you mentioned. The names stay in your message as text."; `mentions/v1` and no
  `thread-tools/v1`, "X will be told who you mentioned, but it cannot ask other agents."; a card that could not be read, "Could not
  check whether X can work with the agents you mentioned." (never "can"). Send is never disabled.
- **Not while an agent waits for an answer** (`blocked`): the text is a `resume` answer, which has no message to carry references, so
  no "@" opens anything there.
- **Accessibility.** The box is the plain textbox "Message" and says it has suggestions (`aria-autocomplete="list"`,
  `aria-haspopup="listbox"`); while the list is open it is the combobox of ARIA 1.2 (`role="combobox"`, `aria-expanded`,
  `aria-controls`, `aria-activedescendant`) and the focus never leaves it. It is a textbox when closed because every screen and test
  finds the box by that role.
- **The mock** (`mock/mentions.ts`, an independent reading of the contract) plays the 400, the 422 and the 503 as the orchestrator
  does (labels and offsets in UTF-16 code units, overlaps, the roles before the registry, an unknown agent, a moved card, the thread's
  own agent, a registry that cannot answer), records `mentions` in the `user_message` of a new thread, a follow-up and a message sent
  while the agent works, and lists `mentions/v1` for the coder and the verifier and `thread-tools/v1` for the coder only, so the
  three states of the warning can be played.
- **Tests.** `lib/mentions.test.ts` (offsets with emoji, combining marks and surrogate pairs), `lib/store.test.ts`,
  `composer-mentions.dom.test.tsx` (the keys, the edit-around recompute, the payload, a refused send), `thread-agent-mentions.dom.test.tsx`,
  `chat-shell-mentions.dom.test.tsx` (the app against the mock: the warnings, the chips, a 503), the mock's contract tests, and
  `e2e/mentions.spec.ts` (keyboard only, a 422 and a 503 shown, axe in both schemes, desktop and phone).

## Asked agents

*Added 2026-10-03 ([ADR 0026](../docs/decisions/0026-agent-mentions-as-structured-references.md), PR-22 of plan 11; the screens are in
[DESIGN.md](DESIGN.md#asked-agents)).* The agent a thread is addressed to may ask an agent the person mentioned to do part of the work
(`ask_agent`, [`thread-tools-v1.md`](../docs/api/thread-tools-v1.md#ask_agent)); the asked agent may ask one more. The orchestrator tells
each ask as a subagent (`sub-ask-<n>`) nested under the one that asked and as a `vymalo.ask` activity. The web draws **the activity**: one
line "Asked <name>" in the Activity tab, a step of the tree like the others, so the chat's line for the turn, the failed chip and the
panel's deep link work for it without a rule of their own.

```mermaid
sequenceDiagram
  participant O as Orchestrator
  participant T as ThreadAgent (runtime parts)
  participant B as step-tree.ts (buildTurnSteps)
  participant P as Activity tab (AskStep)
  O-->>T: ACTIVITY_SNAPSHOT vymalo.ask ask-1 (running), by main
  T->>B: one data part agui-activity/vymalo.ask in the turn's message
  B->>P: node ask-1 under the turn: "Asked Adam", Working
  O-->>T: ACTIVITY_SNAPSHOT vymalo.ask ask-2 (running), by ask:1
  B->>P: node ask-2 under ask-1 (askParent: parentStepId, else by)
  O-->>T: ACTIVITY_SNAPSHOT vymalo.step, path ask-2
  B->>P: the step under ask-2 (the tree files it by the last id of its path)
  O-->>T: ACTIVITY_SNAPSHOT vymalo.ask ask-2 (failed, error), then ask-1 (completed)
  T->>B: the same parts, replaced in place by the runtime
  B->>P: ask-2 Failed with why, ask-1 Answered with its chip "1 failed" while closed
```

```mermaid
stateDiagram-v2
  [*] --> Working: running (spinner)
  Working --> Answered: completed
  Working --> AskedBack: input_required, auth_required (waiting, the question stays under the line)
  Working --> Failed: failed, rejected, timed_out (the reason stays under the line)
  Working --> Stopped: canceled, or the turn ended with the ask still open
  Answered --> [*]
  AskedBack --> [*]
  Failed --> [*]
  Stopped --> [*]
```

- **Where it is read.** `parseAsk` (`lib/agui/vymalo.ts`), `askNode`, `askParent` and `askStepState` (`lib/step-tree.ts`), the words in
  `lib/ask.ts`; the line is `AskStep` in `components/steps/step-node.tsx` and what it opens `ask-details.tsx`. The part is one of the
  turn's step parts (`lib/steps.ts`), so a turn that only asked is a turn with steps.
- **Nesting.** Under the step the call named (`parentStepId`, matched exactly, else by the one step whose id ends in `/<parentStepId>`:
  the log's ids carry the task), else under the ask that asked (`by: ask:<n>`), else under the turn. The steps an asked agent relayed
  (path `["ask-<n>"]`) file under its ask by the last id of their path, as every step does. An ask whose asker is not in the turn is
  never lost: it sits under the turn.
- **Names.** "Asked Adam": the name from the agent list the page already reads (`AgentNamesProvider` in `chat-shell.tsx`, the map
  in `TurnView.agentNames`; a turn is rebuilt when the names it was built with change), the id when the list has none.
- **A failure is visible at every level.** The ask that failed says "Failed" and why on its own line, closed; every ask above it that
  is closed carries the "1 failed" chip (`countUnder`); the turn's header and the chat's line count it, and the chip of the chat's line
  opens the panel on it (`firstFailed`). A completed parent hides nothing.
- **An ask nobody runs.** A turn that ended holds no running step: an ask the log did not end reads "Stopped" (and "Waiting" while its
  turn waits for the person), never "Working". The orchestrator ends asks before their asker's task does, so this is a copy that was cut.
- **Untrusted text.** What was asked, the answer, the question and the reason are an agent's words: text nodes, an artifact is a link only
  when its address is an absolute http(s) one (`safeHttpUrl`).
- **The mock** plays it: `ask-agent` (the Reviewer, the Verifier it asks under it with a search step under that ask, both answer; then
  the Verifier is asked and fails) and `ask-hold` (the same held while two asks run, until `POST /__mock/release` or Stop, which ends
  them canceled). Its projection (`mock/projection.ts`) is the orchestrator's, held to the `ask-agent` golden; the contract tests play
  both scripts and check the frames, the nesting, the order of the ends and the cancel.
- **Tests.** `lib/ask.test.ts`, `parseAsk` in `lib/agui/vymalo.test.ts`, the ask cases of `lib/step-tree.test.ts` (nesting, `by` and
  `parentStepId`, every state, a failed child, a rebuild on new names), `ask-step.dom.test.tsx` (every end state in words, a failure at
  every level, the disclosure, untrusted text), `ask-agent.dom.test.tsx` (the `ask-agent` golden through the real runtime into the panel:
  the collapsed line with its spinner, the nesting, the end states, the failure), the mock's contract tests, and `e2e/asks.spec.ts`
  (the closed and open lines, the asks that run until released, Stop, the keyboard, a reload, axe in both schemes, desktop and phone).

## Who you are and what you may do

*Added 2026-10-02 (S17; [ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md)).* Roles map to
permissions in the orchestrator; the web asks `GET /api/me` once per page load and follows the answer. **It is never a check.** The
orchestrator refuses what a role may not do whatever the page shows, so a page that shows too much costs a 403 with the server's
words, and one that cannot read `/api/me` (a 401 that is being left, a network error, an orchestrator from before roles) shows
everything, as before roles (`unknown` in `use-me.ts`). The rules are pure functions in `features/me/lib/access.ts`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-read-only.png">
  <img src="e2e/__screens__/desktop-light-read-only.png" alt="A person whose role reads and does not write, on a finished thread of their own. Where the message box would be there is one line with an eye, “Read only: your roles do not let you write in threads.”, and the top bar has a chip with an eye (named Read only) beside the check of the Done state." width="720">
</picture>

*A role that reads and does not write, on its own thread, from the web's mock server (`POST /__mock/config?me=read-only`).*

| No access |
|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-no-access.png"><img src="e2e/__screens__/desktop-light-no-access.png" alt="A page that says No access: the panda, then “You are signed in as nobody@example.com (Nina Nobody), and none of your roles gives access to this app.” and a line on asking for a role." width="400"></picture> |

- **A thread is read-only for the person** (`threadAccess`) when no role of theirs holds `thread.write`, or when
  `agent.invoke` does not cover the thread's agent. **Nobody reads another person's thread, an administrator included**
  ([ADR 0039](../docs/decisions/0039-nobody-reads-another-persons-thread.md), which reversed the "administrators read every thread" of
  S17): a link to another's thread is the page of a thread that does not exist (`Thread not found`, a 404 for every role), so there
  is no "someone else's thread" to say. The message box is then a line, `Read only: your roles do not let you write in threads.`
  (a status: words and an eye, never only a colour; `read-only-notice.tsx`), with a **Read only** chip in the top bar (the eye, named and hinted "Read only"). Rename and
  Add or Edit description are disabled in the menu (the reason is their title; Export JSON is a read and stays), the
  turn's Fork from here and the Edit of a message are not drawn, the agent menu's other agents are disabled with the same
  words, and the actions of a card (Choices, buttons) are off and say so (`SurfaceHost.readOnly`). The open thread is
  `ForkProvider`'s `readOnly` and `SurfaceHostProvider`'s, which is how the actions go off without each one asking.
- **The agent picker** of a new chat lists the agents `agents.invoke` covers (`["*"]` is every one); a thread's own agent is
  named in its top bar whatever the roles say. A role without `thread.write`, or with no agent to invoke, has a line where
  the box would be: `Your roles do not let you start chats.`
- **The thread list is the person's own, for every role** (`GET /api/threads`, with no `owner`: the orchestrator answers 400 to one,
  ADR 0039). The Mine / All threads switch of S17 and the owner line under a row are gone, and `isAdmin` with them: the web draws
  nothing from the `admin` permission yet, which is operational and content-free. A choice an earlier version kept in
  `localStorage` (`another-agentic.thread-scope`) is never read.
- **No access.** A person whose `permissions` are empty gets every route but `GET /api/me` as 403 `no_access`, so `ChatShell` shows a
  screen that says who they are signed in as and what to do (`no-access.tsx`), not a list of errors. It is drawn once
  `/api/me` has answered; until then the chat is, as it always was, so a person who is let in sees nothing new, and one
  who is not sees the chat's first requests fail for a moment before the screen replaces it.
- **The other 403s** (`forbidden`) are the problem's `detail`, where the action was: the composer's error line
  for a send, a card's action and Stop, "Could not rename the thread", "Could not fork the chat".
  They keep the field or the draft, as every refused action does.

### Signing in again

*Reworked 2026-10-04, on the owner's request ("Token refresh should work without a full page refresh"); until then any 401 left the
page for the sign-in and the page, its stream and the message in the box were lost ([ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md),
item 5 of the S17 notes, amended).*

> **Two kinds of deployment** ([ADR 0054](../docs/decisions/0054-the-web-holds-its-own-tokens-dpop-bound-in-indexeddb.md)). The page asks
> `GET /api/public/auth` once per load, before its first API call. A `404` (or a network error) is an **edge** deployment: everything
> below, the cookie of oauth2-proxy, as it always was. A `200 {issuer, clientId, scope}` is **browser mode**, where the web holds its own
> tokens: read [Signing in itself](#signing-in-itself-browser-mode) for what replaces the edge's pieces; the banner, the park, the three
> sends and `SessionChangedError` are the same.

A **401** from the orchestrator or the edge is a session the edge no longer accepts: `GET /api/me`, the thread list, the connect
stream's reconnect, a run's POST, any call. Most of them are **not** a person who has to sign in again: they are a session the edge
could have renewed (see [why](#why-a-401-happens)). So the page, in this order, **keeps the session warm**, **refreshes it on a 401
and sends the request again** (a request is sent **up to three times**: the original, after a refresh, and after being held while the person
signs in), and only when the edge has no session **asks the person, in a popup, without leaving the page**. All
of it is **opt-in with the edge's sign-in**: it needs `NEXT_PUBLIC_SIGN_IN_PATH` (a path of this origin, `/oauth2/start` in the
chart's image and the dev stack, `/oauth2/sign_in` works too), a build-time variable because Next inlines `NEXT_PUBLIC_*`:
`docker build --build-arg NEXT_PUBLIC_SIGN_IN_PATH=/oauth2/start -f web/Dockerfile .`. A value that is not a path of this origin
(`https://…`, `//host/…`) counts as unset. Without it a 401 is the error line it always was (`Could not load agents: missing
X-Auth-Request-Email`, which `e2e-system/auth.spec.ts` expects behind a proxy header), nothing is asked of the edge and nothing waits.

```mermaid
sequenceDiagram
  autonumber
  participant P as Page (api client, ThreadAgent)
  participant E as Edge (Caddy and oauth2-proxy)
  participant O as Orchestrator
  participant W as Sign-in popup
  Note over P,E: keeping warm, while the page is visible and used: every 4 minutes, and when the window is back after a minute
  P->>E: GET /oauth2/userinfo (the cookie)
  E-->>P: 200, and Set-Cookie when a session older than --cookie-refresh was refreshed
  P->>E: a call (cookie)
  E->>O: forward_auth /oauth2/auth, then the call with the ID token
  O-->>P: 401 (the edge's own 401 on forward_auth is the same to the page)
  P->>E: GET /oauth2/userinfo (single flight: one question for any number of 401s)
  alt the edge has a session
    E-->>P: 200 (refreshed, cookie renewed)
    P->>E: the same call again (the second send)
  else the edge has none
    E-->>P: 401
    P->>P: status "ended": the banner, the call is held (at most 10 minutes)
    P->>W: Sign in: window.open /oauth2/start?rd=/signed-in
    W->>E: the issuer approves, back to /signed-in
    W-->>P: BroadcastChannel "signed-in", then the popup closes itself
    P->>E: GET /oauth2/userinfo (the message is not believed, the edge is asked anew)
    E-->>P: 200 and whose session it is
    alt the same person as before
      P->>E: the held call (the third send)
    else somebody else
      P->>P: the held calls are rejected, nothing is sent as them, the page is read again
    end
  end
```

```mermaid
stateDiagram-v2
  [*] --> ok
  ok --> ok: a 401 and the edge has a session: refreshed, the call goes again
  ok --> ended: a 401 and the edge has none, or a call still refused after a refresh
  ended --> ok: the edge has a session of the same person (the popup says so, the window comes back, an interval asks)
  ended --> changed: the edge has a session of somebody else
  ok --> changed: a question finds another person's session than the page has been
  changed --> [*]: the held calls are rejected and the page is read again
  ended --> ended: a call waits; after 10 minutes its 401 is the caller's
```

| Piece | Where | What it does |
|---|---|---|
| Keep warm | `keepSessionWarm` (`src/lib/api/session-refresh.ts`), mounted by `useKeepSessionWarm` in the chat shell | `GET /oauth2/userinfo` (`credentials`, `no-store`, `redirect: manual`) every `KEEP_WARM_MS` (4 minutes, shorter than the chart's `--cookie-refresh=10m` and the 15-minute token) and when the window is back (focus, visible) after `FOCUS_GAP_MS` (a minute), and once when the page starts (that answer says whose session it is, which the page remembers). The endpoint is the sign-in's own prefix and `userinfo` (`refreshPath()` in `session.ts`: `/oauth2/start` gives `/oauth2/userinfo`, and a moved `--proxy-prefix` moves both). **Only while the page is visible and used**: a hidden tab asks nothing, and neither does one nobody has touched (no key, pointer, wheel or touch, no return to the window) for `IDLE_LIMIT_MS` (30 minutes), see [the idle bound](#the-idle-bound). It runs only in the signed-in app, never on a shared page |
| Refresh on a 401 | `withSessionRefresh` wraps the `fetch` of `api` (`client.ts`) and of `ThreadAgent`'s client | A 401 asks the edge once (**single flight**: three calls that meet it together are one question), and a 200 sends the call again (the second send). The body is cloned before the first send, so a run's POST goes again whole, every time. **Why a resend is safe, a POST included:** a 401 means no handler ran. The edge's `forward_auth` refuses before the call reaches the orchestrator, and the orchestrator's own 401 (an ID token that expired in between) is the identity layer's: `require_identity` wraps every route and refuses before a handler runs (`orchestrator/crates/api/src/auth.rs`), no handler answers 401 (pinned by `orchestrator/crates/api/tests/only_identity_answers_401.rs`), and an agent's own 401 reaches the web as a 502 (`orchestrator/crates/api/src/problem.rs`), so a 401 never follows work that was done. A call still refused after a refresh says the session has ended. An answer that says nothing (the edge is down, the network) leaves the 401 as it is: no banner, no wait. A 401 whose call is sent again has its body let go (`cancel`) first |
| Ended | `SessionBanner` (`features/session`, in the root layout) | One line over the page, "Your session has ended" and a **Sign in** button, a `status` (not an `alert`: the page did not fail). Every call that needs the session waits (`PARK_MS`, 10 minutes, then its 401 is its own and the page's error line says it); the page, its stream, a draft and a message in the box stay where they are. It is gone when the edge says there is a session: the popup's last page says so, a window that comes back asks, the interval asks |
| Sign in | `openSignIn` (`session.ts`), the button's click | `window.open` of `<path>?rd=/signed-in` (`SIGNED_IN_PATH`), cut off from the page (`opener = null`), so the issuer's page cannot reach back. `/signed-in` (`app/signed-in`, `features/session/components/signed-in.tsx`) says `signed-in` on a `BroadcastChannel` and closes itself, or, in a window a script did not open, says so with a link to the chat. **The message is never believed**: the banner asks the edge |
| Last resort | `redirectToSignIn` (`session.ts`) | The old full-page redirect, `<path>?rd=<this page's path, query and hash>`, **only** when the browser refuses the popup (the button's click), at most once in `REDIRECT_PAUSE_MS` (30 s, `sessionStorage`): a sign-in that does not help is a deployment that is wrong, and a loop of redirects would hide it, so the banner says "Signing in did not help a moment ago" (also when the session ends again within that pause of a popup sign-in, whether or not a redirect was held back). The shared page's own redirect for a link that is not public (`resolveShare`) is this function too |
| Somebody else | `renewSession` (`session-refresh.ts`) | The edge's `userinfo` says whose session it is (`email`, else `user`, lower-cased; kept in memory, never shown). When a session comes back as **another person** than the page has been (a different person signed in in the popup or in another tab), nothing held is sent as them, a run's POST included: the held calls are **rejected** (`SessionChangedError`) and the page is read again (`navigation.reload()`, once). A message of the popup starts a **fresh** question after one already on its way (that one started before the cookie was set). A message sent while the agent works (`sendWhileWorking`) that is held is let go when the page stops (`ThreadAgent.stop()` aborts it) |
| Never | `readerApi` (`client.ts`), the public reader's `ThreadAgent` | A shared page's client is plain `openapi-fetch` ([ADR 0040](../docs/decisions/0040-thread-sharing-by-revocable-link.md)): a 401 is a question there (the public route is tried next), never a session that ended, so nothing is asked of the edge, no banner, no sign-in |

#### Why a 401 happens

*Found 2026-10-04 by reading the chart, the Caddyfile and the sources of the versions the chart pins; **not observed on the live
deployment** (no cluster or container runtime was available). What was run is the web against the mock, which plays the edge's
two routes.*

Every call the page makes goes through Caddy's `forward_auth`, which asks oauth2-proxy `GET /oauth2/auth`
(`deploy/chart/files/Caddyfile`). oauth2-proxy refreshes a session older than `--cookie-refresh` (10 minutes in the chart) on such a
request, and it does: the ID token it forwards is the new one, so the call succeeds. But the renewed session is saved as a
`Set-Cookie` on **that subrequest's** response, and Caddy's `forward_auth` passes on a 2xx only the headers named in `copy_headers`
(`Authorization`): **the cookie never reaches the browser.** The browser keeps the cookie it got at sign-in, and every call after the
first 10 minutes makes oauth2-proxy redeem that same, original refresh token again (a round trip to Keycloak per call, with parallel
calls in a race: the cookie store takes no lock). It works until the issuer stops accepting that token. The likelier reason is
that the token the stale cookie holds has an expiry of its own, about *SSO Session Idle* after the sign-in (*unverified*: the default
of 30 minutes, and this realm's value, are not known here), so a person who has been working for about that long is signed out however
busy they were; a token revoked by its own use (Keycloak's *Revoke Refresh Token*, see below) or a session that idled out are the other
ways. The redeem then fails with `invalid_grant`, oauth2-proxy takes that as fatal, **clears the session** and answers 401, which used
to be the full-page redirect. The only requests whose `Set-Cookie` reaches the browser are the ones that go to oauth2-proxy
**directly** (`handle /oauth2/*` in the Caddyfile): that is the request the page now makes itself, on an interval and on a 401.

To see it on the deployment: oauth2-proxy logs `Refreshing session - User: …` on **every** call after the first 10 minutes of a
session (it should be once per 10 minutes), and `Unable to refresh session … invalid_grant` just before a person is signed out.

What was *verified*, 2026-10-04, by reading the sources at the pinned tags (a read, not a run):

| Fact | Source |
|---|---|
| A session is refreshed only when it is older than `--cookie-refresh` (`needsRefresh`: `session.Age() > refreshPeriod`), on a request that goes through the session chain: the proxy `/`, **`/oauth2/auth`**, **`/oauth2/userinfo`** and `/oauth2/sign_out`, not `/oauth2/start`, `/callback`, `/sign_in`, `/static` or `/ping` | oauth2-proxy v7.15.5 `oauthproxy.go` (`buildServeMux`, `buildProxySubrouter`, `buildSessionChain`) and `pkg/middleware/stored_session.go` |
| The refreshed session is saved with the response of that same request (`store.Save(rw, req, session)`), which for the cookie store is a `Set-Cookie` on `rw`; the ID token is replaced when the refresh returns one, and kept when it does not | `stored_session.go` `refreshSession`; `providers/oidc.go` `redeemRefreshToken` |
| On a 2xx from the auth service, Caddy copies only `copy_headers` to the request and drops the rest of the auth response, `Set-Cookie` included; on any other status the auth response goes to the client | Caddy v2.11.4 `modules/caddyhttp/reverseproxy/forwardauth/caddyfile.go` (the one the chart pins; the dev stack runs the same tag) |
| `/oauth2/auth` and `/oauth2/userinfo` answer **401** (plain `Unauthorized`) when there is no session, never a redirect; a redirect to the issuer is the proxy `/`'s answer for a browser, and even there a request with `Accept: application/json`, `--api-routes` or `--force-json-errors` gets a 401. In the chart, `/api/*` and `/agui/*` are therefore 401 (oauth2-proxy's, passed on by `forward_auth`) and only the web's catch-all turns that 401 into a 302 to `/oauth2/start` (the Caddyfile's `handle_response`) | `oauthproxy.go` `AuthOnly`, `UserInfo`, `Proxy`, `isAjax`; [docs, endpoints](https://oauth2-proxy.github.io/oauth2-proxy/features/endpoints); `deploy/chart/files/Caddyfile` |
| A refresh error that contains `invalid_grant` or `invalid_client` is fatal: the session is cleared and the request has none (401). Another error (the issuer unreachable) keeps the session, which is then valid only until its access token's `exp` | `stored_session.go` `isFatalRefreshError`, `refreshSessionIfNeeded`, `validateSession` |
| The cookie store has no lock: `SessionState.Lock` is a `NoOpLock` unless a store sets one, so parallel calls refresh in parallel | `pkg/apis/sessions/session_state.go` (`ObtainLock`) and `pkg/sessions/cookie/session_store.go` (sets none) |
| Keycloak: *SSO Session Idle* "resets when clients request authentication or send a refresh token request"; *Revoke Refresh Token*: "Keycloak revokes refresh tokens and issues another token that the client must use" | [Keycloak server admin guide, Timeouts](https://www.keycloak.org/docs/latest/server_admin/index.html) (the docs' latest at the date; this realm's values are *unverified*, and so are the defaults) |

**The web-only fix assumes Keycloak's *Revoke Refresh Token* is OFF** (refresh-token rotation; *unverified* for this realm). With rotation
on, a refresh token that was rotated by a `forward_auth` subrequest is lost (its new value never reaches the browser's cookie) and
the next refresh with the old one is refused, so people would be signed out about every 10 to 14 minutes whatever the page does, and
**a Redis session store for oauth2-proxy becomes required** (the cookie then holds a ticket and the rotated token is kept
server-side). Check the setting before relying on the page's refresh alone.

#### The idle bound

*A security trade-off, recorded 2026-10-04; the owner may change it.* Every keep-warm question after `--cookie-refresh` is a refresh
grant, and a refresh resets Keycloak's *SSO Session Idle* ([the docs](https://www.keycloak.org/docs/latest/server_admin/index.html):
the timeout "resets when clients request authentication or send a refresh token request"). A page that asked for ever would therefore
keep a session alive for ever in a tab nobody uses, which the issuer's idle timeout exists to stop. The default: the page keeps the
session warm **only while the tab is visible and the person has been there in the last 30 minutes** (`IDLE_LIMIT_MS` in
`session-refresh.ts`; a key, a pointer move or press, a wheel or touch, or a return to the window counts). A hidden or untouched tab asks
nothing, so its session ages out as the issuer says, and the person finds the banner when they come back (which also asks at once, so
a session that is still good is renewed without a word). The stream's reconnect that meets a 401 **still refreshes**: it is a
person's work being followed, not an idle page, and it is bounded by the issuer's own limits (*SSO Session Max*). Lengthen the
constant to trade security for fewer banners, set it to 0 to keep warm only on a 401.

What the chart could change, and what is **not** needed for the web's side to work: the session is renewed in the browser's cookie
only by a request to `/oauth2/*`, which the page now makes. Storing sessions in Redis (`--session-store-type=redis`, the cookie is
then a ticket) would make the dropped `Set-Cookie` harmless and stop the repeated redeem on every call; it is *unverified* here and
costs a Redis. Both `--cookie-refresh` (shorter than the token) and the realm's *SSO Session Idle* (longer than a person is away)
remain deployment settings; the web does not read them. The dev stack sets no `--cookie-refresh`, its mock issuer hands out no
refresh token and its tokens last an hour ([ADR 0033](../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md) S16
item 6), so there a question to `/oauth2/userinfo` is a 200 that changes nothing, and the ended state is reached by clearing the cookie.

*Verified 2026-10-02 (S16)*, against oauth2-proxy v7.15.5 with the dev stack's flags: both `/oauth2/start` and `/oauth2/sign_in` take
`rd` with a relative path, query included (`/oauth2/start?rd=/threads/x%3Fa%3D1` signs in at the issuer and lands on
`/threads/x?a=1`), and an absolute or `//host` `rd` is refused (the person lands on `/`). The popup's `rd=/signed-in` is such a path.
The dev stack builds the web with `/oauth2/start` (`compose.yaml`, `build.args`;
[`dev/README.md`](../dev/README.md#sign-in-a-mock-issuer-and-oauth2-proxy)). **Unverified:** the popup against a real oauth2-proxy and
Keycloak in a browser (it is run against the mock's `/oauth2/start`, which signs in and redirects as oauth2-proxy does for a relative
`rd`), and whether the issuer's pages set a `Cross-Origin-Opener-Policy` (the page does not depend on it: it hears of a sign-in by
`BroadcastChannel`, which is same-origin, and by asking the edge when its window comes back).

*Tests.* `src/lib/api/session-refresh.test.ts` (a refresh then the second send; a POST refused, refreshed, refused again, held and
sent after the sign-in carries its whole body each time, which a body used twice would fail; one question for three 401s; a refresh
that says nothing; no session: held, then sent when the person is back, or let through after `PARK_MS`; aborted while held; a
different person signing in: the held run is rejected, nothing is sent as them and the page is read once; the sign-in message asks
anew; ended again soon after a sign-in; no redirect anywhere; no sign-in path: nothing asked; keep warm: once at the start, the
interval is shorter than the cookie-refresh, a focus within a minute asks nothing, nothing is asked while hidden or after
`IDLE_LIMIT_MS` without input), `session.test.ts` (`refreshPath`, `openSignIn`: popup, refused popup and the 30-second pause), `client.test.ts`
(`api` asks, `readerApi` never does), `thread-agent-session.dom.test.ts` (the connect stream: refreshed and reopened; held while
the person signs in; a run's POST refused on a stale session is sent again whole and accepted once; a held message is let go when the
page stops; a public reader never asks), `chat-shell-roles.dom.test.tsx` ("a 401": the whole app against the mock's stand-in
for the edge), `features/session/components/session-banner.dom.test.tsx`, and **`pnpm test:e2e:session`**
(`e2e/session-refresh.spec.ts`, its own build with the sign-in built in, [Tests](#tests)): in Chromium a refreshed call, an interval
that keeps the session warm (Playwright's clock), the popup that closes itself, the refused popup that leaves for the sign-in and comes
back, a run sent on a stale session (the POST is refused, the edge asked once, the POST sent again, its answer shown and the mock's log
holding the message once), and a public reader of a shared page that is never asked anything; each checks that the draft in the box and a mark on `window`
survived.

### Signing in itself (browser mode)

*ADR 0054, accepted 2026-10-07, on the owner's request for tokens kept by the browser, an offline token and a refresh token that is really
used. The owner chose it over sessions kept by the edge knowing that a script that runs in the page can use what the page holds.*

The orchestrator names the issuer at `GET /api/public/auth` (`auth.browser`); the web is a **public client** of it
(`another-agentic-web`, PKCE, **DPoP required**). Everything is in `src/lib/auth/`:

| Piece | File | What it does |
|---|---|---|
| Kind of deployment | `config.ts` | One request per page load; `null` is edge mode. An issuer that is not `https:` (or `http:` on loopback) is edge mode too. `authReady()` is what every client awaits. |
| Storage | `db.ts`, `keys.ts` | Dexie database `another-agentic-auth`: `keys` (the ES256 pair, **non-extractable**), `session` (access token, expiry, refresh token, `sub` and `email`, one row per issuer and client), `pending` (state, PKCE verifier, return path, mode; gone after 10 minutes). Nothing in `localStorage`, a cookie or a log. The database is opened on first use and never at import; `authDbExists()` asks the browser first. |
| Sign in | `sign-in.ts`, `app/auth/callback` | Discovery, Authorization Code with PKCE S256 and a state, scope from the config, a full-page redirect; the callback exchanges the code with a DPoP proof (`oauth4webapi`), stores the session and returns to a same-origin path (`safeReturnTo`). Opened as the popup of the banner it posts on the `another-agentic.signed-in` channel and closes itself (`/signed-in` stays for the edge). |
| Every request | `fetch.ts` | `Authorization: DPoP <token>` and a proof (`typ dpop+jwt`, ES256, the public `jwk`, `jti`, `htm`, `htu` = origin and path, `iat`, `ath`), **new for every send**, so a held or repeated request is signed again. Applies to `/api/*` and `/agui/*` of this origin only. An `invalid_dpop_proof` 401 is sent once more with a new proof; an `invalid_token` one is the session's 401. |
| Refresh | `tokens.ts`, `lock.ts` | When the access token has **under 60 s** left or the orchestrator refused it: inside the Web Lock `another-agentic.auth.refresh`, the row is **read again** (another tab may have refreshed), then the refresh grant with a proof. A rotated refresh token replaces the old one. `invalid_grant` ends the session (a tombstone row, so "refused" is told from "never signed in"); a network error or a 5xx says nothing about it. |
| The clock | `clock.ts` | Proofs are stamped with the server's time. The offset comes from the `Date` of same-origin orchestrator answers (the first is `/api/public/auth`) and from the `iat` of every access token received. The issuer's own `Date` is never relied on: a page cannot read it across origins. |
| Sign-in screen | `features/session/components/sign-in-screen.tsx`, `lib/auth/sign-in-need.ts` | *Amended 2026-10-09 (ADR 0054), on the owner's "a proper login, not something deployed but hidden".* The page **never leaves for the issuer by itself**. `SignInGate` (around the chat's pages, and the share page's) stands in for the app with the panda, a line naming the organisation (the Keycloak realm of the issuer, else its host: "Sign in with your vymalo account at auth.verif.fyi.") and one **Sign in** button, which starts the redirect, when this browser holds no sign-in (asked of IndexedDB without creating it), when a request or the first question finds nobody signed in, and, saying "Your session has ended", when the issuer refuses the stored refresh token before the page had any use of it (a person back after it lapsed). An issuer that cannot be reached is a line under the button. A share link that is not public, read by nobody signed in, is the same screen; a public reader never sees it. With an edge none of it exists. |
| Sign out | `sign-out.ts`, `features/session/components/account-menu.tsx`, `app/auth/sign-out` | Revokes the refresh token (RFC 7009, with a proof, when the issuer has the endpoint), deletes the three tables, and goes to `end_session_endpoint` with `client_id` and `post_logout_redirect_uri` = this origin; the start page is then the sign-in screen. **The account menu** at the foot of the sidebar (initials, name and e-mail, a tooltip that says who is signed in) has **Sign out**, and so has the no-access screen; with an edge they go to oauth2-proxy's `/oauth2/sign_out?rd=/` (`edgeSignOutUrl`), whose `--backend-logout-url` ends Keycloak's session too (`deploy/chart/README.md`, "Signing out"). `/auth/sign-out` stays a page that asks first (a page that signed a person out when opened could be opened by any other page). |
| The session machinery | `lib/api/session-refresh.ts`, `session.ts` | In browser mode a "ping" is `getAccessToken` (**single flight** as before), "gone" is a refused refresh token, and whose session it is is the token's `email`, else `sub`, lower-cased. Nobody signed in, or a refusal before the page had a session, is the sign-in screen (no banner); a refusal while the page is in use is the banner, whose **Sign in** opens the popup and keeps the page. A refused refresh token is asked once: every request after it waits for the person, and nothing asks the issuer again until they sign in. |

| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-sign-in.png"><img src="e2e/__screens__/desktop-light-sign-in.png" alt="The sign-in screen: the panda mark over the name another·agentic, the line “Sign in with your acme account at auth.example.com.” and one Sign in button, on an otherwise empty page." width="400"></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-sign-in-ended.png"><img src="e2e/__screens__/desktop-light-sign-in-ended.png" alt="The same screen for a person whose sign-in lapsed: “Your session has ended. Sign in again to go on.” above the organisation's line and the Sign in button." width="400"></picture> |
|---|---|

*The sign-in screen, and the same after a sign-in lapsed: screenshots of the web's mock server (`e2e/screens.spec.ts`), the issuer `https://auth.example.com/realms/acme`.*

**Keeping warm is no timer here.** An access token lives five minutes and every request refreshes it when it needs to, so the page
learns whose session it is at the start and checks again when the window comes back after a minute; an untouched or hidden tab asks
nothing, ever. The idle bound is the issuer's own *Offline Session Idle* (30 days by default), which is the point of an offline token:
[The idle bound](#the-idle-bound) is edge mode's.

**What never carries a token.** The public reader of a share link (`/api/public/*`, `/agui/public/*`, `readerApi`'s public routes and the
public `ThreadAgent`) sends no `Authorization` and no `DPoP` and opens no IndexedDB. The reader of a link is tried signed in first, but
only if a database exists (`readerFetch`): a visitor who never signed in sends nothing to the signed-in route and leaves nothing in the
browser.

**Files are fetched, not linked** (`features/chat/lib/file-access.ts`, `hooks/use-object-url.ts`). An `<img src>` or `<a download>` cannot carry a
header, so in browser mode the image of a kept file, its text preview and its download are fetched through the session and shown from an
object URL (revoked when the component goes). In the Sources tab **open** shows an image inside the app, in a dialog, and downloads
anything else; a `blob:` URL is never navigated to (it has the page's origin and none of the server's headers, so an SVG opened as a page
would run). A public link's files and edge mode keep their links.

**Content security policy** (`src/lib/csp.ts`, for every page in both kinds of deployment; *the nonce of this paragraph was replaced on
2026-10-09 by script hashes, see [Static export](#static-export)*): `default-src 'self'`,
`script-src 'self' 'nonce-<per request>' 'strict-dynamic'`, `style-src 'self' 'unsafe-inline'`, `connect-src 'self' <issuer>`,
`img-src 'self' data: blob:`, `font-src 'self' data:`, `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self' <issuer>`,
`object-src 'none'`. The issuer's origin is read **when the server starts** from `WEB_CSP_CONNECT_SRC` (space-separated origins, empty by default;
trusted operator input, see [Static export](#static-export)) because it is not known when the image is built; the chart sets it. Pages are rendered per
request (the root layout awaits `connection()`) so that the nonce is new every time and our two inline head scripts carry it. `next dev`
adds `'unsafe-eval'` and `ws:` only. The static export of ADR 0047 will need hashes instead of a nonce before it ships.
Zod probes for `eval` with `new Function("")`, which a policy without `unsafe-eval` reports even though the throw is caught: two small
patches (`patches/zod@*.patch`, [`UPSTREAM.md`](patches/UPSTREAM.md)) skip the probe.

*Tests.* `src/lib/auth/auth.test.ts` (PKCE and state, a key that cannot be exported, a replayed callback, an expired sign-in, the clock, a
refresh once however many ask, the row read again inside the lock, rotation, reuse, `invalid_grant`, an unreachable issuer, sign-out),
`fetch.test.ts` (every header of a request verified as the orchestrator does, a new proof each time, the retry on `invalid_dpop_proof`,
no token and no IndexedDB for public routes and for a reader who never signed in, edge mode untouched, nobody signed in is the sign-in
screen and no redirect), `config.test.ts`, `lib/api/session-refresh.browser.test.ts` (ping, banner and park, person switch, keep warm
without a timer, a refused refresh token asked once however many requests wait, the screen for nobody and for a lapsed sign-in),
`features/session/components/sign-in-screen.dom.test.tsx` and `account-menu.dom.test.tsx`, `session.test.ts`, the
files' `kept-file-card.dom.test.tsx`, `lib/csp.test.ts`, `mock/browser-auth.test.ts` (the module against the mock's issuer over real HTTP),
and **`pnpm test:e2e:browser`** (`e2e/browser-auth.spec.ts`, its own build and mock, [Tests](#tests)): sign-in through the mock issuer, data
with DPoP on every call, a reload that stays signed in, a browser clock an hour off, silent refresh, a revoked refresh token (banner,
popup, the held request sent), two tabs and one refresh, a file as a blob, sign-out from the account menu leaving IndexedDB empty, a public
share reader with no token and no database, and zero CSP violations throughout; `pnpm test:e2e:session` adds the account menu's sign-out at the edge; `e2e/csp.spec.ts` does the policy and its violations for an edge deployment.

## Share a conversation

*Added 2026-10-03 (S-B3; [ADR 0040](../docs/decisions/0040-thread-sharing-by-revocable-link.md), build step 2).* The owner of a thread can
share it by a link that they can take back: people with the link **read** it, as it goes on, and cannot write in it. A thread is private
until it is shared, and what it may be shared as is capped by the deployment (`disabled`, `internal`: people who are signed in, or
`public`: anybody). The orchestrator holds the cap, the permission and the link; the web asks and shows (`src/features/sharing/`).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-share-dialog.png">
  <img src="e2e/__screens__/desktop-light-share-dialog.png" alt="A finished thread with the Share this conversation dialog open over it. Three choices, Private, Signed-in people with the link (picked) and Anyone with the link, which carries a warning that anyone with the link can read the conversation, including what was pasted in it, and that the owner's e-mail is not shown. Under them the link in a field with a copy icon button, then a new-link icon button and the Stop sharing button, and at the bottom Done and Save.">
</picture>

*The share dialog on a thread shared with signed-in people, from the web's mock server (`POST /__mock/config?sharing=public`).*

| The thread, shared | The page of the link | A link that does not work |
|---|---|---|
| <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-share-badge.png"><img src="e2e/__screens__/desktop-light-share-badge.png" alt="The thread of the dialog, closed. The top bar has a chip that is the icon of people (named Shared · signed-in) beside the check of the state Done, and the thread's row in the list has the same icon after its title."></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-shared-page.png"><img src="e2e/__screens__/desktop-light-shared-page.png" alt="The same conversation as another signed-in person reads it at its link: a top bar with the title, the check of the state Done, the details button and the copy-link icon button, a banner that says Shared conversation, read only, and the conversation with no message box under it."></picture> | <picture><source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-shared-gone.png"><img src="e2e/__screens__/desktop-light-shared-gone.png" alt="A page with the panda and one heading, This link does not work, and a line that says it may have been copied wrongly or turned off by the person who made it."></picture> |

*From the web's mock server (`/__mock/config?me=admin`, and a link that is not one).*

- **Who may share** is `GET /api/me`'s `sharing`: the deployment's cap for a person whose roles hold `thread.share`, else `disabled`; an
  orchestrator that says nothing is read as `disabled` (`sharingOf`, `lib/sharing.ts`). The thread's overflow menu has **Share…** when it
  is not `disabled`, **and always for a thread that is shared**: taking a link down needs only ownership, so the owner can reach
  **Stop sharing** even where sharing was turned off since. Every thread in the list is the person's own (ADR 0039), so there is no "not
  yours" to check.
- **The dialog** (`share-dialog.tsx`, `use-share-thread.ts`) is a dialog: it takes the focus, Tab stays in it, Escape closes it and the focus
  goes back to the menu's button. Its radios are Private, Signed-in people with the link and Anyone with the link; each above the cap is
  disabled and says why in words (`This deployment shares only with signed-in people.`), and the public one carries its warning (*Anyone
  with this link can read this conversation, including what you pasted in it. Your e-mail is not shown.*). **A choice is picked, then
  saved**: the arrow keys select as they move, and walking past "Anyone with the link" must not make a thread public. Saving Private is
  `DELETE …/share` (the contract makes a `PUT` of `private` a 400); the others are `PUT …/share`, which keeps the link when it widens or
  narrows. With a share there is the link, in a field you can select and a **Copy** (an icon button named *Copy*, then *Copied*), **New link**
  (`POST …/share/rotate`: the old link is a 404 from then on) and **Stop sharing**, which keep their words because each takes the link that is out there away. A refusal (`over_cap`, `sharing_disabled`, a role
  without `thread.share`) is the server's own words under the choices, and changes nothing. The answer is the thread's new `share`, handed
  to the page as the thread it holds, so the chip, the menu and the sidebar follow without another fetch.
- **The chip and the mark** (`share-chip.tsx`): the icon of who can read the thread and, from `sm` up, the words "Shared · signed-in" or
  "Shared · public" in the top bar (people for signed-in, the world for public, on the warning colours, a link when paused: a shape of its own, never a
  colour alone; on a phone the icon, the words for a screen reader) and an icon after the thread's title in the list
  (`ThreadsView` items carry `share` without the link). They say what is **served now** (`effective`): a `public` share under a cap of
  `internal` is "Shared · signed-in", and a share the deployment has paused is "Sharing paused", with the reason in the dialog.
- **The page of a link, `/s/[token]`** (`src/app/s/[token]/page.tsx`, `shared-chat.tsx`). It is outside the sidebar and the composer: a top
  bar with the title, the state, the details panel and **Copy link**, the banner *Shared conversation, read only*, and the conversation.
  `<meta name="robots" content="noindex">`; the existing `Referrer-Policy: same-origin` keeps the token out of cross-origin referrers.
  `resolveShare` (`lib/resolve.ts`) reads the link, and which route it asks first is a **hint**, whether this browser has had a session
  (`lib/api/session-hint.ts`, `localStorage` `another-agentic.had-session`, set by any successful call of the app's own client and by a 200 of the
  signed-in route, forgotten by that route's 401; storage that cannot be read says no; in [browser mode](#signing-in-itself-browser-mode) a sign-in stored in IndexedDB counts too, read without opening anything, and a reader with none gets a 401 made in the page, not a request). A browser **that has had one** asks `GET /api/shared/{token}`
  first; a **401** (not signed in) is tried as `GET /api/public/shared/{token}`. A browser **that never has** (a visitor who followed a link) asks the
  **public route first**, so the edge's 401 of the signed-in route is never met just to be refused, and no token is sent either way
  ([ADR 0040](../docs/decisions/0040-thread-sharing-by-revocable-link.md)); a 404 there (a link that is not public, or a signed-in person whose browser
  forgot) is followed by the signed-in route, and a 401 *there* is the one place a visitor meets one. A wrong hint costs a request, never the answer, with one exception: a signed-in person whose browser has no hint (new, or its site data cleared) who opens a *public* link reads it as anybody (no file cards, an owner not sent to their thread) until the app has been used there once. If both say no, a **404** of the public
  route or a 401 of the signed-in one, the browser goes to `NEXT_PUBLIC_SIGN_IN_PATH?rd=/s/<token>` (the same opt-in and
  pause as [Signing in again](#signing-in-again): the reader's client never goes to sign-in on its first 401, because that 401 is a question, not an
  expired session), and with no sign-in path built in, or one just tried, the answer is the neutral page. **Every way a link fails is one
  page**, "This link does not work" (the 404 of the signed-in route, a 403, a token that cannot be one, a stream that answers 404 while the
  page is open because the owner took the link down or made a new one): it never says which, and never whether a thread exists. A
  signed-in reader who is the thread's owner is sent to `/threads/<id>`. A 429 is a line that says to try again, with a Retry.
  **Every message and turn is dated by when it happened**, not by when the page was opened: the stream says it (`metadata["vymalo.at"]` of a
  person's message and of an invocation's start, [`agui.md`](../docs/api/agui.md)), `ThreadAgent` gives the person's message that time as its
  `createdAt` and puts the invocation's in the actor marker part, and the turn's header reads it. The runtime's own `createdAt` is when a frame
  reached the page, which for a reload or a shared link is the same moment for every message.
- **Read-only is the thread's own components in a mode that cannot act**, not a copy of them: the same runtime, transcript, turns, cards,
  surfaces and step tree (`PanelProvider`, the details panel with Activity and Sources), and `ThreadAgent` told its `source` (a link's token
  and whether the reader is signed in), so its connect stream is `GET /agui/shared/{token}/connect` or `/agui/public/shared/{token}/connect`.
  There is no composer, sidebar, thread menu, agent picker, fork, edit, rename, export or Stop; the providers that make those (fork,
  branches, mentions, tool servers) are not mounted and their defaults are inert; the A2UI surfaces' buttons are off with the words *Read
  only: this is a shared conversation.* (`SurfaceHost.readOnly`); a question the agent asked is the owner's to answer, so it is not
  "Waiting for your reply". The page never asks `/api/agents`, `/api/config` or `/api/tool-servers`: a public reader has no identity for
  them, so an agent is its id and the description is the thread's own.
- **The owner's e-mail is never on the page**: the orchestrator's reader projection says "the owner" for the person and leaves the address out of
  the thread; the web adds nothing. A public reader has no step input, output or files unless the deployment turned them on, and the page shows
  what it is given (so an image in an answer that means a shared file is its placeholder there, [Images in the agent's words](#images-in-the-agents-words-that-mean-a-shared-file)). **Files** are read by the link's route: the stream's artifact `href` still names the owner's
  (`/api/threads/<id>/artifacts/<sha256>`), so `ThreadAgent` rewrites it, from the hash, to `/api/shared/{token}/artifacts/{sha256}` for a
  signed-in reader and `/api/public/shared/{token}/artifacts/{sha256}` for anybody (`FILE_HREF` in `vymalo.ts` accepts those three
  routes and no other).
- **A stream that is behind the head by events with no frame** (`thread_shared`, `thread_unshared`, `ui_catalog`: no frame, no resume
  point) is taken as caught up after `QUIET_MS` (2.5 s) of quiet (`use-chat-runtime.ts`). The thread's `lastSeq` counts those events and the
  stream never reaches them, so a thread that was shared and then left alone would never be "loaded" and a finished one would keep its
  stream (and, for a public reader, one of the link's stream permits) open for ever. A contract gap, recorded in ADR 0040's status note.
- **The mock** plays it all: `PUT`, `DELETE` and `POST …/share/rotate`, `GET /api/shared/{token}` and the public one, the files and the two
  connect streams through a reader projection (`readerEvent` in `mock/server.ts`: "the owner", no fork or catalog events, and for the
  public no step input or output or files), every failure the one 404, the stream ended when the link goes, and the share events in the log.
  A session's cap on sharing is `POST /__mock/config?sharing=disabled|internal|public` (default `internal`; `GET /api/me` says it as
  `sharing` for a profile that holds `thread.share`: `user`, `admin` and `limited`, not `read-only`); `?signedIn=false` makes every route but
  the public ones a 401; `POST /__mock/age?thread=<id>&seconds=<n>` moves the events a thread holds back in time (a test of the times a reader sees needs two messages that were not sent in the same second), `POST /__mock/share?thread=<id>&visibility=internal|public` shares a thread whatever the cap, as an earlier
  state of the deployment would have.

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
  <img src="e2e/__screens__/desktop-light-files.png" alt="An answer of the Reviewer, “I made three files: a chart, my notes and an export of everything”, under three cards: results.png with a bar chart of five green bars, notes.txt with its text shown in a box, and export.zip alone. Every card has a download icon button beside its name, its size and its type. The Activity panel on the right lists three steps, “Shared results.png”, “Shared notes.txt” and “Shared export.zip”." width="720">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-file-image.png">
  <img src="e2e/__screens__/desktop-light-file-image.png" alt="An answer whose interface places the chart in the words: “The results at a glance”, the bar chart with the caption “Figure 1: the results of the run”, and under the answer the card of the same file with its download icon button." width="720">
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

### Images in the agent's words that mean a shared file

*Added 2026-10-09, from the owner's thread of that day.* The coder took screenshots, shared each with `share_file` (an `artifact` event
with `file: {filename, sha256, size}`, beside the step "Share a file" whose `input` is `{path: "shots/4-matches.png", name:
"4-matches.png", repo}`) and then wrote an image whose source was that path (`shots/4-matches.png`) and whose alt text was "Matches list with percentages". That path means nothing to the browser, so
the pictures were broken, and the files only showed at the end of the answer. Now an image whose source is a path is looked up among the
files the thread holds (`lib/inline-images.ts`, pure; `hooks/use-inline-images.ts`; the `img` of `markdown-text.tsx`), and only there.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="e2e/__screens__/desktop-dark-inline-images.png">
  <img src="e2e/__screens__/desktop-light-inline-images.png" alt="An answer of the Reviewer that says it took screenshots. The list of people and the matches with their percentages are drawn in the words, each as a picture of the file shared. Where the login page should be is a small line with an image icon and the words “The login page”. Under the words one card is left, export.zip with its download icon button, because no image names it. The panel's Sources tab beside the answer lists all three files." width="720">
</picture>

*The mock's `inline-images` answer: two screenshots in the words, one path nobody shared as a placeholder, and the one file no image names as a card, with the panel's Sources tab open beside it.*

| Piece | What it does |
|---|---|
| Which sources are looked up | Only a **path**: a relative reference (`shots/4-matches.png`, `./4.png`, `/work/demo/4.png`), in one spelling (`normalisePath`): without its query and fragment, percent-escapes read (the renderer escapes spaces and non-ASCII in an image's source) and without a leading `./`. The step's path goes through the same function before the two are compared. A source with a scheme (`https:`, `data:`, `file:`, a drive letter) or another host (`//host/…`) is never looked up and never requested. An **http(s) image keeps the policy it had**: it is not drawn, it is its alt text and a link to follow. Only an **agent's** words mean a shared file; a person's own Markdown does not |
| Which file | `sharedFilesOf` lists every kept file of the thread in log order, each with the path of the step that shared it: a step with an `input.path` is paired with the artifact it made by the name it gave (`input.name`, else the base name of the path): in a first pass every artifact takes the nearest step before it that is not taken, and only then do the artifacts left over take a step after them (a step first reported when it ends), so a file whose step carries no path never takes the next share's. `resolveSharedImage` then takes, in the turn's own run first and then in the runs before it (never one after), **the file whose share step had exactly that path**, else **the file whose file name (or name) is the source's base name**. Where a name was shared more than once **the latest share wins**. Only an image (`preview: "image"`) can be the answer: a text file or an archive named by an image is a placeholder |
| Drawn | `FileImage` (the same component as the file card and `Image`), so the cookie mode has a plain `<img src=href>` and the browser mode (ADR 0054) a fetch with DPoP shown from an object URL (`lib/file-access.ts`); a public share link's files are plain links. The alt text is the agent's (a file name where it wrote none), and the picture sits in a block of its own in the paragraph |
| Not repeated | The cards after the words (`TurnCards`) leave out a file the words draw (`inlineFileHashes`: the images of the answer's Markdown, parsed so an image written in code is none, resolved the same way). A file no image names stays a card, and the panel's Sources tab lists every file either way |
| Not found | A small line: an image icon and the alt text (`data-slot="md-image-text"`, "image not shown" for a screen reader). Never a broken-image glyph, and never a request to the path |
| A public share link | The reader of a public link has no step input and no files unless the deployment turned them on ([Share a conversation](#share-a-conversation)), so every image there is the placeholder; a signed-in reader's files are read by the link's own route |

```mermaid
sequenceDiagram
  autonumber
  participant L as Log (vymalo.step, vymalo.artifact)
  participant S as sharedFilesOf
  participant M as Markdown img
  participant R as resolveSharedImage
  participant F as FileImage
  participant C as TurnCards
  L->>S: share step (input.path, input.name) and the artifact it made
  S->>S: pair them by name, one step per file
  M->>R: src from the agent's words
  R->>R: a path? else nothing is looked up
  R->>S: files up to this turn, the turn's own first
  R-->>M: the kept file (exact path, else base name, latest wins) or nothing
  M->>F: the file's own href, never src
  M->>M: nothing found: icon and alt text
  C->>R: the same images of the same words
  C->>C: leave out the files they resolve to
```

```mermaid
stateDiagram-v2
  [*] --> Source: ![alt](src) in an agent's answer
  Source --> Remote: http(s)
  Source --> Place: a path
  Source --> Text: any other scheme, or //host
  Remote --> Link: its alt text and a link, not drawn
  Place --> Shared: a shared image of this thread or an earlier turn
  Place --> Placeholder: none, or not an image
  Shared --> Picture: FileImage from the file's href
  Shared --> NotACard: the turn's list leaves the file out
  Text --> Placeholder
  Placeholder --> [*]: icon and alt text
```

`e2e/inline-images.spec.ts` plays the mock's `inline-images` (two screenshots and an archive shared, an answer that places the screenshots
by their paths and one path nobody shared): the pictures are decoded by the browser, the placeholder stands for the unknown path, only the
unnamed file is a card, the panel lists all three, nothing is requested from a path or from outside the app, and a reload is the same;
`e2e/share.spec.ts` has the signed-in and the public reader. The resolver is `lib/inline-images.test.ts`; the component,
`markdown-inline-images.dom.test.tsx`.

## Icons, and the words that stay

*Added 2026-10-09.* The owner: "avoid too much texts when a logo/icon can do the job". A control or a state whose meaning a common icon
says is drawn as the icon; its words are its **name** (`aria-label`, which tests and screen readers use, unchanged) and its **tooltip**
(`components/hint.tsx`, over the `ui/tooltip` primitive every icon of the chat uses). What a person has to read stays text.

| Control | Where | Before | Now |
|---|---|---|---|
| Thread state | top bar (`state-badge.tsx`) | an icon and its words ("Done", "Working…") | the icon alone, **a shape of its own per state** (clock for starting, spinner for working, shield with dots for checking the work, check, cross, stop for stopped); name "Thread state: Done", the same words as the tooltip. **Blocked keeps its words** ("Your turn", "Needs attention") |
| Share chip | top bar (`share-chip.tsx`) | an icon and "Shared · signed-in" (the words below `sm`: only for a screen reader) | **unchanged in its words** (who can read a conversation is not left to a tooltip a keyboard cannot reach); public is now on the warning colours |
| Read only chip | top bar (`read-only-notice.tsx`) | an eye and "Read only" | the eye; the line above the keyboard says it in full |
| Reconnecting | top bar (`thread-header.tsx`) | the text "Reconnecting…" | a pulsing wifi-off icon, "Reconnecting…" as its name and tooltip |
| Tools | composer (`tools-picker.tsx`) | a plug and "Tools" | the plug, name "Tools" |
| Download | file card (`kept-file-card.tsx`) | an icon and "Download" | the icon, name "Download results.png"; a download that failed is a round arrow in red, named "…(it failed, try again)" |
| Copy | share dialog (`share-dialog.tsx`) | an icon and "Copy" / "Copied" | the icon; names "Copy" and "Copied". **New link** keeps its words, like Stop sharing: it makes the old link stop working |
| Copy link | the page of a link (`shared-chat.tsx`) | an icon and "Copy link" (the words below `sm` hidden) | the icon, name "Copy link" |
| Retry, Dismiss | `InlineStatus` actions (`inline-status.tsx`) | link-styled words | a round-arrow and a cross, where the action says so (`Action.icon`); names "Retry" and "Dismiss" |
| Controls that were icons with a native `title`, or with none | Stop, Send, Delivery options (the split's chevron), Thread options, Thread details, Close details, Close and Open sidebar, New chat, Threads, Close (the sheet), Token usage, the remove button of a tool chip and of a mention chip | a `title` that appears after a delay and never for the keyboard, or nothing | the same tooltip, for a pointer and for the keyboard |

**What stays text**: errors and failures (the failed chip says "1 failed" and opens the step), empty states, explanations, form labels, the
radios of the share dialog and their warning, **New link** and **Stop sharing** (they take a link away), the share chip's words, the dialog's Done, Cancel and Save, the agent's name, the
panel's tabs, the menus' items (an icon and their words: a menu is where a person reads), "New chat" in the sidebar, and a call to action such as
"View pull request". A control that opens a menu or a popover keeps its tooltip shut while it is open (`Hint`'s `suppressed`).

- **The tooltip is a hint, not the name.** `Hint` (which `TooltipIconButton`, the copy, fork, edit and scroll buttons of a turn, is built on) puts words on one element (a `button`, an `a`, a `span` for a state that is no control); the
  element's name is its own `aria-label`, and the tooltip says the same words or more (a shortcut), so a screen reader loses nothing when it
  does not open. A disabled button has no tooltip (the browser sends it no pointer), so a disabled control that must say why keeps a native
  `title` (the split's Send while the conversation loads).
- **The keyboard.** The tooltip opens on focus that a keyboard would show a ring for (`:focus-visible`) and not on focus the page moves
  after a click (closing the sidebar hands the focus to the button that opens it again: no tooltip over a pointer that has left). The
  buttons keep the app's focus ring. **Escape dismisses the tooltip first** (WCAG 1.4.13: it can be dismissed without moving the focus), so in
  a dialog with a tooltip open the dialog closes with the next Escape.
- **No colour or motion alone.** Each state is a different shape, so one that is still (a person who asks for less motion) reads as
  it does in motion: a clock, a spinner, a shield with dots, a check, a cross, a stop, a warning, a question. The shield of "checking the work" has
  dots, because the shield with a check is the check that passed; the stop is the Stop button's, because a ban says "forbidden". A turn's line and its header in the panel (`steps/turn-glyph.tsx`) draw the states they share with the pill from the same map (`state-shapes.ts`), so the two never differ.
- Tests: `components/hint.dom.test.tsx`, `chat/components/state-badge.dom.test.tsx` and `e2e/icons.spec.ts` (the width of a pill, the tooltip for
  a pointer and the keyboard, no tooltip after a click's focus, none over an open menu); the existing specs find these controls by their names.


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
                               lib/inline-images.ts (an image in an agent's words that means a shared file: the
                               files with their share steps' paths, the resolver, pure) and hooks/use-inline-images.ts,
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
                               hook (the person's own threads, ADR 0039; a shared thread has the share's mark after its title); lib/sidebar-state.ts: the remembered open or closed sidebar and
                               the head script that hides a closed one before the first paint.
                               Paging goes by creation (`before=<id>`, UUIDv7) while the groups
                               go by the last change, so an old thread touched today can sit under
                               Today on a later page: known, and left as is
src/features/tools/            MCP servers attached to a conversation (ADR 0024): components/tools-picker.tsx (the Tools menu, the chips and
                               the warning for an agent that cannot use them), server-icon.tsx (a `data:` image or the generic
                               icon), tools-line.tsx (the `vymalo.tools` line), tool-servers-context.tsx (the list for the steps and
                               the line); hooks/use-tool-servers.ts (`GET /api/tool-servers`, live), hooks/use-thread-tools.ts
                               (`PUT /api/threads/{id}/tools` and the set the log says); lib/icon.ts (the one decision on what an
                               icon may be), lib/servers.ts (what is offered for an agent, the set now, pure), lib/line.ts
src/features/session/          the session that ends (Signing in again): components/session-banner.tsx (the line over the page and its Sign in button), signed-in.tsx (the last page of the popup, edge mode), auth-callback.tsx and sign-out.tsx (browser mode: `app/auth/callback`, `app/auth/sign-out`), hooks/use-keep-session-warm.ts; `app/signed-in` is the edge's popup route
src/lib/auth/                  the web's own tokens (Signing in itself): Dexie db, DPoP key and proofs, sign-in, refresh under a Web Lock, sign-out, `authenticatedFetch`
src/lib/csp.ts, scripts/csp-meta.ts   the content security policy's two halves: the static server's header, and each page's meta of script hashes (Static export)
src/lib/runtime-config.ts      `/config.json`, read once per page load: where the API is, the client this build signs in as (Static export)
src/lib/static-routes.ts, Caddyfile, scripts/serve-static.ts   the static server: the shells of `/threads/*` and `/s/*`, the headers (Static export)
src/features/routing/          the shells' client side: the id or token read from the address after the first render (`address.ts`), `thread-route.tsx`, `shared-route.tsx`
src/features/sharing/          sharing a thread by a link (ADR 0040): components/share-dialog.tsx (the dialog), share-chip.tsx (the
                               top bar's chip and the sidebar's mark), shared-chat.tsx (the page of a link: read-only), link-does-not-work.tsx;
                               hooks/use-share-thread.ts (`PUT`, `DELETE`, `…/rotate`), hooks/use-shared-thread.ts (reads a link);
                               lib/sharing.ts (pure: the cap, the choices and their reasons, the chip's words, the token's shape, the
                               file routes), lib/resolve.ts (signed in, then public, then sign-in: pure); `src/app/s/[token]/` is the route
src/features/me/               who the person is (ADR 0033): hooks/use-me.ts (`GET /api/me`, once per page load, `unknown` hides
                               nothing), lib/access.ts (pure: `threadAccess`, `newChatAccess`, `invokable`, `isAdmin`),
                               components/read-only-notice.tsx (the line and the chip), components/no-access.tsx
src/features/agents/           the new chat: greeting, suggestion chips; the agent picker (agent-menu.tsx, a menu
                               button in the top bar: the agents, then the chosen agent's releases; on a thread another
                               agent or release asks "Continue with … in a new chat?" and forks); agents hook; lib/selection.ts (which agent
                               and release a new chat sends to, `?agent=`); the agents hook reads `/api/agents` and
                               `/api/registry` together, on mount, when the picker opens and when the window gets the
                               focus back (at most every 5 s), keeping the list on screen while it refreshes;
                               hooks/use-agent-capabilities.ts reads the card's extensions live (`custom`) so the
                               composer can flag an agent that does not list `thread-tools/v1`;
                               registry-notice.tsx is the line "The agent registry is unreachable; showing the configured
                               agents only." (with Retry) under the greeting of a new chat and in the picker's menu
src/lib/                       api client (`api`, and `readerApi` for a shared page, which never meets the session refresh or the sign-in) and types (schema.d.ts is generated, never committed), api/session.ts (where the edge's sign-in and `userinfo` are, the popup, the last-resort redirect), api/session-refresh.ts (keep warm, refresh on a 401, hold a call while the person signs in), api/session-hint.ts (whether this browser has had a session: which route of a share link is asked first), markdown-refs.ts (the links and images of a Markdown text, one walker: the Sources tab's links and the images that mean a shared file), uuidv7
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
pnpm build         # the static export to out/, then each page's script hashes (Static export)
pnpm start         # out/ on :3000 with the image's headers and rewrites (scripts/serve-static.ts); `start:e2e` also forwards /api, /agui, /oauth2 to the mock
pnpm test:e2e      # Playwright + axe + Lighthouse (>= 95 accessibility) against the mock orchestrator
pnpm test:e2e:session  # Playwright on a build with the edge's sign-in built in: the session refresh and the sign-in popup (Signing in again)
pnpm test:e2e:browser  # Playwright on a build and a mock that is the issuer (MOCK_BROWSER_AUTH=1): the web's own tokens, DPoP, files as blobs, the policy (Signing in itself)
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
validates every response and every AG-UI frame against the contract and the vendored AG-UI schema. The mock plays `forkThread` and `listBranches` (and the list's `branches=include`), and its projection reads the fork goldens (`fork`, `fork-blocked`: the copied events, `thread_forked`, the fork's own life) and tells the golden stream (`mock/golden.test.ts`). Its projection also reads the `tools-attach` golden (MCP servers attached to a thread, [ADR 0024](../docs/decisions/0024-mcp-tools-attached-per-conversation.md): `thread.tools` in every snapshot and a `vymalo.tools` card, ids only) and the `tools-relay` golden (a relayed call of such a server's tool as one `vymalo.step` with `icon: mcp-server:<id>`, its input and output); the mock server has no route to attach one yet, which the composer's picker brings.
The mock also **stands in for the edge's own two routes** that the page's session refresh and sign-in use (`mock/server.ts`;
`next.config.ts` forwards `/oauth2/*` to it, for `MOCK_API_ORIGIN` only): `GET /oauth2/userinfo` answers 200 for a session and refreshes a
stale one, 401 (plain `Unauthorized`) for none, as oauth2-proxy does; `GET /oauth2/start?rd=<path>` signs the session in and redirects to
`rd` (a path of this origin, anything else is `/`). A test makes a session `stale` (`POST /__mock/config?stale=true`: every call but the
public ones is a 401 until `userinfo` refreshes it, as the edge's `forward_auth` answers) or not signed in (`signedIn=false`), and reads
what the stand-in did with `GET /__mock/edge?session=` (`refreshes`, `signIns`).

**Browser mode** is a switch, `MOCK_BROWSER_AUTH=1` (`pnpm mock`; `MOCK_PUBLIC_ORIGINS` lists the web's origins, 3000 to 3002 by default): the
mock is **also the issuer** (`mock/issuer.ts`, under `/oidc/`: discovery, an authorize that approves at once as the person the test chose,
a token endpoint with authorization code + PKCE and refresh with rotation and reuse detection, tokens bound to the proof's key by
`cnf.jkt`, revocation, end-session, CORS that does **not** expose `Date`), `GET /api/public/auth` names it (without the switch it is a
404), and every non-public API request must carry `Authorization: DPoP` and a proof that the mock verifies as the orchestrator will (`typ`,
ES256, the signature, `htm`, `htu`, `iat`, `jti` once, `ath`, `cnf.jkt`); a 401 carries `WWW-Authenticate: DPoP error="invalid_token"` or
`"invalid_dpop_proof"`. Test hooks, per session: `POST /__mock/issuer-config?lifetime=<s>&refreshDelay=<ms>&loginAs=<profile>`,
`POST /__mock/issuer-revoke` (an administrator's revocation), and `GET /__mock/issuer?session=` (`codeGrants`, `refreshGrants`, `reuses`,
`revocations`, `endSessions`, `apiRequests`, `publicWithCredentials`, `refused`). Who the person is comes from the token's profile. Two more switches let the desktop app run against the mock ([`apps/tauri/README.md`, "Tests"](../apps/tauri/README.md#tests)): `MOCK_LOOPBACK=1` takes
the loopback redirect `http://127.0.0.1:<any port>/callback` as Keycloak does, and `MOCK_CORS_ORIGINS` (comma-separated) answers the API's CORS for those origins as
the orchestrator's `server.cors` does (`mock/desktop-mode.test.ts`).
Without the switch the mock is the edge deployment above, unchanged.

The mock's default agent is **Adam** (`adam`, alias `coder`: [ADR 0049](../docs/decisions/0049-the-coder-is-shown-as-adam-agents-may-have-aliases.md)), like the stack's: `GET /api/agents` lists it under `adam` with `aliases: ["coder"]`, a run on `/agui/agents/coder` is a run of Adam and creates the thread under `adam`, and the page names an agent by its id *or* an alias (`useAgentNames`, `selectedAgent`, `requestedAgent` and the mention search), because a thread keeps the id it was created with. The goldens of `docs/api/examples` were recorded with a stand-in agent called `coder`, so `mock/golden.test.ts` reads the mock's `adam` as `coder` when it compares.

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
| `inline-images` | mock only: the coder's `share_file` as the owner's thread of 2026-10-09 had it: two screenshots (`3-list.png`, `4-matches.png`) and an archive shared, each by a step "Share a file" (input `{path, name, repo}`) and the artifact it made, then an answer that places the two screenshots by their paths (`shots/3-list.png`, `shots/4-matches.png`) and one path nobody shared (`shots/9-login.png`): the pictures in the words, a placeholder, one card |
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
| `think`, `think-gate`, `think-cut` | the `reasoning` golden ([`reasoning.events.json`](../docs/api/examples/reasoning.events.json), [`reasoning-live.feed.json`](../docs/api/examples/reasoning-live.feed.json), ADR 0044): what the model thought as four live reasoning pieces (`REASONING_*` with `vymalo.live`, no `id:`), then the log's `agent_reasoning` under the same id, then the reply `Fibonacci in Rust.` as in `stream` (`think`); the same held after two pieces until `POST /__mock/release?thread=<id>`, so a block stays open on a reasoning that is still being written (`think-gate`, the screens and `e2e/thinking.spec.ts`); a log whose reasoning was cut at its bound, with no live piece, and the reply `Done.` (`think-cut`). [Thinking](#thinking) |
| `coder-notes`, `coder-notes-legacy`, `coder-notes-hold`, `coder-notes-running` | mock only, working text and the answer (ADR 0031), the owner's coder chat of 2026-10-02 in its shape and in other words: six sentences said before tool calls, ten tool steps (a test run fails, one is `show`), a surface drawn on the way (two cards) and one answer, done (`coder-notes`; `Draw` is the same in plain words, for the screenshots); the same turn with no word marked, as an older log or a plain A2A agent says it, which the screen reads by its rule (`coder-notes-legacy`); the first five sentences and the steps between them, working until cancelled, so the line shows its ticker (`coder-notes-hold`, `Sketch` for the screenshots); one unmarked sentence and then nothing until the test releases the run, which shows as a draft of the answer, folds when a step starts after it, and ends with the words that are the answer (`coder-notes-running`). `e2e/answer-view.spec.ts`, `chat-shell-answer.dom.test.tsx` |
| `turn-output` | the `turn-output` golden (ADR 0031, the amendment): working, a sentence stated as an `agent_message` with `purpose: working`, a command `npm test`, then the answer the agent announced with the `turn_output` tool (`purpose: answer, via: turn_output`), the closing line as a `purpose: working` message (the core writes the words of a status that ends a turn that announced its answer as working text), the status that keeps it, and done. The projection says each in `vymalo.purpose` and `vymalo.via` on the `START`. The rule that **the answer of a turn is the last message marked `answer`** (a later `turn_output` replaces the earlier) is the screen's, not the mock's |
| `relay` | mock only: calls of attached MCP servers as the orchestrator will report them (`icon: "mcp-server:<id>"`, label `Web search · search`, input on the start, output on the end): a search of a server with an icon, a call of one the list has no icon for, a failed one, one of a server the deployment no longer lists; the answer and done. The orchestrator does not relay yet, so this is the mock's story (`e2e/tools.spec.ts`, the `tools-steps` screen) |
| `ask-agent`, `ask-hold` | mock only, an agent that asks agents (ADR 0026, `ask_agent`; the `ask-agent` golden's story with the mock's own agents): the thread's agent asks the Reviewer, which asks the Verifier (`ask_started`/`ask_finished`, a search step with path `ask-2` under that ask), both answer; then it asks the Verifier again and that one fails ("the verifier did not answer: connection refused"), the result, done (`ask-agent`). `ask-hold` is the same held while the two asks run until the test releases it (`POST /__mock/release?thread=<id>`) or the person stops it (the asks end canceled, "the asking task ended") |
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

**Who a session is** (ADR 0033): `GET /api/me` answers the session's profile, `POST /__mock/config?me=<profile>&session=<name>` switches it
(`user`, the default and what every session was before roles; `admin`, a user who also holds `admin`, which reaches no thread but their own, ADR 0039; `read-only`, who reads their own threads and does not write nor invoke; `limited`, who invokes the reviewer only;
`no-access`, whose roles grant nothing; `fixtures.ts`), and the mock enforces what it says as the orchestrator does: a thread belongs to the
session that ran into it (`Thread.owner`), the list is the caller's own and an `owner` parameter is a 400 for every role, another's thread is a
404 for every role (read, rename, cancel, fork or run), a permission no role holds is 403 `forbidden`, an agent the roles do not name is 403, and a person with no grants gets 403
`no_access` from every route but `/api/me`. `POST /__mock/owner?thread=<id>&owner=<e-mail>` hands a thread to someone, as a test says it.
`mock/server.contract.test.ts` checks each answer against the contract. A `/__mock/config` call changes only what it names.

**Sharing** (ADR 0040): the share routes, the reader's routes and streams, and the hooks `POST /__mock/config?sharing=<cap>&signedIn=<bool>` and
`POST /__mock/share?thread=<id>&visibility=<level>` are in [Share a conversation](#share-a-conversation); `mock/server.contract.test.ts`
checks every answer against the contract, the one 404 of a link that fails included.

**MCP servers** (ADR 0024): `GET /api/tool-servers` lists the deployment's servers, per session, in its order (`websearch` with an SVG icon for every agent, `github` with no icon for the coder only, `docs` with a PNG icon for the coder and the reviewer; `fixtures.ts`), and `POST /__mock/tool-servers?session=<name>` with an array as the body replaces the list, taken as it is (an icon that is a URL included, so a test can show the page never fetches one); both need `thread.write` (403 `forbidden`). `PUT /api/threads/{id}/tools` sets the whole set in any state of the thread: 200 `{servers}` (sorted; one `tools_attached` and one `tools_detached` for what differs, the same set nothing), 400 for a body that is not exactly `{servers: [ids]}` or an id that is not a server id, 403 `read_only` for an administrator on another's thread, 404, 422 for a server the deployment does not list or does not offer for the thread's agent (the detail names the id) or more than 16 (a server the thread has is not checked again). A run that creates a thread takes `forwardedProps["vymalo.tools"]` (400 before the stream when it is not an array of strings, 422 as above, nothing made), and attaches in the creation commit after the message; a fork keeps what its agent may use and detaches the rest first. The coder's capabilities list `thread-tools/v1` in `custom`, and the reviewer's and verifier's do not. `mock/server.contract.test.ts` checks every answer against the contract.

**A long thread** (what it costs to open one, [Opening a long thread](#opening-a-long-thread)): `POST /__mock/long-thread?turns=<n>` (1 to 2000, 200 when left out)
makes a finished thread of that many turns for the session's person at once, one minute apart: each is the person's message and what the coder did (a
job marker, two tool steps with their input and output, the branch and checks artifacts, a Markdown answer with a code block, a pull request card in every
fifth; `longThreadTurn` in `mock/scripts.ts`). The answer is `201 {threadId, turns, lastSeq}`; `mock/long-thread.test.ts` checks it against the stream.

Agents: `coder` (has `releases`) and `reviewer` (none).

## Tests

| Command | What | Count (2026-10-02) |
|---|---|---|
| `pnpm test` | vitest: token usage (`lib/usage.test.ts`, the fold of the `usage` golden, a replay equal to the live fold; `lib/agui/thread-agent.test.ts`, folded across a reconnect and never handed to the runtime; `mock/golden.test.ts`, the mock's projection of the `usage` golden), the agents hook (`use-agents.dom.test.tsx`, against the mock: the list and the registry's state read together, an agent the platform adds shown on the next read, a registry that is down flagged with the configured agents kept, an older orchestrator without `/api/registry` losing nothing, the list kept on screen while it refreshes and when that fails, a refresh on focus held back to every five seconds) and the registry notice in the picker (`agent-menu.dom.test.tsx`: nothing while every source answered, the line and a Retry item when one did not, on a new chat and on a thread, an agent's tags); the SSE reader; `ThreadAgent` (groups, reconnect with `Last-Event-ID`, dedupe by seq, acceptance at `RUN_STARTED`, problems, abort is not cancel); the AG-UI goldens through the patched runtime (messages, activities, interrupts, the cancelled outcome); the app in jsdom against the mock (new thread, replay, answer, refused send, cancel, not found, a surface and its action); **the A2UI validator and its security tests** (every bad URL scheme and trick, each limit at and one over, an expansion bomb and a reference bomb, the vocabulary, reserved names, function values, inputs), **the renderer** (the golden through the runtime, replace in place, delete, a refusal, a later run, no auto-send under fake timers, a click sends once, disabled states, `openUrl` as a link, `userMessage` in the composer unsent, no image ever) and **the app's side of an action** (what a click puts on the wire, with and without an open interrupt); **the verification gate** (the parsing of `vymalo.check`, `vymalo.rework` and `job`: malformed payloads draw nothing and unknown fields are ignored; the check and rework steps and the badge; findings that are markup or markdown shown as text, long ones cut and expandable; `verify-green` and `verify-red` through the runtime and through the whole `ChatShell` against the mock: the badge goes verifying, queued, working, verifying, done or failed, no attempt counter in the header, the check and rework steps in order ("Checks failed — trying again (2/3)"), "Checks failed after 3 attempts", a reconnect mid-verification and a fresh page showing the same); **CI results** (`parseCi`: every required member needed, unknown fields ignored, a url that is not http(s) dropped; the CI step for every conclusion with its words, icon and tone, an unknown conclusion, a missing url, a `javascript:` url handed straight to the step, markdown and HTML in the name, branch and summary shown as text, a long summary and name cut; the `ci` golden through the runtime and through the whole `ChatShell` against the mock, a step replaced by whatever id the wire gives it, malformed payloads drawing nothing); **the turn** (what is a step, a card or prose, `lib/steps.ts`; the pull request and file cards; the thread list's recency groups); **Export JSON** (the file name the server gives and never a path, the download through the API client, a refused export saying why and downloading nothing, the mock's export against the contract); **A thread's description** (`chat-shell-description.dom.test.tsx` against the mock: the model's description under the header and as the sidebar link's description, one that arrives after the thread is open drawn and cleared with no reload, plain text whatever it holds, Add description with the field focused and Enter saving through the API, an empty text clearing it, Escape and the same text sending nothing, a refused save keeping the field, Rename unchanged beside it, `ui.showDescriptions` off leaving no line, no description and no menu item; `thread-description.dom.test.tsx`: the Show more control only when the line cuts the text and following the width, `aria-expanded`, Markdown and markup as typed, the field's keys; `thread-sidebar.dom.test.tsx`: the link's description, the hover card by focus and by pointer, in plain text; `use-ui-config.dom.test.tsx`: read once, defaults, off until known, tried again after a failure; the mock's `PATCH` description, `GET /api/config`, the model's description not replacing a person's and the `description` golden through the server, in `mock/`); **Rename** (the title becomes a field that has the focus, Enter saves through the API and the header and the sidebar say it, Escape sends nothing, leaving the field saves once, the same or an empty title is no request, a refused rename says why and keeps the field; the mock's `PATCH` against the contract; a run that holds only a snapshot never reaches the runtime, in `ThreadAgent` and through the goldens); **the UI catalog** (the canonical JSON and the known-answer digest the orchestrator pins too, the lock against `catalog.json` and against the released versions, the catalog document's own rules, every send rule of `ThreadAgent`, the validator's catalog and schema rules, the placeholder for a newer catalog and the refusal when it is not newer); the mock against the contract and the goldens (`a2ui`, all four `verify-*` and `ci` included, the verifier's subagent among them) and its record of the UI catalog; **Choices** (the schema's limits at and one over, the two rules a schema cannot say, the reserved action name, the lowering through the converter, what is sent and in what order, `Other`, the gate on required questions, no send by drawing, choosing, typing, Enter or time, read-only when the thread does not wait or the copy is not the newest, a question id of `__proto__`; **the person's answers**: the shape of an answer, the labels resolved from the surface in the transcript and the raw ids and values without it, the bubble above the agent's mark and not a step, a button's action still a step, text only); **Cards and Mermaid** (both schemas' limits at and one over, a card's link by the schema and by `safeHttpUrl` with the bad-URL table, the validator's rules for both, the component for a newer thread; the cards drawn as text with their links and hosts and nothing fetched, a refusal that names the card; the graph drawn through a stand-in for mermaid: the strict configuration, the alt text, the source disclosure, loading, a parse error, a refused SVG, a broken image, a change of scheme, a later copy; **the pinned mermaid against hostile directives and front matter**, with a control that shows what mermaid's own `secure` list lets through; `svgImage` on scripts, handlers, loading CSS, entities and the viewBox; mermaid imported by one file only); **the brand** (`components/brand/`: the SVGs use only the three brand fills and no image, script or link, within 16 KB; the PNG and ICO sizes; the manifest's icons exist; an agent avatar's initial, stable tint, `aria-hidden` and 4.5:1 contrast in both schemes); **the agent picker** (`agent-menu.dom.test.tsx`: radio semantics, the check on the chosen agent, the releases as a group of the same menu, a refetch on every open, a failed refresh, the notice slot, the keyboard: Enter, Space, arrows, Escape returning the focus; on a thread another agent or release asking "Continue with … in a new chat?" before it forks, cancelled by Cancel, disabled with its reason while a turn runs; `lib/selection.test.ts`: which agent and release a new chat sends to, `?agent=`); **the step tree** (`lib/step-tree.test.ts`: the tree of a turn, numbered as the Sources tab numbers turns, nested by `path`, a step whose parent is not in the turn under the agent, a step said again updated in place (a retry too), an unknown kind or icon, a report that does not validate drawing nothing, today's activities as leaves with a command as a command step, the gate's nodes after the agent, a pending check spinning only while the run is open, the state of a turn (running, verifying, paused, failed, stopped, a turn that ended on a question staying paused), no running step in a turn that is not running, the turn's times, the summary's counts and the step it is on, `summaryLine` for every state, which children a level lists (the failed always), durations, and a turn rebuilt only when its message or the thread's state changed; `parseStep` in `vymalo.test.ts`; the `steps` and `steps-ask` goldens, and their connect variants, through the runtime and into a tree; `components/steps/turn-summary.dom.test.tsx`: the line, its name and chip for every state, the spinner only while running, a click opening the panel on the turn, `aria-expanded`; `steps-pane.dom.test.tsx`: collapsed by default, a click then "Show 10 more", the failed ones kept in view, the failure chip at every collapsed level, a level of 50 plain and one of 120 a window of rows, which turns are listed and open, following the live turn until the person chooses, a request to show a turn opening, scrolling to and focusing its header once per key; `steps-panel-content.dom.test.tsx`: the connected forms over the runtime playing the goldens; `chat-shell.dom.test.tsx`: a turn with nested steps is one line in the chat and no list, the line opens the panel on its turn, and each turn's line focuses its own; **a tool step's input and output**: `parseStep` reading `input`, `output` and `ioDropped`, a member of the wrong shape dropped and the step kept, `inputCut`; `lib/step-label.test.ts`: `server__tool` in words and any other label left alone, the argument a row quotes, sizes; `step-tree.test.ts`: the input kept across a step's reports, a failed `run_checks` counted once and a red artifact with no failed step counted, `pathTo` and `firstFailed`; `components/steps/step-io.dom.test.tsx`: the native button and its `aria-expanded` and `aria-controls`, Input then Output, a list or pretty JSON, an input that was cut, a cut output, Error in place of Output, the `ioDropped` note, no button for a step with nothing, markup in an argument, a result or an error shown as text and never run, a sub-agent opening its children and its block together, a request to show a step opening the way and focusing it; `expansion.test.ts`: opening the way to a step; the chat's failed chip in `turn-summary.dom.test.tsx`: a button beside the line's, calling `openSteps(turn, step)`); **live text** (`lib/agui/live-drafts.test.ts`: which frames are live, a draft opened by a `START` and grown by the pieces of the `stream` golden, an overlap, a repeat, a gap, an emoji's two UTF-16 units, an id that is not open, the bound, an abandoned `END`; the log's message taking a draft over: the plain message the runtime reads, made of the draft up to `offset` and the rest, a final that replaced the text, `offset: 0` with no draft, a final that continues text the connection never held, the other frames of the group in their place; what a completed draft draws until the transcript has its words. `ThreadAgent`: a piece shown at once with no `id:` to wait for and never in the run or a resume point, the log's message completing the draft and the next group dropping it, the runtime reading the same events with or without live text, a group it cannot tell whole not delivered and the connection reopened at the last resume point without an error, a cut connection forgetting its drafts, a given-up reply, an abandoned `END` inside the group that closes the invocation, the end of a run clearing the drafts, a live frame that carries an `id:` anyway. The `stream` golden through the runtime: nothing of the reply in the transcript while it is a draft, one reply after it, no draft left. `components/chat-shell-live.dom.test.tsx`: the whole app against the mock, a draft that grows as Markdown, `aria-busy`, the one reply that replaces it, Stop, a given-up stream, a connection cut mid-reply, a page opened mid-reply told the text so far by the sender's refresh, a thread opened after the reply reading it plainly. `mock/live.test.ts`: the mock's overlay on the rules of the real one, and `mock/golden.test.ts` the `stream` golden); **the panel** (`features/panel/`: `lib/sources.test.ts`: every kind of source, only http(s) links, a URL in code never read, one item per URL with every turn that cited it, a typed source over the same link in words, the groups and their order, numbering the turns as the chat does; `lib/panel-state.test.ts`: where the panel docks and the widths it may have, what is remembered, and the head script run against the same rules, storage blocked; `hooks/use-panel.dom.test.tsx`: the defaults by window width, the shortcut on every combination of keys, the remembered choice, a sheet that is for the visit only, the `useStepsPanel()` contract of the step tree; `components/thread-panel.dom.test.tsx`: the landmark, the tabs and their arrows, the separator's keyboard and pointer, the focus on close, the sheet from the right and the bottom, a Turn button; `sources-tab.dom.test.tsx`); **forks** (`ThreadAgent`: the event each person's message came in and the last event of each run, across jobs, a cut connection and a fork's own run; the actor marker carrying its `runId`; the fork goldens, and a blocked thread's, through the runtime: the marker one message of its own and the copied question closed; `chat-shell-fork.dom.test.tsx`: Fork from here making a thread that ends where the turn does and going to it, disabled while a turn runs and in the agent menu with its reason, enabled once stopped, an earlier turn forked while a later one runs, `turn_open` and another refusal in words with Dismiss, the divider linking to the parent and saying "continued with" another agent, a fork of a thread that waited for an answer taking an ordinary message, the fork marked in the list and an edit not listed; **branches** (`message-editor.dom.test.tsx`: the editor opens with the caret at the end, Escape cancels, Ctrl or ⌘ with Enter sends, a plain Enter and a composing Enter send nothing, an empty message is not sent, Send waits while the fork is made; `branch-picker.dom.test.tsx`: the count from 1, the first ‹ and the last › disabled and never wrapping, the live region in words; `chat-shell-branches.dom.test.tsx`: the pencil, the editor in the bubble and Escape, Ctrl+Enter making an edit and going to `#m-<seq>`, a refusal as "Could not edit the message" with the words kept, 2/2 and 1/2 with the arrows going to the other thread, no picker on a thread never edited or one whose branches cannot be read, the edit left out of the list and the conversation's first thread highlighted); `mock/server.contract.test.ts`: the mock's `forkThread` and `listBranches` against the contract, the cut, the target, the repeat by id, `turn_open`, every documented problem, the list flag); **the answer and the working text** (`lib/working.test.ts`: the mark wins; unmarked text in a turn that is over, the last is the answer and a marked answer wins over a later unmarked text; in a turn that runs an unmarked text is a draft of the answer until a step starts after it, a status that says what the agent does folds it, an artifact does not; the ticker line plain, cut and empty for words that say nothing. `lib/step-tree.test.ts`: notes in time order among the steps, not counted, not the step a turn is on, "3 notes" for a turn with none, the ticker, the unmarked rule. `lib/agui/live-drafts.test.ts`: the purpose an `END` says, the plain message's `START` saying it, an `END` that says only `final` leaving the draft the answer, a working draft drawing nothing. `runtime-goldens.dom.test.tsx`: the `working` golden through the runtime with the marker before each text, and the `stream` golden ended as working. `components/steps/working-notes.dom.test.tsx`: the note rows (a screen reader's "Working note:", whole in the page, clamped with Show more, text and never markup, a turn of notes listed, a turn of its answer only not) and the ticker (the last line, muted, one line, not a live region, not a control, gone when the turn is over). `components/chat-shell-answer.dom.test.tsx`: the whole app against the mock on the owner's chat, marked and unmarked: one answer in the column and none of the six sentences, the surface kept, the notes in Activity in order, Copy copying the answer, a question as the answer, a plain agent's one message, an unmarked sentence a draft until a step starts and then a note, the ticker while a turn is held. `chat-shell-live.dom.test.tsx`: a live draft that ends as working text leaving the column; roles, ADR 0033: `use-me.dom.test.tsx` (`GET /api/me` read once, an identity that cannot be read is unknown), `access.test.ts` (read-only by owner, by permission and by agent, the picker's agents, the administrator), `session.test.ts` and `thread-agent-session.dom.test.ts` (a 401 goes to the sign-in with `rd`, once per pause, never without the variable or for a path of another origin), `chat-shell-roles.dom.test.tsx` (the whole app against the mock's profiles: a read-only thread has a line and no box, no fork, no edit, no rename, a card's actions off; the picker lists the invokable agents; the administrator's Mine / All threads with owners, kept, ignored for anyone else; the no-access screen; the other 403s in the server's words, and an orchestrator that cannot say shows everything), `tools-picker.dom.test.tsx`, `tools-hooks.dom.test.tsx`, `use-agent-capabilities.dom.test.tsx`, `step-server-icon.dom.test.tsx` and `chat-shell-tools.dom.test.tsx` (MCP servers, ADR 0024: the picker's checkbox items, chips and sixteen at most, the flag for an agent whose card does not list `thread-tools/v1`, the list read live and a 403 or 404 of it as no picker, the whole set with one `PUT` and the orchestrator's words when it is refused, the icon drawn from a `data:` URI and never from a URL, the step's icon slot, a new chat's `vymalo.tools`, no request for the list without `thread.write`), `lib/icon.test.ts` (every way an icon is refused), `lib/servers.test.ts` (what is offered for an agent, the set the log and the answer of a `PUT` say), the `tools-attach` golden through the runtime (three cards, none replacing another), and the mock's contract tests for `GET /api/tool-servers` and `PUT /api/threads/{id}/tools` (400, 403, 404, 422); sharing (`sharing.dom.test.tsx` against the mock: the menu, the dialog, the page of a link read signed in, as anybody, as the owner and for a link that does not work; `lib/sharing.test.ts`, `lib/resolve.test.ts`, `thread-agent-shared.test.ts`; the mock's share routes in `server.contract.test.ts`) | 1433 (83 files) |
| `pnpm test:e2e` | Playwright on the production build against the mock: the token ring (`usage.spec.ts`, desktop and phone, see [Token usage](#token-usage)), MCP servers (`tools.spec.ts`, desktop and phone, each test with a deployment and a person of its own in the mock: a new chat picks Web search and Team docs with the menu staying open, takes one off, sends, and the run that creates the thread carries `vymalo.tools` and a follow-up does not, the line "Web search attached" in the conversation and the chip on the thread; on an existing thread attach, detach, the same after a reload, the lines in the order they happened, and while the agent works; an agent whose card does not list `thread-tools/v1` flagged before anything is sent, the choice kept, and gone for the coder; a server the agent may not use not offered and no picker when nothing is; the icon drawn from a `data:` URI and from nothing else, with an http(s), a protocol-relative and another kind of `data:` icon in the list and every request of the page watched, none made, on the picker, the chips and the steps of the mock's `relay`; the read-only view with no picker and no request for the list; the keyboard alone from the box to Send; a refused change in the orchestrator's words; axe with the menu open, with chips, with the flag and on a thread with a line and server icons, both schemes); roles (`roles.spec.ts`, desktop and phone: an administrator lists everyone's threads with their owners and reads another's with no box, "Read only: this is dev@example.com’s thread." as a status and a chip, rename disabled, no fork or edit; the choice kept; the picker of a role that invokes one agent; a role that cannot start a chat; the no-access screen; axe on a read-only thread, the administrator's list and the no-access screen in both schemes); the files an agent made (`files.spec.ts`, desktop and phone: the cards of an image, a text file and an archive with their names, sizes and downloads, the picture decoded and the text drawn as text, the download saving the file under its name, the Sources panel's Files, an SVG written to run a script drawn as a picture that does and asks for nothing, the catalog's `Image` placing the file in the answer, a hash the thread does not hold refused, a file that was not kept with its error and no download); the answer and the working text (`answer-view.spec.ts`, desktop and phone, on the mock's `coder-notes` scenarios: one answer in the column and none of the six sentences said on the way, the surface kept, the sentences as notes in Activity in the order they were said, the same for a log with no word marked, one answer for every turn of a conversation, nothing focusable behind anything hidden in the column, the ticker of a running turn quiet and one line and never a live region, the line and its failed chip not squeezed by it, a streamed sentence filed as a note; axe on the finished turn with its notes and on the running turn with its ticker, both schemes, reduced motion); the agent registry (`registry.spec.ts`, desktop and phone: a registry that cannot be read is said under the greeting and in the menu with the configured agents still there, and the line goes on Retry or when the picker opens after it came back; an agent the platform adds is in the picker the next time it opens, without a reload, with its tags, after the configured ones; a message goes to it; axe with the notice, light and dark; each test has a registry of its own in the mock, by cookie); axe (no serious or critical issue, light and dark; new, finished and blocked thread, and a thread with an A2UI surface waiting, finished and refused) and Lighthouse accessibility >= 95 in both schemes; create, follow-up, cancel, failure, releases, API errors; reconnect (a dropped stream, a cut in the middle of a message); two tabs; A2UI (a surface is drawn and only a click sends the action, read-only after the thread finishes and after a reload, a refusal, no remote content); the UI catalog (the first run of a thread carries it whole and a later one does not, the placeholder of a thread opened by a newer app, kept after a reload and never answered with a catalog, the refusal when the thread is not newer; axe on the placeholder, both schemes); Choices (three questions answered by clicking: the button waits, one action goes out with no message and no `resume`, the bubble with the labels, no step, the agent's echo, the Choices read-only and the same after a reload; "Other" as the person's own words; the keyboard alone, arrows and space, Enter in a text box sending nothing; on a phone too; axe unanswered, answered and after sending, both schemes); Cards and Mermaid with the real mermaid in Chromium (one turn of words, three cards and the picture of the graph, links in a new tab with their hosts, the source one click away, nothing requested outside the app, the same after a reload; mermaid loaded when a graph is drawn and not with every thread; the graph in the colours of the light page and of the dark one; a card that breaks the schema, and a link that breaks only ADR 0013's rule, each refused with its rule and the raw operations; a graph that does not parse beside cards that are drawn; a graph that tries to switch its security off, drawn as a plain image with no script run, no link and no request; ten kinds of graph all drawn; on a phone too; axe on the answer with the source open, on the graph error and on the refusal, both schemes); verification (sent back and done on attempt 2 of 3, `Checks failed after 3 attempts`, a pending check while verifying that survives a reload and ends with Cancel, a verifier agent's findings and pass, a pending check of the verifier that ends with Cancel, no counter without a gate; axe on a thread being verified, on one waiting for its verifier, on a stale check and on failed checks, both schemes); CI results (a red report and a green one as steps with their links, a late report of an older push, no card while CI has not answered; axe on the CI cards, both schemes); Export JSON, from the thread's menu (the thread downloads as `thread-<id>.json` with its events, and a failed export is said and leaves the page usable); nested steps (a delegation to OpenCode is one line in the chat, with its failure chip and its name for a screen reader, and the tree in the panel: depth 1 listed, the sub-agent one collapsed line with its count and its failure chip, a click then the latest three and the failed one, Show 10 more twice, closing it, the same tree after a reload; each turn's line focusing its own turn and again on a second ask; a level of 120 steps a scroll box that draws a window of the rows in Chromium; a running turn's line naming the step and spinning until Stop; axe on the tree open, on the scroll box and on a running turn, both schemes, on a desktop and on a phone's sheet); opening a tool step (`step-io.spec.ts`, the mock's `steps-io`, on a desktop and a phone: a tool named by its tool, its server and its query; a click, Enter and Space opening Input then Output with `aria-expanded` and closing it; the same block after a reload; a cut output saying "n KiB more not kept" and an input too big to keep saying so; a failed step showing Error, a step the record budget missed saying so, a step with nothing not a button; the chat's failed chip opening the panel on the failed step, inside a sub-agent too, with the focus on its button; axe with every step open, both schemes); the turn (a coder's steps in the panel, one line in the chat, a command, the push, the checks, the pull request card and its link, the markdown answer said once; the live step and Stop; "is starting…" before the first event; a rework step and the folded findings; the question waiting for a reply; a suggestion that fills the box and sends nothing; the sidebar closed, remembered and opened); the brand (the icons, manifest and head tags served, the title, the panda in the sidebar and over the greeting, an agent's letter and not the panda in a turn); the agent picker (the menu button in the top bar, the agents with their descriptions and the check, the keyboard, a click outside, the release group, the choice the first message goes to, on a thread the other agents as a new chat with `?agent=`, every control of the top bar on the screen on a phone; axe with the menu open, both schemes); live text (the words grow while the agent writes them, as Markdown, then the reply stays, once, with no draft left; Stop ends a draft; a stream the model gave up; a connection cut and a page reloaded mid-reply; the caret blinks, and stays on, still, under reduced motion; axe with a draft on the screen, both schemes, on a desktop and on a phone; Lighthouse >= 95 with a draft on the page, both schemes, with a desktop and a phone form factor); the panel (open by itself on a wide window and remembered, the shortcut from the message box, the tabs and their arrows, Sources listing the pull request, the branch, the run and the links in the agent's words once each and never a URL in code, a Turn button scrolling to and focusing the turn, the resize separator by keyboard and pointer, the sheet from the right and from the bottom with Escape and the focus back; axe with the panel open, both tabs and the empty one, both schemes); forks (`fork.spec.ts`, desktop and phone: Fork from here makes a new chat that ends where the turn does, with the divider, the parent untouched and its link back, the fork's marked row in the list; an earlier turn forks to its own end; the button disabled with its reason while a turn runs (a turn the mock holds until Stop) and enabled once stopped; the keyboard; another agent or another release in the menu asks first and Cancel changes nothing; a thread that waited for an answer forked and replied to; a `turn_open` refusal shown as a message; axe on the turn's actions, the question and the fork, both schemes); descriptions (`description.spec.ts`, desktop and phone: the model's description arriving on the open thread with no reload, one muted line that opens to the whole text and closes again with the keyboard, the description in the export, the sidebar row's hover card by pointer and by keyboard and the link's description, writing it from the menu with the keyboard and an empty text clearing it, a refused save keeping the field, `ui.showDescriptions` off leaving it nowhere, plain text whatever it holds; axe on the line closed and open, the field and the card, both schemes; each test that switches `ui` has a session of its own in the mock, by cookie); branches (`branches.spec.ts`, desktop and phone: edit the second message and the new chat shows 2/2 with the new answer and the focus on the message, the list showing the conversation once; ‹ goes back to 1/2 and the original, › to the edit; edit the first message; the keyboard alone (Escape gives the words back and the focus to the pencil, a plain Enter is a new line, Ctrl+Enter sends, Enter on an arrow); an empty message not sent and a refused edit saying so with the words kept; axe on the pencil, the editor and the picker, both schemes); sharing (`share.spec.ts`, desktop and phone: the dialog and its keyboard, a choice that is picked and then saved, the cap, the chip and the mark, a new link and Stop sharing, a role without `thread.share` that can still take a link down, the page of a link for a signed-in reader and for anybody with no composer, menu, fork or e-mail and with its files by the link's route, the neutral page for every failing link, a stream let go that has no frame to reach the head, axe in both schemes) | 377 tests, 2026-10-02: 366 pass, 10 are skipped (the panel's desktop tests have no phone and its phone test no desktop, one keyboard test, and the description's hover card on a phone) and the axe test of the description's hover card (light, on a desktop) fails, on the base commit too (the card is not there when the hover lands; 4 of 4 runs here, one of them on the base commit); the Lighthouse test with a draft on the page needs a quiet machine and fails on a loaded one, with or without descriptions |
| `pnpm test:e2e:session` | Playwright on its own build, with the edge's sign-in built in (`playwright.session.config.ts`, port 3001, `.next-session`; the build of `pnpm test:e2e` has none, and several of its specs say what a page does then): `e2e/session-refresh.spec.ts`, the page's [session refresh and sign-in popup](#signing-in-again) against the mock's stand-in for the edge: a call refused with a 401 is refreshed and sent again, an interval (Playwright's clock) keeps the session warm so that no call is refused, a run sent on a stale session is refused once and sent again, a session that ended shows the banner and its popup signs in and closes itself, a refused popup leaves for the sign-in and comes back, and a public reader of a shared page is never asked anything. Each keeps a draft in the box and a mark on `window` | 6 tests |
| `pnpm test:e2e:system` | the same UI against the real orchestrator, see below (the token ring drawn from the real projection of the fake agent's `usage` script, `usage.spec.ts`; Export JSON included: the downloaded file is the log the connect stream replays; forks: the fork's agent is told the conversation in a context of its own, `fork.spec.ts`; an edited message likewise, with the versions as siblings, `branches.spec.ts`) | 26 |

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
| 3100 | the app (`pnpm build:system`, then `scripts/serve-static.ts`, which forwards `/api` and `/agui` to the orchestrator) |
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

`Dockerfile`: the static export (`pnpm build`), served by **Caddy 2.11.4** (pinned by tag and digest, the edge's) with [`Caddyfile`](Caddyfile):
no Node at run time, `USER 1000`, port 3000, nothing written but Caddy's directories under `/tmp`. The build context is the repository root:

```sh
docker build -f web/Dockerfile -t web .
sh web/tests/image-smoke.sh web    # as the chart runs it: read-only, unprivileged, the shells, the headers, the policy
```

`NEXT_PUBLIC_SIGN_IN_PATH` is a build argument (`ARG`, empty by default): the edge's sign-in path, which the page's session refresh and
sign-in start from ([Signing in again](#signing-in-again)). Next inlines it at build time, so the image is built per deployment that wants it.
`WEB_CSP_CONNECT_SRC` is a **runtime** variable, not a build argument: the origins (space-separated) the page may connect to and
submit to besides itself, which in browser mode is the issuer's ([Signing in itself](#signing-in-itself-browser-mode)); empty by default. Caddy reads it
when it starts and writes it into the policy header **as it is**: it is trusted operator input, never a person's (the chart derives it from `auth.issuer`
and refuses anything but one `https://host[:port]` origin, `deploy/chart/templates/_validate.tpl`; `scripts/serve-static.ts` drops what is not an origin). `NEXT_PUBLIC_BUILD_REVISION` is another (the workflow passes the commit sha; empty by default): the build the web sends with an export,
as `X-Web-Revision` ([ADR 0053](../docs/decisions/0053-a-thread-export-says-which-builds-made-it.md)). The caddy binary carries the file capability
`cap_net_bind_service`: a pod that drops every capability must keep `NET_BIND_SERVICE` for it to start (the chart does, as for the edge).

The install stage copies `patches/` next to the lockfile: `patchedDependencies` must be on disk when pnpm
resolves the install.

### Static export

*ADR 0047, decision 1, built 2026-10-09 (amended that day: the policy's hashes, the shells, `/config.json`).* `next build` writes every page as
HTML to `out/` (`next.config.ts`: `output: "export"`; `next dev` keeps the rewrites to the mock). There is no server of ours, so:

| What a server did | What does it now |
|---|---|
| `/threads/[id]`, `/s/[token]` rendered per id | one page each, `/threads/_` and `/s/_` (`generateStaticParams`), which the static server answers for any `/threads/<id>` and `/s/<token>`, and for the data Next fetches for them when the app moves there (`<id>.txt`, `<id>/__next.*.txt`): moving between threads never loads the page again (`e2e/static-export.spec.ts`). The page reads the id from the address after its first render (`features/routing`), so the first render is the exported one |
| the headers (`nosniff`, `Referrer-Policy: same-origin`) | the static server ([`Caddyfile`](Caddyfile); `scripts/serve-static.ts` for the e2e and the system tests, held to the same routes and headers by `src/lib/static-server.test.ts`) |
| the policy with a nonce per request (`src/proxy.ts`) | two halves a browser enforces together: the static server's **header** (everything but the hashes, `script-src 'self' 'unsafe-inline'`, the issuer from `WEB_CSP_CONNECT_SRC`) and, first in every page's `<head>`, a **meta** that `scripts/csp-meta.ts` writes after the build: `script-src 'self'` and the SHA-256 of each inline script of that page. A hash voids `'unsafe-inline'` in its policy, and a script must pass both: only the build's own inline scripts run. **Weaker than the nonce:** with no `'strict-dynamic'`, `'self'` runs any response of this origin that has a JavaScript type, so the API never sends a file as one (non-preview types go as `application/octet-stream`, and a file fetched as a script, a worker, a style or an object is a 403: ADR 0054, *Correction (2026-10-09)*). The build fails on a page without `<head>` or with a policy already. `e2e/csp.spec.ts` recomputes the hashes of every page and counts zero violations while the app is used |
| one image per deployment for the API's address | `/config.json`, read once per page load (`src/lib/runtime-config.ts`): `apiOrigin` (where the API is, for the desktop app), `clientId` (the public client this build signs in as), `signIn` (`redirect` or `loopback`), `organisation` (the sign-in screen's line). The image ships `{}`: the API on the page's own origin, as before. Every request to `/api` and `/agui` goes through `atApi`, which sends it to `apiOrigin` when that is another origin; the orchestrator then needs that page's origin in `server.cors.allowedOrigins` |

CI builds it on every change and pushes `ghcr.io/vymalo/another-agentic-system/web:sha-<7>` and
`:latest` from `main` (`.github/workflows/web.yml`).
