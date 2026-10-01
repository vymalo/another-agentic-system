import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

export type ThreadMeta = {
  /** `GET /api/threads/{id}`: title, target and the newest `lastSeq`; null until fetched. */
  thread: ApiThread | null;
  notFound: boolean;
  error: string | null;
  /** Fetch again soon (debounced): the conversation moved on. */
  refetchSoon: () => void;
  /** The thread as the server just said it (the answer to a rename), without another fetch. */
  apply: (thread: ApiThread) => void;
  reload: () => void;
};

const REFETCH_DEBOUNCE_MS = 150;

/**
 * The resource half of a thread (`GET /api/threads/{id}`). The conversation itself is AG-UI:
 * `ThreadAgent` follows it and the runtime renders it; this is only the metadata around it.
 */
export function useThreadMeta(threadId: string | null): ThreadMeta {
  const [thread, setThread] = useState<ApiThread | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const fetchThread = useCallback(async () => {
    if (threadId === null) return;
    const {
      data,
      error: err,
      response,
    } = await api.GET("/api/threads/{threadId}", { params: { path: { threadId } } });
    if (data) {
      setThread(data);
      setError(null);
      setNotFound(false);
    } else if (response.status === 404) {
      setNotFound(true);
    } else {
      setError(problemMessage(err));
    }
  }, [threadId]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: reloadKey re-runs the initial fetch
  useEffect(() => {
    void fetchThread().catch((e: unknown) => setError(problemMessage(e)));
    return () => clearTimeout(timer.current);
  }, [fetchThread, reloadKey]);

  const refetchSoon = useCallback(() => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      void fetchThread().catch(() => {});
    }, REFETCH_DEBOUNCE_MS);
  }, [fetchThread]);

  const apply = useCallback(
    (next: ApiThread) => {
      if (next.id === threadId) setThread(next);
    },
    [threadId],
  );

  return {
    thread,
    notFound,
    error,
    refetchSoon,
    apply,
    reload: () => setReloadKey((k) => k + 1),
  };
}
