import { useCallback, useMemo, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import { currentTools, MAX_ATTACHED, sameSet, sortedSet } from "../lib/servers";

export type ThreadTools = {
  /** The ids attached to the thread now, sorted: what the log says, or the person's last change until it does. */
  tools: string[];
  /** A change is on its way to the orchestrator. */
  busy: boolean;
  /** Why the last change was refused, in the orchestrator's words; null after one that was not. */
  error: string | null;
  /** Makes `servers` the whole set (`PUT /api/threads/{id}/tools`); resolves whether it was taken. */
  set: (servers: readonly string[]) => Promise<boolean>;
};

type Args = {
  /** null on the new-thread page: there is no thread to change, and `set` takes nothing. */
  threadId: string | null;
  /** `STATE_SNAPSHOT.thread.tools` of the newest group of the stream, and where the stream is. */
  stream: readonly string[] | undefined;
  streamSeq: number;
  /** `Thread.tools` and `Thread.lastSeq` of the last read of the resource, null until there is one. */
  fetched: readonly string[] | undefined;
  fetchedSeq: number | null;
  /** The conversation moved on: read the resource again soon. */
  refetchSoon: () => void;
};

/**
 * The servers attached to an open thread, and the one way to change them. Attaching is not a
 * message: it is `PUT` of the whole set, in any state of the thread, finished ones included (the
 * set applies to the next message). The truth is the log: the stream's snapshot says it when the
 * stream is open, and the resource when it is not (a finished thread's stream is paused), and
 * whichever has read further wins. The answer of the `PUT` stands in between, so a toggle shows at
 * once and does not flicker back while the log catches up (`currentTools`).
 */
export function useThreadTools({
  threadId,
  stream,
  streamSeq,
  fetched,
  fetchedSeq,
  refetchSoon,
}: Args): ThreadTools {
  const [put, setPut] = useState<{ servers: string[]; seq: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tools = useMemo(
    () => currentTools({ stream, streamSeq, fetched, fetchedSeq, put }),
    [stream, streamSeq, fetched, fetchedSeq, put],
  );

  const set = useCallback(
    async (servers: readonly string[]): Promise<boolean> => {
      if (threadId === null) return false;
      const wanted = sortedSet(servers);
      if (wanted.length > MAX_ATTACHED) {
        setError(`At most ${MAX_ATTACHED} servers can be attached to a chat.`);
        return false;
      }
      if (sameSet(wanted, tools)) return true;
      setBusy(true);
      setError(null);
      try {
        const { data, error: problem } = await api.PUT("/api/threads/{threadId}/tools", {
          params: { path: { threadId } },
          body: { servers: wanted },
        });
        if (!data) {
          setError(problemMessage(problem));
          return false;
        }
        setPut({ servers: data.servers, seq: Math.max(streamSeq, fetchedSeq ?? 0) });
        refetchSoon();
        return true;
      } catch (e: unknown) {
        setError(problemMessage(e));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [threadId, tools, streamSeq, fetchedSeq, refetchSoon],
  );

  return { tools, busy, error, set };
}
