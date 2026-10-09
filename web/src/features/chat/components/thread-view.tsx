"use client";

import { createContext, type ReactNode, useContext } from "react";
import type { KeptFile } from "@/features/chat/lib/files";
import type { ApiThread, ThreadState } from "@/lib/api/types";

/** What the transcript needs to know about the thread beyond its messages. */
export type ThreadView = {
  /** The thread's state; undefined until known. */
  state: ThreadState | undefined;
  /** The agent asked a question and waits for the answer (an interrupt is open). */
  waiting: boolean;
  /** The id of the agent the thread talks to (`adam`), for "Adam is starting…". */
  agentId: string | null;
  /**
   * The version of the UI catalog the thread has recorded (`STATE_SNAPSHOT.thread.uiCatalog`, ADR
   * 0023), when it has one: a surface that names a component this build lacks is "needs a newer
   * version of the app" when this is above the build's own, an error of the agent's otherwise.
   */
  catalogVersion?: number | undefined;
  /**
   * Where the thread was forked from, as the resource says it (ADR 0029): the parent's `threadId`
   * is gone once the parent is deleted, and then the divider is not a link.
   */
  forkedFrom?: ApiThread["forkedFrom"];
  /**
   * Agent turns before the first one the transcript holds, for a thread opened at its end (ADR 0059): the number of a turn is
   * its place among the turns held and this, so it does not change when older turns load.
   */
  turnsBefore?: number | undefined;
  /** The thread's kept files before the first turn the transcript holds: an `Image` may name them too. */
  carriedFiles?: readonly KeptFile[] | undefined;
};

const Context = createContext<ThreadView>({ state: undefined, waiting: false, agentId: null });

export function ThreadViewProvider({
  value,
  children,
}: {
  value: ThreadView;
  children: ReactNode;
}) {
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export const useThreadView = (): ThreadView => useContext(Context);
