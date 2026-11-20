import { useCallback, useEffect, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiMyUsage } from "@/lib/api/types";

export type MyUsageState =
  | { status: "loading" }
  | { status: "ready"; usage: ApiMyUsage }
  /** `unavailable`: the orchestrator has no such route (404, one from before the ledger): a fact, not a failure. */
  | { status: "unavailable" }
  /** `signed-out`: a 401 that the app's own session handling did not take (a deployment with no sign-in built in). */
  | { status: "signed-out" }
  | { status: "error"; message: string };

const LOADING: MyUsageState = { status: "loading" };

/** What a failed read says: the problem's own words, else the status, never a page of HTML. */
function failure(error: unknown, status: number): string {
  if (error && typeof error === "object") return problemMessage(error);
  return `The usage could not be read (error ${status}). Try again.`;
}

/**
 * The caller's own usage in one UTC month (`GET /api/me/usage?month=`, ADR 0065). Never cached, as the route
 * is not (`no-store`): a month is read again every time it is asked for, and `reload` asks again. The state is
 * held with the month it answers, so on the render after the month changes it is `loading` and never the last
 * month's figures; a read that is overtaken by another month's is dropped, so a slow answer never replaces what
 * is on the screen; and an answer that names another month than the one asked is an error, not that month's data.
 */
export function useMyUsage(month: string): MyUsageState & { reload: () => void } {
  const [held, setHeld] = useState<{ asked: string; state: MyUsageState }>({
    asked: month,
    state: LOADING,
  });
  const [attempt, setAttempt] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt re-runs the read on reload
  useEffect(() => {
    let cancelled = false;
    const settle = (state: MyUsageState) => {
      if (!cancelled) setHeld({ asked: month, state });
    };
    setHeld({ asked: month, state: LOADING });
    api
      .GET("/api/me/usage", { params: { query: { month } } })
      .then(({ data, error, response }) => {
        if (data) {
          settle(
            data.month === month
              ? { status: "ready", usage: data }
              : { status: "error", message: "The answer was for another month. Try again." },
          );
        } else if (response.status === 404) settle({ status: "unavailable" });
        else if (response.status === 401) settle({ status: "signed-out" });
        else settle({ status: "error", message: failure(error, response.status) });
      })
      .catch((e: unknown) => settle({ status: "error", message: problemMessage(e) }));
    return () => {
      cancelled = true;
    };
  }, [month, attempt]);

  const reload = useCallback(() => setAttempt((a) => a + 1), []);
  return { ...(held.asked === month ? held.state : LOADING), reload };
}
