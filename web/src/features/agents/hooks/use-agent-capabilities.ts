import { useCallback, useEffect, useState } from "react";
import { api } from "@/lib/api/client";

/**
 * What the agent's card lists, as far as the screen could tell (`GET /agui/agents/{id}/capabilities`,
 * docs/api/agui.md "Capabilities document"). `custom` has one key for each extension of the
 * orchestrator's own that the live card lists, by exact URI (ADR 0008): the key is the signal. The
 * card is read on every request and never cached, so this reads it again whenever the agent
 * changes and whenever `refresh` is called (the picker opens). A card that cannot be read in time
 * gives the smaller document, with no `custom`: nothing is listed, and a client that wants an
 * extension fails closed.
 */
export type AgentCapabilities =
  /** No agent chosen, or the first read is not back. */
  | { status: "unknown"; supports: (uri: string) => null; refresh: () => void }
  /** The document could not be read (a network error, a 403, a 5xx): nothing can be said. */
  | { status: "unreadable"; supports: (uri: string) => null; refresh: () => void }
  | { status: "ready"; supports: (uri: string) => boolean; refresh: () => void };

const unknownFor = (refresh: () => void, status: "unknown" | "unreadable"): AgentCapabilities => ({
  status,
  supports: () => null,
  refresh,
});

export function useAgentCapabilities(agentId: string | null): AgentCapabilities {
  const [read, setRead] = useState<{
    agentId: string;
    custom: Record<string, unknown> | null;
  } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const refresh = useCallback(() => setAttempt((a) => a + 1), []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-reads the card on refresh
  useEffect(() => {
    if (agentId === null) return;
    let cancelled = false;
    api
      .GET("/agui/agents/{agentId}/capabilities", { params: { path: { agentId } } })
      .then(({ data }) => {
        if (cancelled) return;
        setRead(data ? { agentId, custom: data.custom ?? {} } : { agentId, custom: null });
      })
      .catch(() => {
        if (!cancelled) setRead({ agentId, custom: null });
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, attempt]);

  // an answer about another agent is no answer about this one
  if (agentId === null || read === null || read.agentId !== agentId) {
    return unknownFor(refresh, "unknown");
  }
  const { custom } = read;
  if (custom === null) return unknownFor(refresh, "unreadable");
  return { status: "ready", supports: (uri) => Object.hasOwn(custom, uri), refresh };
}
