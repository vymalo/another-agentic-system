import type { ApiThread } from "@/lib/api/types";
import { cardNewer, childOf, type SessionCard } from "./cards";
import type { AppliedClose } from "./children";

/**
 * A subagent's own thread, seen from inside it (child-sessions/v1, docs/api/agui.md "Child sessions as
 * subagents"): what the banner and the composer follow. Two sources say it, and the newer wins:
 *
 * - `GET /api/threads/{id}` (`childOf`), which the page reads again at every event of the log;
 * - the live `session-self` card of the stream (`snapshot.self`), which is faster than the fetch.
 *
 * A close the person asked for in this page is applied at once, before either says it.
 */

/** Where the session stands for the box: it takes messages, it closes (a round ends first, or the close is on its way), or it is closed. */
export type ChildPhase = "open" | "closing" | "closed";

export type ChildView = SessionCard & {
  /** The parent thread, a UUID; absent when the thread's is not one. */
  parent?: string;
  phase: ChildPhase;
};

export function childView(input: {
  thread: ApiThread | null;
  /** This thread's id, so a close can name it. */
  threadId: string;
  self: SessionCard | null;
  applied?: AppliedClose | undefined;
}): ChildView | null {
  const { thread, threadId, self, applied } = input;
  const row = childOf(thread);
  if (row === null && self === null) return null;
  const useSelf = self !== null && (row === null || cardNewer(self, thread?.updatedAt));
  const base: SessionCard = useSelf
    ? self
    : {
        session: row?.session ?? "",
        agent: row?.agent ?? self?.agent ?? "",
        description: "",
        keepOpen: row?.keepOpen ?? false,
        state: row?.state ?? self?.state ?? "idle",
        closing: row?.closing ?? false,
        closeAsked: row?.closeAsked ?? false,
      };
  const closed = base.state === "closed" || applied === "closed";
  const closing = !closed && (base.closing || base.closeAsked || applied === "closing");
  return {
    ...base,
    // the self card says nothing of the agent's name once the thread knows it
    agent: base.agent || row?.agent || self?.agent || "",
    threadId,
    ...(row?.parent ? { parent: row.parent } : {}),
    ...(self?.report && useSelf ? { report: self.report } : {}),
    state: closed ? "closed" : base.state,
    closing,
    phase: closed ? "closed" : closing ? "closing" : "open",
  };
}
