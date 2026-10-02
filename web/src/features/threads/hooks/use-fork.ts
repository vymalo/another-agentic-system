import { useRouter } from "next/navigation";
import { useCallback, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import { uuidv7 } from "@/lib/uuid";

/** The agent (and release) a fork talks to; the parent's when it is left out. */
export type ForkTarget = { agentId: string; release?: string | null };

/**
 * What to cut a thread at (`POST /api/threads/{id}/fork`, ADR 0029): `after` is an event of the
 * turn to copy ("fork from here", "continue with another agent"), `replace` a message of the
 * person to say again (an edit, a branch).
 */
export type ForkRequest =
  | { after: number; target?: ForkTarget }
  | { replace: number; text: string; messageId: string };

export type Forker = {
  /** A fork is being made. */
  busy: boolean;
  /** Why the last fork was refused, in words; null before any, and while the next is made. */
  error: string | null;
  clearError: () => void;
  /** Makes the fork and goes to it. Resolves `true` when it was made, `false` (with `error`) when not. */
  fork: (request: ForkRequest) => Promise<boolean>;
};

/** What `forkThread` answers with `code: turn_open`: the turn to copy is not over. */
export const TURN_OPEN_MESSAGE =
  "The agent is still working on this turn. Try again when it has finished.";

const bodyOf = (request: ForkRequest, id: string) =>
  "after" in request
    ? {
        id,
        after: request.after,
        ...(request.target
          ? {
              target: {
                agentId: request.target.agentId,
                ...(request.target.release ? { release: request.target.release } : {}),
              },
            }
          : {}),
      }
    : { id, replace: request.replace, text: request.text, messageId: request.messageId };

/**
 * Where an edit lands: its new message. The edit copies the events before the replaced message
 * (`forkedFrom.seq` is the last of them), then writes `thread_forked` and then the message, so the
 * message is the second event after the cut (ADR 0029). The page scrolls to it (`#m-<seq>`) and a
 * fork the server made some other way is simply opened at its end.
 */
const editLanding = (fork: { forkedFrom?: { seq: number } | undefined }): string =>
  fork.forkedFrom ? `#m-${fork.forkedFrom.seq + 2}` : "";

/**
 * Forking the open thread and going to the new one. The thread is the parent's id; the fork gets
 * an id of this page's choosing, which stays the same for a repeat of the same request (a
 * connection that dropped after the server made the fork): the server then answers the fork it
 * made and writes nothing, so a retry never makes a second one.
 */
export function useFork(parentId: string | null): Forker {
  const router = useRouter();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const working = useRef(false);
  const attempt = useRef<{ key: string; id: string } | null>(null);

  const fork = useCallback(
    async (request: ForkRequest): Promise<boolean> => {
      if (parentId === null || working.current) return false;
      working.current = true;
      setBusy(true);
      setError(null);
      const key = JSON.stringify(request);
      const id = attempt.current?.key === key ? attempt.current.id : uuidv7();
      attempt.current = { key, id };
      try {
        const { data, error: problem } = await api.POST("/api/threads/{threadId}/fork", {
          params: { path: { threadId: parentId } },
          body: bodyOf(request, id),
        });
        if (!data) {
          const code = (problem as { code?: string } | undefined)?.code;
          setError(code === "turn_open" ? TURN_OPEN_MESSAGE : problemMessage(problem));
          return false;
        }
        attempt.current = null;
        router.push(`/threads/${data.id}${"replace" in request ? editLanding(data) : ""}`);
        return true;
      } catch (e) {
        setError(problemMessage(e));
        return false;
      } finally {
        working.current = false;
        setBusy(false);
      }
    },
    [parentId, router],
  );

  return { busy, error, clearError: useCallback(() => setError(null), []), fork };
}
