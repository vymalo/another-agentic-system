import { useCallback, useEffect, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiAgent } from "@/lib/api/types";

export type AgentsView = {
  agents: ApiAgent[];
  loading: boolean;
  error: string | null;
  retry: () => void;
};

/**
 * `GET /api/agents`, read live every time the new-thread panel mounts: `releases` reflects the
 * agent's card right now and is never cached (ADR 0008).
 */
export function useAgents(enabled: boolean): AgentsView {
  const [agents, setAgents] = useState<ApiAgent[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-runs the fetch on retry
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    setLoading(true);
    api
      .GET("/api/agents")
      .then(({ data, error: err }) => {
        if (cancelled) return;
        if (data) {
          setAgents(data);
          setError(null);
        } else {
          setError(problemMessage(err));
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

  const retry = useCallback(() => setAttempt((a) => a + 1), []);
  return { agents, loading, error, retry };
}
