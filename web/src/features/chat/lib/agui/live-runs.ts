import type { CreateAppendMessage } from "@assistant-ui/react";
import type { AgUiAssistantRuntime, AgUiResumeEntry } from "@assistant-ui/react-ag-ui";
import type { ExternalRun, ExternalUserMessage, ThreadAgent } from "./thread-agent";

/**
 * Hands the runs nobody here started (the replay of the thread at load, another tab, a producer's
 * own run) to `@assistant-ui/react-ag-ui`, one at a time, through its public API only.
 *
 * The runtime applies events for runs it starts itself. For a run it did not start, this does
 * what a user's send does, minus the POST: it appends the run's user message (if it has one) and
 * starts a run, and `ThreadAgent.adopt()` makes that run read the external run's events instead
 * of sending. The reply is then aggregated by the runtime's own code, so a replayed run, a live
 * one and one the user started all render the same way.
 *
 * An unanswered interrupt makes the runtime refuse a new run, so an external run that follows one
 * (the other tab answered it) goes through `steerAway` / `unstable_submitInterruptResponses`,
 * which close the interrupt and start the run.
 */
export type LiveRunsRuntime = Pick<
  AgUiAssistantRuntime,
  "thread" | "unstable_getPendingInterrupts" | "unstable_submitInterruptResponses"
>;

export type SteerAway = (
  message: CreateAppendMessage,
  responses?: readonly AgUiResumeEntry[],
) => Promise<void>;

const userMessage = (m: ExternalUserMessage, startRun: boolean): CreateAppendMessage => ({
  role: "user",
  content: [{ type: "text", text: m.text }],
  startRun,
  // `seq`: where the message is in the log, which a fork or an edit of it names (ADR 0029)
  metadata: {
    custom: {
      ...(m.actor ? { actor: m.actor } : {}),
      ...(m.seq !== undefined ? { seq: m.seq } : {}),
    },
  },
});

/**
 * How long the transcript must stay unchanged, and not running, to count as caught up: enough for
 * the runtime to commit what it has been handed (it does so over several turns of the event loop).
 */
const SETTLE_MS = 20;
/** How long `quiesce` waits for the runtime before it goes on without the transcript. */
export const QUIESCE_TIMEOUT_MS = 5_000;

type Watched = Pick<LiveRunsRuntime["thread"], "getState" | "subscribe">;

/**
 * Waits until the transcript `thread.getState()` shows has caught up with what the runtime holds:
 * the state lags the runtime by a render. `ThreadRuntime.append` hangs a message off the last
 * message of *that* state, so a run applied before the previous one showed up would start a
 * branch of its own and replace the earlier runs. A replay delivers every run of a thread at
 * once, and since a message on a finished thread starts the next job (ADR 0020) a thread has
 * several runs with nothing between them.
 *
 * It listens to the runtime's own state subscription instead of polling: it resolves `true` when
 * the thread is not running, it shows at least `atLeast` messages (what the caller just appended)
 * and its message count has not changed for `settleMs`. If that does not
 * happen within `timeoutMs` (a run that never ends) it warns and resolves `false`, and the caller
 * goes on with the transcript it has.
 */
export function quiesce(
  thread: Watched,
  timeoutMs = QUIESCE_TIMEOUT_MS,
  settleMs = SETTLE_MS,
  /** The transcript is not caught up before it shows this many messages. */
  atLeast = 0,
): Promise<boolean> {
  return new Promise((resolve) => {
    let settle: ReturnType<typeof setTimeout> | undefined;
    const finish = (caughtUp: boolean) => {
      clearTimeout(settle);
      clearTimeout(timer);
      unsubscribe();
      if (!caughtUp) {
        console.warn(
          `The transcript did not settle within ${timeoutMs} ms (the thread is still running): going on without it`,
        );
      }
      resolve(caughtUp);
    };
    const look = () => {
      clearTimeout(settle);
      const state = thread.getState();
      const count = state.messages.length;
      if (state.isRunning || count < atLeast) return; // the next notification looks again
      settle = setTimeout(() => {
        const now = thread.getState();
        if (!now.isRunning && now.messages.length === count) finish(true);
        else look();
      }, settleMs);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    const unsubscribe = thread.subscribe(look);
    look();
  });
}

/** Applies one external run and resolves when the runtime finished it. */
export async function applyExternalRun(
  agent: ThreadAgent,
  run: ExternalRun,
  runtime: LiveRunsRuntime,
  steerAway: SteerAway,
  /** A run was applied before this one: the transcript may not show it yet (see `quiesce`). */
  after = false,
): Promise<void> {
  await run.leadIn;
  const thread = runtime.thread;
  if (after) await quiesce(thread);
  const users = [...run.userMessages];
  const pending = runtime.unstable_getPendingInterrupts();
  agent.adopt(run);
  if (pending.length > 0) {
    const last = users.pop();
    for (const m of users) await thread.append(userMessage(m, false));
    if (last) await steerAway(userMessage(last, true));
    else {
      await runtime.unstable_submitInterruptResponses(
        pending.map((i) => ({ interruptId: i.id, status: "cancelled" as const })),
      );
    }
    return;
  }
  // `append` and `startRun` read the transcript `thread.getState()` shows (see `quiesce`): after an
  // earlier run, let it show what has been appended before each step. The first run has no
  // transcript to hang off: its parent is the start.
  let shown = thread.getState().messages.length;
  for (const m of users) {
    await thread.append(userMessage(m, false));
    // the message is in the transcript before the next step reads it
    if (after) await quiesce(thread, QUIESCE_TIMEOUT_MS, SETTLE_MS, ++shown);
  }
  const head = thread.getState().messages.at(-1);
  await thread.startRun({ parentId: head?.id ?? null });
}

/** The loop: takes external runs in order until `signal` aborts. */
export async function driveExternalRuns(
  agent: ThreadAgent,
  runtime: () => LiveRunsRuntime,
  steerAway: () => SteerAway,
  signal: AbortSignal,
): Promise<void> {
  let applied = false;
  for (;;) {
    const run = await agent.nextExternalRun(signal);
    if (!run || signal.aborted) return;
    try {
      const after = applied;
      applied = true;
      await applyExternalRun(agent, run, runtime(), steerAway(), after);
    } catch (e) {
      // A run that ended in RUN_ERROR rejects the runtime's run: its transcript is complete.
      if (!isRunFailure(e)) console.warn("Could not apply a run to the transcript", e);
    }
  }
}

/** The error the runtime throws when the run it drove ended in `RUN_ERROR`. */
const isRunFailure = (e: unknown): boolean =>
  e instanceof Error && typeof (e as { code?: unknown }).code === "string";
