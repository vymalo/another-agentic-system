import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/api/client";
import type { ApiThread } from "@/api/types";

const PAGE = 50;

export type ThreadsView = {
  threads: ApiThread[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
  refresh: () => void;
};

/**
 * The thread list (`GET /api/threads`). Refetched on window focus and whenever `refreshKey`
 * changes (the caller passes something that changes with the open thread's state).
 */
export function useThreads(refreshKey: string): ThreadsView {
  const [threads, setThreads] = useState<ApiThread[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const loadedPages = useRef(1);

  const refresh = useCallback(async () => {
    try {
      const { data, error: err } = await api.GET("/api/threads", {
        params: { query: { limit: PAGE } },
      });
      if (!data) {
        setError(problemMessage(err));
        return;
      }
      setError(null);
      // the newest page replaces what we had; older pages the user already loaded stay
      setThreads((prev) => {
        const fresh = new Set(data.map((t) => t.id));
        return [...data, ...prev.filter((t) => !fresh.has(t.id))];
      });
      setHasMore((h) => (loadedPages.current > 1 ? h : data.length === PAGE));
    } catch (e) {
      setError(problemMessage(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is the refetch trigger
  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

  useEffect(() => {
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  const loadMore = useCallback(async () => {
    const last = threads.at(-1);
    if (!last) return;
    const { data, error: err } = await api.GET("/api/threads", {
      params: { query: { limit: PAGE, before: last.id } },
    });
    if (!data) {
      setError(problemMessage(err));
      return;
    }
    loadedPages.current += 1;
    setThreads((prev) => [...prev, ...data.filter((t) => !prev.some((p) => p.id === t.id))]);
    setHasMore(data.length === PAGE);
  }, [threads]);

  return {
    threads,
    loading,
    error,
    hasMore,
    loadMore: () => void loadMore(),
    refresh: () => void refresh(),
  };
}
