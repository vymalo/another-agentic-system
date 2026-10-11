import { parseTier, type Tier } from "./tier";

/**
 * The pick of a thread, kept in the browser (ADR 0061): a per-person convenience, never the
 * orchestrator's (it stores nothing between messages, and the tier is sent with each one). Every read and
 * write is guarded: storage can be blocked, full or absent (a private window, a preview), and the chip works
 * without it.
 */
const PREFIX = "vymalo.tier:";

const keyOf = (threadId: string) => `${PREFIX}${threadId}`;

/** The tier remembered for the thread, if any word of the four is. */
export function readPick(threadId: string): Tier | undefined {
  try {
    return parseTier(window.localStorage.getItem(keyOf(threadId)));
  } catch {
    return undefined;
  }
}

/** Remembers the pick of the thread, or forgets it (`undefined`). */
export function writePick(threadId: string, tier: Tier | undefined): void {
  try {
    if (tier === undefined) window.localStorage.removeItem(keyOf(threadId));
    else window.localStorage.setItem(keyOf(threadId), tier);
  } catch {
    // not remembered: the pick still holds for this page
  }
}
