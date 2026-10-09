"use client";

import { createContext, type MutableRefObject, useContext } from "react";

/*
 * The older turns of a thread that was opened at its end (ADR 0059): whether there are any, whether a page of them is being
 * read, and the place of the first turn in view, which the transcript keeps across the import that puts them in front.
 */

export type EarlierState = "idle" | "loading" | "waiting" | "error";

/** The place of the turn at the top of the viewport, noted before an import and put back after it. */
export type Anchor = { capture(): void };

export type EarlierControl = {
  /** The log has turns before the first one the transcript holds. */
  earlier: boolean;
  state: EarlierState;
  /** Why the last page could not be had; set with `state: "error"`. */
  error: string | null;
  /** Asks for the next older page (one at a time; a call while one is on its way does nothing). */
  load: () => void;
  /** Asks for every older page, one after the other, until the log's first event is held or one cannot be had. */
  loadAll: () => void;
  /** The thread was opened for a link to a message that the page could not reach: it shows the end of the thread. */
  anchorMissed: boolean;
  /** The transcript registers itself here. */
  anchor: MutableRefObject<Anchor | null>;
};

const EarlierContext = createContext<EarlierControl | null>(null);

export const EarlierProvider = EarlierContext.Provider;

/** The older turns' control; null where the transcript is not a window on a thread's end (a new chat, a replay). */
export function useEarlierControl(): EarlierControl | null {
  return useContext(EarlierContext);
}
