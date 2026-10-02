"use client";

import { createContext, type ReactNode, useContext } from "react";
import type { ApiToolServer } from "@/lib/api/types";

/*
 * The deployment's servers (`GET /api/tool-servers`) for the parts of the chat that name a server
 * or draw its icon without being the picker: a step that is a call of `mcp-server:<id>` and the
 * line that says a server was attached. Read once for the chat by `useToolServers` (chat-shell.tsx);
 * without a provider, or while the list is not back, the list is empty and a server is its id with
 * the generic icon.
 */
const Context = createContext<readonly ApiToolServer[]>([]);

export function ToolServersProvider({
  servers,
  children,
}: {
  servers: readonly ApiToolServer[];
  children: ReactNode;
}) {
  return <Context.Provider value={servers}>{children}</Context.Provider>;
}

export const useToolServerList = (): readonly ApiToolServer[] => useContext(Context);

/** The server `id` names, when the deployment lists it. */
export const useToolServer = (id: string | undefined): ApiToolServer | undefined => {
  const servers = useContext(Context);
  return id === undefined ? undefined : servers.find((s) => s.id === id);
};
