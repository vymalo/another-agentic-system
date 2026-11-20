import { useCallback, useState } from "react";
import type { Tier } from "@/features/tier/lib/tier";
import { api } from "@/lib/api/client";
import { type Refused, refusedBy, unreached } from "./refused";

/** What a person fills in to start a subagent: kept until the start has its outcome, as the contract asks of a client. */
export type StartDraft = {
  agent: string;
  description: string;
  text: string;
  /** What is sent: the session stays open after a round (the dialog's "Close when done" is off). */
  keepOpen: boolean;
};

export type StartResult = { ok: true } | Refused;

/**
 * `POST /api/threads/{id}/sessions` (`startChildSession`): the answer is 202, with the outcome to come
 * as events (`child_session_start_ended`); a refusal is decided before anything is written. The request's
 * `id` is the caller's, so that a repeat (after a refusal or a lost answer) is the same request.
 */
export function useStartSession(threadId: string) {
  const [busy, setBusy] = useState(false);
  const start = useCallback(
    async (request: string, draft: StartDraft, tier: Tier | undefined): Promise<StartResult> => {
      setBusy(true);
      try {
        const { error, response } = await api.POST("/api/threads/{threadId}/sessions", {
          params: { path: { threadId } },
          body: {
            id: request,
            agent: draft.agent,
            description: draft.description,
            text: draft.text,
            keepOpen: draft.keepOpen,
            ...(tier ? { tier } : {}),
          },
        });
        return response.status === 202 ? { ok: true } : refusedBy(error, response.status);
      } catch (e) {
        return unreached(e);
      } finally {
        setBusy(false);
      }
    },
    [threadId],
  );
  return { start, busy };
}
