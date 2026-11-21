import { useCallback, useState } from "react";
import { api } from "@/lib/api/client";
import { type Refused, refusedBy, unreached } from "./refused";

export type CloseResult = { ok: true; state: "closing" | "closed" } | Refused;

/**
 * `POST /api/threads/{child}/session/close` (`closeChildSession`): 202 says what the orchestrator's
 * record is after the request (`closing`, or `closed`), and the caller applies it at once; the outcome
 * is the child's `child_session_state` event. `cancel` stops the round that works first.
 */
export function useCloseSession() {
  const [busy, setBusy] = useState(false);
  const close = useCallback(async (child: string, cancel: boolean): Promise<CloseResult> => {
    setBusy(true);
    try {
      const { data, error, response } = await api.POST("/api/threads/{threadId}/session/close", {
        params: { path: { threadId: child } },
        body: { cancel },
      });
      if (response.status === 202) {
        return { ok: true, state: data?.state === "closed" ? "closed" : "closing" };
      }
      return refusedBy(error, response.status);
    } catch (e) {
      return unreached(e);
    } finally {
      setBusy(false);
    }
  }, []);
  return { close, busy };
}
