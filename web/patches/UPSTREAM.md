# Upstream twins of the patches

Every file in this directory patches `@assistant-ui/react-ag-ui@0.0.62` (`patchedDependencies` in
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

Two things the plan expected to patch turned out not to need it in this web; both are real in the
package and are drafted here in case the owner wants them upstream.

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
