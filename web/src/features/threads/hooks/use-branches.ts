import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "@/lib/api/client";
import type { ApiBranchPoint } from "@/lib/api/types";

/** What the open thread knows of its family of edits (`GET /api/threads/{id}/branches`, ADR 0029). */
export type Branches = {
  /** The thread the conversation started from; the open thread itself until it is read. */
  root: string | null;
  /** The versions of the message at `seq` of this thread; undefined when it has none. */
  pointAt: (seq: number | undefined) => ApiBranchPoint | undefined;
};

export const NO_BRANCHES: Branches = { root: null, pointAt: () => undefined };

/**
 * The messages of the open thread that have other versions, read from the server: on opening, when
 * the window gets the focus back (another tab may have made an edit) and whenever `refreshKey`
 * changes (the caller passes something that moves with the thread's state). A thread that was
 * never edited has no points, and one that cannot be read has none either: the chat does not need
 * them to work, the picker is only absent. The last good answer stays while a refresh is made.
 */
export function useBranches(threadId: string | null, refreshKey = ""): Branches {
  const [read, setRead] = useState<{
    threadId: string;
    root: string;
    points: Map<number, ApiBranchPoint>;
  } | null>(null);

  const refresh = useCallback(async () => {
    if (threadId === null) return;
    try {
      const { data } = await api.GET("/api/threads/{threadId}/branches", {
        params: { path: { threadId } },
      });
      if (data) {
        setRead({ threadId, root: data.root, points: new Map(data.points.map((p) => [p.seq, p])) });
      }
    } catch {
      // the picker is absent, the chat is not affected
    }
  }, [threadId]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is the refetch trigger
  useEffect(() => {
    void refresh();
  }, [refresh, refreshKey]);

  useEffect(() => {
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  return useMemo(() => {
    const mine = read?.threadId === threadId ? read : null;
    return {
      root: mine?.root ?? threadId,
      pointAt: (seq) => (seq === undefined ? undefined : mine?.points.get(seq)),
    };
  }, [read, threadId]);
}
