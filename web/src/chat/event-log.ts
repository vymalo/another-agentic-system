import { type ApiEvent, type ThreadState, type TypedEvent, toTypedEvent } from "@/api/types";

/**
 * The client-side copy of a thread's append-only event log.
 *
 * Pure and immutable: `applyEvents` returns the same object when nothing changed so React
 * dependency checks stay stable across SSE replays.
 */
export type EventLog = {
  readonly threadId: string;
  readonly bySeq: ReadonlyMap<number, TypedEvent>;
  /** Sorted by seq. */
  readonly ordered: readonly TypedEvent[];
  readonly lastSeq: number;
};

export const emptyLog = (threadId: string): EventLog => ({
  threadId,
  bySeq: new Map(),
  ordered: [],
  lastSeq: 0,
});

/** Dedupe by seq, ignore other threads and malformed events, keep seq order. */
export function applyEvents(log: EventLog, incoming: readonly ApiEvent[]): EventLog {
  let fresh: Map<number, TypedEvent> | undefined;
  for (const raw of incoming) {
    if (raw.threadId !== log.threadId) continue;
    if (log.bySeq.has(raw.seq) || fresh?.has(raw.seq)) continue;
    const typed = toTypedEvent(raw);
    if (!typed) continue;
    fresh ??= new Map();
    fresh.set(typed.seq, typed);
  }
  if (!fresh) return log;

  const bySeq = new Map(log.bySeq);
  for (const [seq, e] of fresh) bySeq.set(seq, e);
  const ordered = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  return { threadId: log.threadId, bySeq, ordered, lastSeq: ordered.at(-1)?.seq ?? 0 };
}

/**
 * What the transcript shows: `thread_state` goes to the header badge instead, and every
 * `agent_message` with the same `messageId` collapses to one entry that keeps the position
 * of the first occurrence and the content of the highest seq (a partial is replaced by its
 * final version).
 */
export function visibleEvents(log: EventLog): TypedEvent[] {
  const latestByMessage = new Map<string, TypedEvent>();
  for (const e of log.ordered) {
    if (e.kind === "agent_message") latestByMessage.set(e.data.messageId, e);
  }
  const seen = new Set<string>();
  const out: TypedEvent[] = [];
  for (const e of log.ordered) {
    if (e.kind === "thread_state") continue;
    if (e.kind === "agent_message") {
      if (seen.has(e.data.messageId)) continue;
      seen.add(e.data.messageId);
      out.push(latestByMessage.get(e.data.messageId) ?? e);
      continue;
    }
    out.push(e);
  }
  return out;
}

/** The newest `thread_state` event in the log, if any. */
export function lastThreadStateEvent(
  log: EventLog,
): { state: ThreadState; seq: number } | undefined {
  for (let i = log.ordered.length - 1; i >= 0; i--) {
    const e = log.ordered[i];
    if (e?.kind === "thread_state") return { state: e.data.state, seq: e.seq };
  }
  return undefined;
}
