"use client";

import { useMemo } from "react";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { type AgentCommand, COMMANDS_URI, parseCommands } from "../lib/commands";

/**
 * The commands the current agent serves (ADR 0079, `docs/api/commands-v1.md` section 6), from the
 * capabilities document the screen already reads for the access tier and the tools: the same live
 * card, never cached (ADR 0008). The key is the signal — an agent whose card lists nothing, lists it
 * badly, or whose card could not be read, serves no command a client can offer, and this is empty:
 * the composer then draws no `/` menu of commands and nothing changes for the person.
 *
 * `capabilities` is the value `useAgentCapabilities` returns (the page reads the card once, for every
 * extension); `undefined` while the screen has none, which is no command.
 */
export function useCommands(capabilities: AgentCapabilities | undefined): readonly AgentCommand[] {
  const entry = capabilities?.status === "ready" ? capabilities.entry?.(COMMANDS_URI) : undefined;
  return useMemo(() => (entry === undefined ? [] : (parseCommands(entry) ?? [])), [entry]);
}
