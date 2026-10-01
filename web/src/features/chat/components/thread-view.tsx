"use client";

import { createContext, type ReactNode, useContext } from "react";
import type { ThreadState } from "@/lib/api/types";

/** What the transcript needs to know about the thread beyond its messages. */
export type ThreadView = {
  /** The thread's state; undefined until known. */
  state: ThreadState | undefined;
  /** The agent asked a question and waits for the answer (an interrupt is open). */
  waiting: boolean;
  /** The id of the agent the thread talks to (`coder`), for "Coder is starting…". */
  agentId: string | null;
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
