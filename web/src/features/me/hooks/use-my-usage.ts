import { useCallback, useEffect, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiMyUsage } from "@/lib/api/types";

export type MyUsageState =
  | { status: "loading" }
  | { status: "ready"; usage: ApiMyUsage }
  /** `unavailable`: the orchestrator has no such route (404, one from before the ledger): a fact, not a failure. */
  | { status: "unavailable" }
  | { status: "error"; message: string };

/**
 * The caller's own usage in one UTC month (`GET /api/me/usage?month=`, ADR 0065). Never cached, as the route
 * is not (`no-store`): a month is read again every time it is asked for, and `reload` asks again. A read that
 * is overtaken by another month's is dropped, so a slow answer never replaces the month on the screen.
 */
export function useMyUsage(month: string): MyUsageState & { reload: () => void } {
  const [state, setState] = useState<MyUsageState>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-runs the read on reload
  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    api
      .GET("/api/me/usage", { params: { query: { month } } })
      .then(({ data, error, response }) => {
        if (cancelled) return;
        if (data) setState({ status: "ready", usage: data });
        else if (response.status === 404) setState({ status: "unavailable" });
        else setState({ status: "error", message: problemMessage(error) });
      })
      .catch((e: unknown) => {
        if (!cancelled) setState({ status: "error", message: problemMessage(e) });
      });
    return () => {
      cancelled = true;
    };
  }, [month, attempt]);

  const reload = useCallback(() => setAttempt((a) => a + 1), []);
  return { ...state, reload };
}
