import type { ApiToolServer } from "@/lib/api/types";

/*
 * What the deployment offers for attaching (`GET /api/tool-servers`, ADR 0024) and what a thread
 * has, as the questions a screen asks. Nothing here is a check: the orchestrator refuses a server
 * the thread's agent may not use (422), a thread the person may not change (403) and more than 16
 * (422); the screen only stops offering what it would refuse.
 */

/** The A2A extension an agent lists when it can use the tools attached to a thread. */
export const THREAD_TOOLS_URI = "https://agents.vymalo.com/a2a/extensions/thread-tools/v1";

/** At most this many distinct servers on one thread (`ThreadToolsRequest`). */
export const MAX_ATTACHED = 16;

/**
 * The servers that may be attached for `agentId`: every listed one whose `agents` is absent or names
 * the agent. With no agent known yet (a thread still loading) the list is whole: the server decides.
 */
export function offeredFor(
  servers: readonly ApiToolServer[],
  agentId: string | null,
): ApiToolServer[] {
  return servers.filter(
    (s) => agentId === null || s.agents === undefined || s.agents.includes(agentId),
  );
}

/** The ids as a sorted list without repeats: the order a thread keeps them in. */
export const sortedSet = (ids: Iterable<string>): string[] => [...new Set(ids)].sort();

export const sameSet = (a: readonly string[], b: readonly string[]): boolean => {
  const x = sortedSet(a);
  const y = sortedSet(b);
  return x.length === y.length && x.every((id, i) => id === y[i]);
};

/** The choice with `id` on or off, sorted. */
export const withServer = (chosen: readonly string[], id: string, on: boolean): string[] =>
  sortedSet(on ? [...chosen, id] : chosen.filter((c) => c !== id));

/**
 * What a new chat keeps of its choice when the agent changes or the list is read again: the ids
 * the agent may have attached. A server the deployment stopped listing is dropped too (the
 * orchestrator would answer 422 for it).
 */
export function stillOffered(
  chosen: readonly string[],
  servers: readonly ApiToolServer[],
  agentId: string | null,
): string[] {
  const offered = new Set(offeredFor(servers, agentId).map((s) => s.id));
  return chosen.filter((id) => offered.has(id));
}

/** What the person reads for a server: its name when the list has it, else its id. */
export const nameOf = (servers: readonly ApiToolServer[], id: string): string =>
  servers.find((s) => s.id === id)?.name ?? id;

/** What the stream and the resource each say about the attached servers, and when. */
export type ToolsSources = {
  /** `STATE_SNAPSHOT.thread.tools` of the last group of the stream; undefined: none. */
  stream: readonly string[] | undefined;
  /** The `seq` of the last group the stream delivered (0: nothing yet). */
  streamSeq: number;
  /** `Thread.tools` of the last read of the resource. */
  fetched: readonly string[] | undefined;
  /** `Thread.lastSeq` of that read; null: not read yet. */
  fetchedSeq: number | null;
  /** The answer of the last `PUT`, and how far the log was when it came. */
  put: { servers: readonly string[]; seq: number } | null;
};

/**
 * The servers attached to the thread now: what the newest of the stream, the resource and the
 * person's own last change says. They are ordered by the log (`seq`); an answer of `PUT` stands
 * until something newer than the log was when it came says otherwise (the change it made is an
 * event, so the next read is newer).
 */
export function currentTools(s: ToolsSources): string[] {
  const fetchedSeq = s.fetchedSeq ?? -1;
  const read = s.streamSeq >= fetchedSeq ? (s.stream ?? []) : (s.fetched ?? []);
  const readSeq = Math.max(s.streamSeq, fetchedSeq);
  if (s.put && readSeq <= s.put.seq) return sortedSet(s.put.servers);
  return sortedSet(read);
}
