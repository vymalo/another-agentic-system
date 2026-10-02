import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiToolServer } from "@/lib/api/types";

export type ToolServersView = {
  /** The deployment's servers, in its order; empty until read, and when it offers none. */
  servers: ApiToolServer[];
  /** The first read is not back yet. */
  loading: boolean;
  /**
   * The list cannot be had for this person, and will not be: no role of theirs holds `thread.write`
   * (403), or the orchestrator has no such route (404, one from before attaching). The screen then
   * offers nothing, and says nothing: that is no failure.
   */
  unavailable: boolean;
  /** Why the last read failed (a network error, a 5xx); the list stays as it was. */
  error: string | null;
  /** Reads the list again now. */
  reload: () => void;
};

/**
 * `GET /api/tool-servers`, live: read when the chat mounts and again when the picker is opened
 * (`reload`), never cached ("Never cached", chat-api.yaml): a deployment that changed its servers
 * shows them without a reload of the page. A read that fails keeps the list on screen. The icons
 * in it are `data:` URIs the screen draws as they are; nothing in the list is ever fetched.
 */
export function useToolServers(enabled: boolean): ToolServersView {
  const [servers, setServers] = useState<ApiToolServer[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [unavailable, setUnavailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const loadedOnce = useRef(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-runs the read on reload
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    if (!loadedOnce.current) setLoading(true);
    api
      .GET("/api/tool-servers")
      .then(({ data, error: problem, response }) => {
        if (cancelled) return;
        if (data) {
          loadedOnce.current = true;
          setServers(data);
          setUnavailable(false);
          setError(null);
        } else if (response.status === 403 || response.status === 404) {
          setUnavailable(true);
          setServers([]);
        } else {
          setError(problemMessage(problem));
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

  const reload = useCallback(() => setAttempt((a) => a + 1), []);
  return { servers, loading, unavailable, error, reload };
}
