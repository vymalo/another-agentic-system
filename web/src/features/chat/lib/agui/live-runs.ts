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
  // when it was sent, in the log: without it the runtime says now, which is when the page was opened
  ...(m.at ? { createdAt: new Date(m.at) } : {}),
  startRun,
  // `seq`: where the message is in the log, which a fork or an edit of it names (ADR 0029)
  metadata: {
    custom: {
      ...(m.actor ? { actor: m.actor } : {}),
      ...(m.seq !== undefined ? { seq: m.seq } : {}),
      // sent while the agent worked (ADR 0036): the bubble says so
      ...(m.delivery ? { delivery: m.delivery } : {}),
      // the agents the message mentions (ADR 0026): the bubble draws them as chips
      ...(m.mentions ? { mentions: m.mentions } : {}),
    },
  },
});

/** How long `untilShown` waits for the runtime before it goes on without the transcript. */
export const SHOWN_TIMEOUT_MS = 5_000;

type Watched = Pick<LiveRunsRuntime["thread"], "getState" | "subscribe">;

/**
 * Waits until the transcript `thread.getState()` shows `atLeast` messages and is not running.
 *
 * That transcript lags the runtime by a render, and `ThreadRuntime.append` hangs a message off its
 * last message (as `startRun` does with the head it is given): a run applied before the earlier
 * one showed up would start a branch of its own and replace the earlier runs for good. A replay
 * delivers every run of a thread at once, and since a message on a finished thread starts the next
 * job (ADR 0020) a thread has several runs with nothing between them. How late the render comes is
 * up to the machine, so no time says it has come: what the replay knows does. Every run it applies
 * leaves a known number of messages (`applyExternalRun`), and the next run waits until the
 * transcript shows them all.
 *
 * It listens to the runtime's own state subscription instead of polling, and resolves `true` when
 * the condition holds. If it does not hold within `timeoutMs` (a run that never ends, a count that
 * is not what the replay expected) it warns and resolves `false`, and the caller goes on with the
 * transcript it has.
 */
export function untilShown(
  thread: Watched,
  atLeast: number,
  timeoutMs = SHOWN_TIMEOUT_MS,
): Promise<boolean> {
  return new Promise((resolve) => {
    const finish = (caughtUp: boolean) => {
      clearTimeout(timer);
      unsubscribe();
      if (!caughtUp) {
        console.warn(
          `The transcript did not show ${atLeast} messages within ${timeoutMs} ms (it shows ${thread.getState().messages.length}): going on without them`,
        );
      }
      resolve(caughtUp);
    };
    const look = () => {
      const state = thread.getState();
      if (!state.isRunning && state.messages.length >= atLeast) finish(true);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    const unsubscribe = thread.subscribe(look);
    look();
  });
}

/**
 * Waits until the transcript shows `atLeast` messages, running or not (`untilShown` waits for the
 * end of the run too, which a run that is still open does not have). Resolves when it does, or
 * after `timeoutMs` (a count that is not what the replay expected must not hold a send back for
 * ever).
 */
export function untilHeld(
  thread: Watched,
  atLeast: number,
  timeoutMs = SHOWN_TIMEOUT_MS,
): Promise<void> {
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer);
      unsubscribe();
      resolve();
    };
    const timer = setTimeout(finish, timeoutMs);
    const unsubscribe = thread.subscribe(() => {
      if (thread.getState().messages.length >= atLeast) finish();
    });
    if (thread.getState().messages.length >= atLeast) finish();
  });
}

/**
 * What the replay knows of the transcript the runtime holds, whether or not it is shown yet: how
 * many messages the runs applied so far leave in it (`applyExternalRun` raises it).
 */
export type Held = { messages: number };

/**
 * Applies one external run and resolves when the runtime finished it. Before that it sets
 * `held.messages` to what the transcript holds once it shows the run: what it showed before, the
 * run's user messages and the assistant message the runtime makes for the run (a run that has any
 * material for the transcript always gets one, even with no output). It does so up front, so a run
 * that ends in an error, which rejects, has counted too.
 */
export async function applyExternalRun(
  agent: ThreadAgent,
  run: ExternalRun,
  runtime: LiveRunsRuntime,
  steerAway: SteerAway,
  /** What the runs before this one left in the transcript, which may not show it yet (see `untilShown`). */
  held: Held = { messages: 0 },
): Promise<void> {
  await run.leadIn;
  const thread = runtime.thread;
  // The first run has no transcript to hang off: its parent is the start. One after a run the runtime
  // made itself (this page sent it: its message is in the transcript, and the run may not have ended
  // there yet, which the stream said first) has one, whatever the replay has counted.
  const after = held.messages > 0 || thread.getState().messages.length > 0;
  if (after) await untilShown(thread, held.messages);
  const users = [...run.userMessages];
  const pending = runtime.unstable_getPendingInterrupts();
  let shown = thread.getState().messages.length;
  held.messages = shown + users.length + 1;
  // once the runtime shows what the run leaves, it is the head a new message hangs off: a send is
  // held back until then (`ThreadSnapshot.replaying`), or it would replace the turns before it
  void untilHeld(thread, held.messages).then(() => agent.applied(run));
  agent.adopt(run);
  if (pending.length > 0) {
    const last = users.pop();
    for (const m of users) {
      await thread.append(userMessage(m, false));
      if (after) await untilShown(thread, ++shown);
    }
    if (last) await steerAway(userMessage(last, true));
    else {
      await runtime.unstable_submitInterruptResponses(
        pending.map((i) => ({ interruptId: i.id, status: "cancelled" as const })),
      );
    }
    return;
  }
  // `append` and `startRun` read the transcript `thread.getState()` shows (see `untilShown`): after
  // an earlier run, let it show each message that has been appended before the next step reads it.
  for (const m of users) {
    await thread.append(userMessage(m, false));
    if (after) await untilShown(thread, ++shown);
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
  const held: Held = { messages: 0 };
  for (;;) {
    const run = await agent.nextExternalRun(signal);
    if (!run || signal.aborted) return;
    try {
      await applyExternalRun(agent, run, runtime(), steerAway(), held);
    } catch (e) {
      // A run that ended in RUN_ERROR rejects the runtime's run: its transcript is complete.
      if (!isRunFailure(e)) console.warn("Could not apply a run to the transcript", e);
    } finally {
      agent.applied(run);
    }
  }
}

/** The error the runtime throws when the run it drove ended in `RUN_ERROR`. */
const isRunFailure = (e: unknown): boolean =>
  e instanceof Error && typeof (e as { code?: unknown }).code === "string";
