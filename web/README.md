# web — chat surface

Next.js (App Router, TypeScript strict), [shadcn/ui](https://ui.shadcn.com/) on Tailwind v4 and
[assistant-ui](https://www.assistant-ui.com/) on **AG-UI**: the conversation is the orchestrator's
[AG-UI 1.0](../docs/api/agui.md) projection of the event log, rendered by
`@assistant-ui/react-ag-ui`. It lets the owner pick an agent (and a release, when the agent offers
one), start threads, follow up, answer an interrupt, cancel and watch progress live, and it keeps
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

The four legacy interaction operations (`createThread`, `postMessage`, `listEvents`,
`streamEvents`) are deprecated and the web does not call them.

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
  endpoint. The runtime drops an event's `metadata`, so the agent moves `vymalo.actor` into the
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
- **Thread state** (the header badge, whether the composer shows Cancel or Send) is the newest
  `STATE_SNAPSHOT.thread` the agent delivered, else `GET /api/threads/{id}`.
- **Interrupts.** The run that ended in an interrupt leaves the runtime holding it
  (`useAgUiInterrupts`); the composer shows its `message` as the question and sends the answer as
  `resume` (`steerAway` with a `resolved` entry carrying `payload.text`). Never a plain message: the
  orchestrator refuses a message together with a `resume`.
- **A send the server refuses** (a problem before the stream) is a `SendError`, assistant-ui's
  `MessageNotSentError`: the composer takes its text back, the failed message is removed from the
  transcript (`dropFailedSend`) and the problem's `detail` is shown.
- **Renderers** are `agui-activity/vymalo.status`, `.artifact` and `.error` (`data-uis.tsx`, reusing
  the status line, artifact card and error line), and a `vymalo.action` renderer that draws nothing
  until A2UI surfaces are rendered. A shape a renderer does not know renders nothing.
- **Thread ids** are UUIDv7 (`src/lib/uuid.ts`): the consumer mints them, and the orchestrator lists
  threads by id, newest first.

### Dependencies and patches

Pinned exactly. *Verified 2026-09-29* on the npm registry (`npm view`):

| Package | Version | Note |
|---|---|---|
| `@assistant-ui/react-ag-ui` | 0.0.62 (2026-09-24, latest) | depends on `@ag-ui/client` `^0.0.59`, `@assistant-ui/core` `^0.3.21`, `@assistant-ui/react-generative-ui` `^0.0.21` |
| `@assistant-ui/react-generative-ui` | 0.0.21 (2026-09-24, latest) | peer of the runtime (it renders A2UI surfaces, ADR 0013: not rendered yet) |
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

## Layout

Every file name is kebab-case (`pnpm check` fails otherwise). Tests sit next to the code they test.

```
src/app/                       routes only: thin pages that compose features
src/components/ui/             shadcn primitives (generated by the shadcn CLI, then formatted)
src/components/assistant-ui/   assistant-ui registry items, pruned to what a run's parts need
src/components/inline-status.tsx   shared empty, loading and error lines
src/features/chat/             the conversation: components (shell, composer, renderers, LiveRuns),
                               hooks (runtime, thread details), lib/agui (ThreadAgent, SSE reader,
                               live runs, the vymalo vocabulary)
src/features/threads/          thread list: sidebar (desktop) and sheet (phone), paging hook
src/features/agents/           new-thread panel: agent and release pickers, agents hook
src/lib/                       api client and types (schema.d.ts is generated, never committed), uuidv7
patches/                       pnpm patches of dependencies, and the drafts of their upstream twins
e2e/, e2e-system/, mock/       Playwright suites (mock / real orchestrator) and the mock orchestrator
```

The theme is `src/app/globals.css`: the design tokens (light, and dark under
`prefers-color-scheme`) mapped onto shadcn's names. There is no theme toggle. To add a primitive
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
```

Playwright uses the browser Playwright pins (`@playwright/test` is pinned exactly; CI runs
`playwright install --with-deps chromium`).

## Mock server

`mock/server.ts` is a small stateful mock of the orchestrator: the resource API (agents, threads,
cancel) and the AG-UI routes (run, connect with `Last-Event-ID` and `?mode=run`, capabilities). It
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
| `partial` | mock only, **not produced by the current orchestrator**: a partial agent message replaced by its final version |
| `unreachable` | mock only: an error activity, `RUN_ERROR` `delivery_failed`, thread blocked |

The mock tells the orchestrator's story: `mock/golden.test.ts` drives every scenario of
[`docs/api/examples`](../docs/api/examples/README.md) through the mock's run route and requires the
connect stream a viewer reads to be the golden `agui/<name>.agui.json`, frame for frame, so the mock
cannot drift from the orchestrator unnoticed. Test hooks for the e2e suite: `POST
/__mock/drop-streams` (cut every open stream), `POST /__mock/cut-next-connect?frames=n` (cut the
next connect stream after `n` frames, in the middle of a group) and `POST /__mock/reset`.

Agents: `coder` (has `releases`) and `reviewer` (none).

## Tests

| Command | What | Count (2026-09-29) |
|---|---|---|
| `pnpm test` | vitest: the SSE reader; `ThreadAgent` (groups, reconnect with `Last-Event-ID`, dedupe by seq, acceptance at `RUN_STARTED`, problems, abort is not cancel); the AG-UI goldens through the patched runtime (messages, activities, interrupts, the cancelled outcome); the app in jsdom against the mock (new thread, replay, answer, refused send, cancel, not found); the mock against the contract and the goldens | 83 (11 files) |
| `pnpm test:e2e` | Playwright on the production build against the mock: axe (no serious or critical issue, light and dark; new, finished and blocked thread) and Lighthouse accessibility >= 95 in both schemes; create, follow-up, cancel, failure, releases, API errors; reconnect (a dropped stream, a cut in the middle of a message); two tabs | 34 pass and 1 is skipped on desktop (28 chromium, 6 mobile) |
| `pnpm test:e2e:system` | the same UI against the real orchestrator, see below | 15 |

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
| 4021, 4022 | the `coder` (with release channels) and `plain` fake agents |
| 3101 | `reconnect.spec.ts` only: a TCP forwarder in front of the app that cuts the connect stream |

`e2e-system/*.spec.ts` cover the identity through the rewrite (and 401, user isolation, and another
user running into a thread id), the create/echo lifecycle (and the log behind it, read as the
connect stream), agent text, ask and answer on the same A2A task (a `resume`), cancel reaching the
agent, the failure shape, the 409 on a finished thread, releases, a dropped stream, a SIGKILLed
orchestrator, history by URL (the connect stream closes on a finished thread) and paging of the
thread list. Every test starts on an empty database; the orchestrator log of a run is
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
