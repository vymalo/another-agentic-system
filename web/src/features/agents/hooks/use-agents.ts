import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiAgent } from "@/lib/api/types";

/** The sources of agents that could not be read, by name (ADR 0022): the platform's registry. */
export type RegistryView = { unreachable: string[] };

export type AgentsView = {
  agents: ApiAgent[];
  loading: boolean;
  error: string | null;
  /**
   * Which sources of agents could not be read on the last read. A source that cannot be read lists
   * none of its agents, so the list is incomplete and the picker says so; empty when every source
   * answered, and when the orchestrator does not say (an older one, or `GET /api/registry` failing:
   * the list is then shown as it is).
   */
  registry: RegistryView;
  /** Reads the list again now. */
  retry: () => void;
};

/**
 * A refresh because the window got the focus again is not made more often than this: the list
 * reads every agent's card, and switching tabs back and forth is not a reason to ask each time.
 * Opening the picker and the Retry buttons are not held back.
 */
const FOCUS_REFRESH_MS = 5_000;

const NONE: RegistryView = { unreachable: [] };

/**
 * `GET /api/agents` and `GET /api/registry`, read together and live: when the new-thread panel
 * mounts, when the picker is opened (`retry`), and when the window gets the focus again, so an agent
 * the platform added shows without a reload and a registry that came back is noticed. `releases`
 * reflects the agent's card right now and is never cached (ADR 0008); the registry's agents are the
 * registry's as of the last read (ADR 0022). A refresh keeps the list on screen while it runs.
 */
export function useAgents(enabled: boolean): AgentsView {
  const [agents, setAgents] = useState<ApiAgent[]>([]);
  const [registry, setRegistry] = useState<RegistryView>(NONE);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const loadedOnce = useRef(false);
  const lastRead = useRef(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-runs the fetch on retry
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    // the first read shows a skeleton; a refresh leaves the list as it is until the new one arrives
    if (!loadedOnce.current) setLoading(true);
    lastRead.current = Date.now();
    Promise.all([
      api.GET("/api/agents"),
      // the registry's state is a hint: failing to get it must not take the list with it
      api.GET("/api/registry").catch(() => undefined),
    ])
      .then(([list, status]) => {
        if (cancelled) return;
        if (list.data) {
          loadedOnce.current = true;
          setAgents(list.data);
          setError(null);
          setRegistry({
            unreachable: (status?.data?.sources ?? [])
              .filter((s) => s.status === "unavailable")
              .map((s) => s.name),
          });
        } else {
          setError(problemMessage(list.error));
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(problemMessage(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [enabled, attempt]);

  // coming back to the tab or the window reads the list again (throttled), so it is never stale
  useEffect(() => {
    if (!enabled) return;
    const refresh = () => {
      if (document.visibilityState === "hidden") return;
      if (Date.now() - lastRead.current < FOCUS_REFRESH_MS) return;
      setAttempt((a) => a + 1);
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [enabled]);

  const retry = useCallback(() => setAttempt((a) => a + 1), []);
  return { agents, loading, error, registry, retry };
}
