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
  metadata: { custom: m.actor ? { actor: m.actor } : {} },
});

/** Applies one external run and resolves when the runtime finished it. */
export async function applyExternalRun(
  agent: ThreadAgent,
  run: ExternalRun,
  runtime: LiveRunsRuntime,
  steerAway: SteerAway,
): Promise<void> {
  await run.leadIn;
  const thread = runtime.thread;
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
  for (const m of users) await thread.append(userMessage(m, false));
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
  for (;;) {
    const run = await agent.nextExternalRun(signal);
    if (!run || signal.aborted) return;
    try {
      await applyExternalRun(agent, run, runtime(), steerAway());
    } catch (e) {
      // A run that ended in RUN_ERROR rejects the runtime's run: its transcript is complete.
      if (!isRunFailure(e)) console.warn("Could not apply a run to the transcript", e);
    }
  }
}

/** The error the runtime throws when the run it drove ended in `RUN_ERROR`. */
const isRunFailure = (e: unknown): boolean =>
  e instanceof Error && typeof (e as { code?: unknown }).code === "string";
