import type { ApiThread } from "@/lib/api/types";
import { childOf, type SessionCard, type SessionStartCard } from "./cards";

/**
 * The subagents of a parent thread, as the panel lists them. Three sources say it, and they are merged
 * by the session's id:
 *
 * - the rail rows whose `childOf` names the parent: the orchestrator's record, which wins on the state;
 * - the `vymalo.session` cards of the parent's stream, which cover a session whose rail page is not
 *   loaded (and carry the round and how it ended, which a row does not say);
 * - the starts a person asked for that have no outcome yet.
 *
 * A close the person asked for is applied at once (`applied`, by child thread), before the rail says it.
 */

/** How many starts of one thread may wait for their outcome (`startChildSession`: 409 `too_many_starts` past it). */
export const MAX_WAITING_STARTS = 4;

/** What a close answered: the state the orchestrator's record has after the request. */
export type AppliedClose = "closing" | "closed";

export type MergedChildren = {
  /** Working or idle (closing ones too), in the rail's order, then the ones only a card says, newest first. */
  open: SessionCard[];
  /** Closed, in the same order. */
  closed: SessionCard[];
  /** Starts that wait for their outcome. */
  pending: SessionStartCard[];
};

const same = (a: string | undefined, b: string | undefined): boolean =>
  a !== undefined && b !== undefined && a.toLowerCase() === b.toLowerCase();

/** How many starts wait: the stream's `starting` ones and the requests this page sent that the stream has not said yet. */
export function startsWaiting(
  starts: ReadonlyMap<string, SessionStartCard>,
  local: readonly string[] = [],
): number {
  const waiting = new Set<string>();
  for (const [request, card] of starts) if (card.state === "starting") waiting.add(request);
  for (const request of local) if (!starts.has(request)) waiting.add(request);
  return waiting.size;
}

export function mergeChildren(input: {
  parent: string;
  rows: readonly ApiThread[];
  cards: ReadonlyMap<string, SessionCard>;
  starts: ReadonlyMap<string, SessionStartCard>;
  applied: ReadonlyMap<string, AppliedClose>;
}): MergedChildren {
  const { parent, rows, cards, starts, applied } = input;
  const bySession = new Map<string, SessionCard>();
  for (const card of cards.values()) bySession.set(card.session, card);

  const merged: SessionCard[] = [];
  const seen = new Set<string>();
  for (const row of rows) {
    const own = childOf(row);
    if (!own || !same(own.parent, parent)) continue;
    seen.add(own.session);
    const card = bySession.get(own.session);
    // the thread says whether it waits for the person, so a card's older "blocked" is not kept against it
    const { report: said, ...rest } = card ?? { report: undefined };
    const report = row.state === "blocked" ? "blocked" : said === "blocked" ? undefined : said;
    merged.push({
      ...rest,
      ...(report ? { report } : {}),
      session: own.session,
      threadId: row.id,
      agent: own.agent ?? card?.agent ?? "",
      description: row.title || card?.description || "",
      keepOpen: own.keepOpen,
      state: own.state ?? card?.state ?? "idle",
      closing: own.closing,
      closeAsked: own.closeAsked,
    });
  }
  // the ones only a card says: the newest card first, as the rail's is
  const only = [...bySession.values()].filter((c) => !seen.has(c.session)).reverse();
  merged.push(...only);

  const settled = merged.map((card): SessionCard => {
    const close = card.threadId === undefined ? undefined : applied.get(card.threadId);
    if (close === "closed") return { ...card, state: "closed" };
    if (close === "closing" && card.state !== "closed") return { ...card, closing: true };
    return card;
  });
  return {
    open: settled.filter((c) => c.state !== "closed"),
    closed: settled.filter((c) => c.state === "closed"),
    pending: [...starts.values()].filter((s) => s.state === "starting"),
  };
}
