import { useCallback, useEffect, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

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
 * changes (the caller passes something that changes with the open thread's state). `all` lists
 * everyone's threads (`?owner=*`, an administrator's: each thread says its `owner`) instead of the
 * person's own; changing it starts the list over.
 */
export function useThreads(refreshKey: string, all = false): ThreadsView {
  const [threads, setThreads] = useState<ApiThread[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const loadedPages = useRef(1);
  /** Which list the answers in flight are for: one that arrives after the scope changed is dropped. */
  const listing = useRef(all);
  listing.current = all;

  const query = useCallback(
    (extra: { before?: string }) => ({
      limit: PAGE,
      ...extra,
      ...(all ? { owner: "*" } : {}),
    }),
    [all],
  );

  const refresh = useCallback(async () => {
    const asked = all;
    try {
      const { data, error: err } = await api.GET("/api/threads", {
        params: { query: query({}) },
      });
      if (listing.current !== asked) return;
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
      if (listing.current === asked) setError(problemMessage(e));
    } finally {
      if (listing.current === asked) setLoading(false);
    }
  }, [all, query]);

  // the other list is another list: nothing of the one before stays in it
  const shown = useRef(all);
  useEffect(() => {
    if (shown.current === all) return;
    shown.current = all;
    loadedPages.current = 1;
    setThreads([]);
    setHasMore(false);
    setError(null);
    setLoading(true);
  }, [all]);

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
      params: { query: query({ before: last.id }) },
    });
    if (!data) {
      setError(problemMessage(err));
      return;
    }
    loadedPages.current += 1;
    setThreads((prev) => [...prev, ...data.filter((t) => !prev.some((p) => p.id === t.id))]);
    setHasMore(data.length === PAGE);
  }, [threads, query]);

  return {
    threads,
    loading,
    error,
    hasMore,
    loadMore: () => void loadMore(),
    refresh: () => void refresh(),
  };
}
