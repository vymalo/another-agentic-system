"use client";

import { createContext, type ReactNode, useContext } from "react";
import { type Branches, NO_BRANCHES, useBranches } from "@/features/threads/hooks/use-branches";

const Context = createContext<Branches>(NO_BRANCHES);

/** The versions of the open thread's messages (ADR 0029); none outside a thread. */
export const useThreadBranches = (): Branches => useContext(Context);

/** Reads the open thread's branch points (`use-branches.ts`) for the chat and the thread list. */
export function BranchesProvider({
  threadId,
  refreshKey,
  children,
}: {
  threadId: string;
  /** Something that moves with the thread (its state): the points are read again when it does. */
  refreshKey: string;
  children: ReactNode;
}) {
  return <Context.Provider value={useBranches(threadId, refreshKey)}>{children}</Context.Provider>;
}
