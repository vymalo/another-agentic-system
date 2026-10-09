# Upstream twins of the patches

Every file in this directory but one patches `@assistant-ui/react-ag-ui@0.0.62` (the exceptions, `zod@4.6.5.patch` and `zod@3.25.76.patch`, are at the end) (`patchedDependencies` in
[`../pnpm-workspace.yaml`](../pnpm-workspace.yaml)). The policy, in short: each patch has an
upstream issue or pull request, a patch is deleted when a pinned release contains the fix, and
nothing is filed without the owner's say-so. **None of the texts below has been filed** (drafted
2026-09-29); each patch file starts with a comment that says so and points here. When one is filed,
replace "not yet filed" in the patch header and in the section below with the link.

Repository: <https://github.com/assistant-ui/assistant-ui> (the package is
`packages/react-ag-ui`). Facts are *verified 2026-09-29* against the published package
`@assistant-ui/react-ag-ui@0.0.62` (`dist/` and `src/` as shipped) and `@ag-ui/client@1.0.0`.

| Section | Patch | Status |
|---|---|---|
| [Run outcome `cancelled`](#run-outcome-cancelled) | [`@assistant-ui__react-ag-ui@0.0.62.patch`](@assistant-ui__react-ag-ui@0.0.62.patch) | not yet filed |
| [Export the thread core](#export-the-thread-core) | [`@assistant-ui__react-ag-ui@0.0.62.patch`](@assistant-ui__react-ag-ui@0.0.62.patch) (`package.json`) | not yet filed |
| [Support `@ag-ui/client` 1.x](#support-agui-client-1x) | none (a pnpm `override`, not a patch) | not yet filed |
| [Observed, not patched](#observed-not-patched) | none | not yet filed |

## Run outcome `cancelled`

The patch: [`@assistant-ui__react-ag-ui@0.0.62.patch`](@assistant-ui__react-ag-ui@0.0.62.patch)
(`src/runtime/event-parser.ts`, `src/runtime/adapter/run-aggregator.ts`, `src/runtime/types.ts`, and
the same three files in `dist/`, which is what runs). Upstream: not yet filed.

**Title:** `RUN_FINISHED` with `outcome: { type: "cancelled" }` is shown as a successful run

**Problem.** AG-UI 1.0 (`RUN_FINISHED.outcome`) has three outcomes: `success`, `interrupt` and
`cancelled`. The lifecycle page says a run that a producer stopped on purpose MUST end with
`RUN_FINISHED{outcome:{type:"cancelled"}}` and that it is shown as neither success nor failure
([events/lifecycle](https://docs.ag-ui.com/spec/1.0/events/lifecycle.md)). `AgUiRunFinishedOutcome`
has no `cancelled` member, `parseRunFinishedOutcome` logs "has unsupported outcome type" and drops
the outcome, and `RunAggregator` then ends the message as `{ type: "complete" }`. A server-side
cancel therefore looks like a finished answer. (The legacy `RUN_CANCELLED` event is handled: it gives
`{ type: "incomplete", reason: "cancelled" }`.)

**Minimal repro.** An agent whose run ends `RUN_STARTED`, `RUN_FINISHED{outcome:{type:"cancelled"}}`:

```ts
class CancelledAgent extends AbstractAgent {
  run() {
    return of<BaseEvent>(
      { type: EventType.RUN_STARTED, threadId: "t", runId: "r" },
      { type: EventType.RUN_FINISHED, threadId: "t", runId: "r", outcome: { type: "cancelled" } },
    );
  }
}
const { result } = renderHook(() => useAgUiRuntime({ agent: new CancelledAgent() }));
await act(() => result.current.thread.append("hi"));
result.current.thread.getState().messages.at(-1)?.status;
// today:    { type: "complete", reason: "unknown" }
// expected: { type: "incomplete", reason: "cancelled" }
```

**Proposed change.** Add `{ type: "cancelled" }` to `AgUiRunFinishedOutcome`; accept it in
`parseRunFinishedOutcome`; in `RunAggregator`'s `RUN_FINISHED` case, treat it like `RUN_CANCELLED`
(status `incomplete`/`cancelled`, open subagent runs closed with that status, interrupts cleared).
Three small hunks, no behaviour change for the other outcomes. Our test:
`web/src/features/chat/lib/agui/runtime-goldens.dom.test.tsx` (the `cancel` golden fails without it).

## Export the thread core

**Title:** Export `AgUiThreadRuntimeCore` as a subpath, so a thread can be built without React

**Problem.** `AgUiThreadRuntimeCore` holds everything a transcript is made of (the run aggregator, the interrupts, the
messages) and has no React in it, but the package's `exports` map has only `.`, and `.` does not export the class. An
application that has to make the messages of many past runs at once (opening a long thread from a page of its history, ADR 0059 of
this repository) can only drive the visible runtime a run at a time: each run is `append`, a wait for React to show it, `startRun`,
and a wait again, about a quarter of a second a run whatever the transcript. The same code run directly takes 4 to 10 ms a run.

**Finding.** The class needs an agent (`AbstractAgent`), a logger and `notifyUpdate`; `getMessages()` is the transcript after
`append` and `reload` resolve. Nothing in it reads the DOM. Our test: `web/src/features/chat/lib/agui/seed.dom.test.tsx` makes the
messages of every golden both ways and they are equal (ids the runtime invents aside).

**Proposed change.** One entry in `exports`: `"./runtime/core": { "types": "./dist/runtime/AgUiThreadRuntimeCore.d.ts",
"default": "./dist/runtime/AgUiThreadRuntimeCore.js" }`. No code change. A supported way to make a transcript without a render
would be better than a deep import, but the export is what we need.

## Support `@ag-ui/client` 1.x

Not a patch: `web/pnpm-workspace.yaml` overrides `@ag-ui/client` to `1.0.0`, and the web depends on it
directly (spike S5, [`../README.md`](../README.md#spike-s5-the-runtime-on-agui-client-100)).

**Title:** Support `@ag-ui/client` `^1.0.0`

**Problem.** `@assistant-ui/react-ag-ui@0.0.62` depends on `@ag-ui/client` `^0.0.59`, a week after
AG-UI 1.0 and `@ag-ui/client@1.0.0` shipped (2026-09-17). An application on 1.x ends up with two
copies of the client, or overrides the range.

**Finding.** The runtime imports one value from `@ag-ui/client` (`buildResumeArray`, in
`tool-approval`) and otherwise only types (`AbstractAgent`, `AgentSubscriber`, `RunAgentParameters`,
`RunAgentResult`); both exist in 1.0.0 with the same shape. With the override, the runtime drives
`AbstractAgent.runAgent`/`connectAgent` of 1.0.0 and passes our goldens, the one gap being the
`cancelled` outcome above (1.0 is where it appears).

**Proposed change.** Widen the dependency to `^1.0.0` (or accept both majors), and land the
`cancelled` outcome with it.

## Observed, not patched

Two things the plan expected to patch turned out not to need it in this web, and three more were met
by the A2UI renderer (slice 13); all are real in the package and are drafted here in case the owner
wants them upstream. **None is filed.**

### Activities before any assistant message are dropped from a restored history

**Title:** `fromAgUiMessages` drops an activity message that precedes every assistant message

**Problem.** `conversions.ts` attaches an `activity` message to the assistant message before it and
skips it when there is none (`if (ownerIndex === -1) continue;`). A producer that reports progress
before it says anything, as ours does (`vymalo.status: working` opens every run), loses those
activities on every restore from a `MESSAGES_SNAPSHOT` or a history adapter, while the same run
streamed live keeps them.

**Repro.** *Verified 2026-09-29:*

```ts
fromAgUiMessages([
  { id: "u1", role: "user", content: "hi" },
  { id: "a1", role: "activity", activityType: "vymalo.status", content: { status: "working" } },
  { id: "m1", role: "assistant", content: "hello" },
  { id: "a2", role: "activity", activityType: "vymalo.status", content: { status: "completed" } },
]);
// the assistant message carries text, and the a2 data part; a1 is gone
```

**Proposed change.** Keep an owner-less activity as an assistant message of its own (content: the
data part). We did not patch it because the web never restores through `fromAgUiMessages`: it
replays the connect stream through the run aggregator, which keeps every activity
([`../README.md`](../README.md#the-chat-layer)).

### Applying runs the client did not start

**Title:** Follow a thread across runs (`agent.connect`) instead of only inside `runAgent`

**Problem.** The runtime applies events only from a run it started with `agent.runAgent`. A run
started elsewhere (another tab, a webhook, a run already open at page load) never reaches it, and
the in-flight resume path (`ThreadHistoryAdapter.resume`) yields `ChatModelRunResult`s, not AG-UI
events, and is per load. `@ag-ui/client` already has `connect()`/`connectAgent()` for exactly this.

**Proposed change.** An opt-in: when the agent implements `connect()`, the runtime attaches to it
after the history load and applies each run it did not start through the same run aggregator,
appending the run's user messages (`TEXT_MESSAGE_*` with `role: "user"`, which the aggregator
currently renders as assistant text). We solve it outside the package with its public API only:
`web/src/features/chat/lib/agui/live-runs.ts` appends the user message, starts a run
(`thread.startRun`, or `steerAway` when an interrupt is open) and makes the agent's `run()` serve
the external run's events. That is a workable pattern, not a substitute for the runtime knowing
about `connect()`.

### A2UI: `v0.9.1` operations are dropped, and the built-in path converts before any host check

**Title:** `applyA2uiOperations` rejects `version: "v0.9.1"`, which the A2UI docs say is read as `v0.9`

**Problem.** *Verified 2026-09-29* against `@assistant-ui/react-generative-ui@0.0.21` (`src/a2ui/reducer.ts`,
`isVersion`): only `"v0.9"` and `"v1.0"` are accepted. The A2UI page on assistant-ui.com says
"v0.9.1 (read as v0.9)". An agent that says `v0.9.1`, as the current A2UI release does (and as our
orchestrator relays it, `docs/api/examples/agui/a2ui.agui.json`), gets `Operation at index n has an
unsupported version.` for every operation, and `@assistant-ui/react-ag-ui` draws nothing.

**Repro.**

```ts
applyA2uiOperations(new Map(), [{ version: "v0.9.1", createSurface: { surfaceId: "s1" } }]);
// state.size === 0, warnings: ["Operation at index 0 has an unsupported version."]
```

**Proposed change.** Accept `"v0.9.1"` (as `"v0.9"`). We do not patch it: the web hands surfaces to its
own validator, which maps `v0.9.1` to `v0.9` before the reducer sees them
([`../README.md`](../README.md#a2ui-surfaces)).

A second observation belongs with it: the runtime converts an `a2ui-surface` snapshot in the run
aggregator, the moment it arrives, so a host has no place to validate before conversion (the
conversion is bounded, at depth 32 and 5000 nodes, but not by the host's rules). We rename the activity
in `ThreadAgent` so the runtime never converts, and convert after our checks.

### A2UI: `sendA2uiAction` throws while an interrupt is open

**Title:** `useAgUiSendA2uiAction` cannot answer a thread that is waiting on an interrupt

**Problem.** `AgUiThreadRuntimeCore.sendA2uiAction` starts with `assertNoPendingInterrupts()`, which
throws `cannot start a new run while interrupts are pending`. A surface that comes with the agent's
question (`input-required`, so the run ended in an interrupt) is exactly when its button is used.

**Proposed change.** Let `sendA2uiAction` close the open interrupts as cancelled and start the run,
as `steerAway` does for a message. We do not patch it: the app closes the interrupt through
`unstable_submitInterruptResponses` and stages the action on its agent, which sends it instead of
the `resume` (`ThreadAgent.stageA2uiAction`).

### A2UI: the pinned converter has no `openUrl`, `userMessage` or input values

**Title:** The A2UI docs describe converter features that `0.0.21` does not have

**Problem.** *Verified 2026-09-29.* The A2UI page on assistant-ui.com describes `a2ui:functionCall`
(`openUrl`), the event's `userMessage`, `$field` references for bound inputs and template children
written `{componentId, path}`. `@assistant-ui/react-generative-ui@0.0.21` (the latest on npm, published
2026-09-24; `src/a2ui/convert.ts`) has none of them: `mappedAction` drops a `functionCall` action,
ignores `userMessage`, and `children` must be `{template: {componentId, path}}`. The docs appear to
describe the repository head.

**Proposed change.** None for the package; a note on the docs page, or a release that matches it. The
web lowers what it needs before conversion (an `openUrl` call, a `userMessage`, the current value of
an input) so that the pinned converter can carry it.

## zod without the `new Function` probe

`zod@4.6.5.patch` (a dependency of `@assistant-ui/react-ag-ui`) and `zod@3.25.76.patch` (whose `zod/v4` is what
`@ag-ui/client` builds its schemas with) make zod's `allowsEval` answer `false` without
trying `new Function("")`. Zod 4 probes for eval once, when the first object schema is built, and a content
security policy without `unsafe-eval` reports the caught throw as a `securitypolicyviolation`, which the web's
policy (ADR 0054, decision 10: no `unsafe-eval`, and `e2e/csp.spec.ts` counts violations) must not have. Zod's own
escape is `z.config({ jitless: true })`, but it has to run before any schema is built, and the schemas are built
when `@assistant-ui` is imported, from many files: a patch is the one place that is early enough. The cost is
zod's JIT-compiled parsers, which the web's small payloads do not need. Delete the patch when a pinned
`@assistant-ui/react-ag-ui` sets `jitless` itself or zod stops probing. Not filed upstream.
