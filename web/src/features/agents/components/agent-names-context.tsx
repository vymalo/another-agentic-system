"use client";

import { createContext, type ReactNode, useContext, useMemo } from "react";
import type { ApiAgent } from "@/lib/api/types";

/*
 * The names of the agents the page lists (`GET /api/agents`, read live by `useAgents`) for the parts
 * of the chat that name an agent they were not given the name of: an ask is "Asked Coder", from the
 * agent's id in the log. Without a provider, or while the list is not back, the map is empty and an
 * agent is its id.
 */
const NONE: ReadonlyMap<string, string> = new Map();
const Context = createContext<ReadonlyMap<string, string>>(NONE);

export function AgentNamesProvider({
  agents,
  children,
}: {
  agents: readonly ApiAgent[];
  children: ReactNode;
}) {
  const names = useMemo(() => new Map(agents.map((a) => [a.id, a.name])), [agents]);
  return <Context.Provider value={names}>{children}</Context.Provider>;
}

/** The names by agent id; stable while the list is. */
export const useAgentNames = (): ReadonlyMap<string, string> => useContext(Context);
