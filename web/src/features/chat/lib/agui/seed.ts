import { AbstractAgent, type BaseEvent, type RunAgentInput } from "@ag-ui/client";
import type { ExportedMessageRepository, ThreadMessage } from "@assistant-ui/react";
import { AgUiThreadRuntimeCore } from "@assistant-ui/react-ag-ui/runtime/core";
import { EMPTY, type Observable } from "rxjs";
import { userMessage } from "./live-runs";
import type { ExternalRun } from "./thread-agent";

/*
 * The seed (ADR 0059): the messages of a page of turns, made without the runtime that shows them.
 *
 * A replay applies a run at a time through the visible runtime: `append`, wait for React to show it, `startRun`,
 * wait again (live-runs.ts), and that is a fixed cost per run of a quarter of a second whatever the transcript, on top
 * of the render of every turn so far. The transcript of a page is made here instead, by the thread core of
 * `@assistant-ui/react-ag-ui` (the same code the visible runtime runs, exported by web/patches) driven the way
 * `applyExternalRun` drives the runtime, minus the waits for a render that nobody does: the core's state is the
 * messages, synchronously. The result goes into the visible runtime in one `import`.
 */

/** Runs the core with the frames of the run it is told to follow (the way `ThreadAgent.adopt` does). */
class ReplayAgent extends AbstractAgent {
  next: ExternalRun | null = null;

  override run(_input: RunAgentInput): Observable<BaseEvent> {
    const run = this.next;
    this.next = null;
    return run ? run.frames.asObservable() : EMPTY;
  }
}

const quiet = { debug: () => {}, error: () => {} };

/**
 * The messages the runs make, in order, as the visible runtime would hold them after applying the same runs one by one
 * (the equality is the gate of lib/agui/seed.dom.test.tsx, on every golden).
 *
 * The runs must be complete (a page ends at a settled point). The message ids are the runtime's own: a text message's
 * id is the server's, a run without text gets a random one.
 */
export async function buildMessages(
  runs: readonly ExternalRun[],
  threadId: string,
  /**
   * Later turns follow these. A question the last run ended on is then no longer waiting: the run that follows answers it or
   * steers away from it, which is what the replay does to its message (`clearPendingInterrupts`), and this does the same.
   */
  followed = false,
): Promise<ThreadMessage[]> {
  const agent = new ReplayAgent({ threadId });
  // a run that ends in RUN_ERROR rejects the core's run after it has told `onError`: its transcript is complete
  const failed = new Set<unknown>();
  const core = new AgUiThreadRuntimeCore({
    agent,
    logger: quiet,
    showThinking: true,
    resumeTranscript: "appended",
    notifyUpdate: () => {},
    onError: (error) => void failed.add(error),
  });
  // the core's public surface is typed loosely where it takes a message the runtime has already completed
  const append = (message: ReturnType<typeof userMessage>) =>
    core.append(message as unknown as Parameters<typeof core.append>[0]);
  const head = () => core.getMessages().at(-1)?.id ?? null;
  for (const run of runs) {
    agent.next = run;
    const users = [...run.userMessages];
    const pending = core.getPendingInterrupts();
    try {
      if (pending) {
        // a run after an unanswered question: the person's reply (or the run itself) closes it
        const last = users.pop();
        for (const m of users) await append(userMessage(m, false));
        if (last) {
          await core.steerAway(
            userMessage(last, true) as unknown as Parameters<typeof core.steerAway>[0],
          );
        } else {
          await core.submitInterruptResponses(
            pending.interrupts.map((i) => ({ interruptId: i.id, status: "cancelled" as const })),
          );
        }
        continue;
      }
      for (const m of users) await append(userMessage(m, false));
      await core.reload(head());
    } catch (e) {
      // anything but a run that failed is a transcript that cannot be trusted: the caller replays the log instead
      if (!failed.has(e)) throw e;
    }
  }
  const pending = followed ? core.getPendingInterrupts() : null;
  if (pending) {
    // not in the core's types, and what a replay does to the message of a question the next run answered
    const clear = (core as unknown as { clearPendingInterrupts?: unknown }).clearPendingInterrupts;
    if (typeof clear !== "function") {
      throw new Error("the runtime no longer settles the question of an earlier turn");
    }
    clear.call(
      core,
      pending.messageId,
      pending.interrupts.map((i) => ({ interruptId: i.id, status: "cancelled" as const })),
    );
  }
  return [...core.getMessages()];
}

/** A transcript as the linear chain `thread.import` takes: each message's parent is the one before. */
export function asRepository(messages: readonly ThreadMessage[]): ExportedMessageRepository {
  return {
    headId: messages.at(-1)?.id ?? null,
    messages: messages.map((message, i) => ({ message, parentId: messages[i - 1]?.id ?? null })),
  };
}

/**
 * The older messages in front of the current ones, as one transcript. A message id the two hold twice is kept once, **the
 * newest copy**, in its place: the runtime keeps the first copy of a repeated id (`applyExternalMessages`), which would
 * show the older page's state of a thing a later page says again.
 */
export function joinMessages(
  older: readonly ThreadMessage[],
  current: readonly ThreadMessage[],
): ThreadMessage[] {
  const later = new Set(current.map((m) => m.id));
  return [...older.filter((m) => !later.has(m.id)), ...current];
}
