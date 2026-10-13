import { useCallback, useEffect, useRef, useState } from "react";
import { arrangementFor, blocksOf, lastRoot, type RowAction } from "@/features/threads/lib/rail";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

const PAGE = 50;

/** The actions that are one `arrangeThread` call (Delete is `deleteThread`, its own flow). */
export type ArrangeAction = Exclude<RowAction, "delete">;

const VERB: Record<ArrangeAction, string> = {
  pin: "pin",
  unpin: "unpin",
  archive: "archive",
  unarchive: "unarchive",
  eject: "eject",
};

/** The archived section: read when it is opened (`archived=only`), forgotten when it is closed. */
export type ArchivedView = {
  open: boolean;
  toggle: () => void;
  threads: ApiThread[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
};

export type ThreadsView = {
  /** The list in the person's own order: each top-level thread followed by the ones nested under it. */
  threads: ApiThread[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  loadMore: () => void;
  refresh: () => void;
  archived: ArchivedView;
  /** One `arrangeThread` call, then the list is read again (the ranks are the server's); a refusal is in `notice`. */
  arrange: (thread: ApiThread, action: ArrangeAction) => Promise<void>;
  /** A thread was deleted: it goes from the list at once, and the list is read again for what took its place. */
  forget: (id: string) => void;
  /** Why the last arrangement was refused, in words; null when there is nothing to say. */
  notice: string | null;
  dismissNotice: () => void;
};

/** A page with no list of the person's own: a reader of a shared thread has none (ADR 0040). */
export const NO_THREADS: ThreadsView = {
  threads: [],
  loading: false,
  error: null,
  hasMore: false,
  loadMore: () => {},
  refresh: () => {},
  archived: {
    open: false,
    toggle: () => {},
    threads: [],
    loading: false,
    error: null,
    hasMore: false,
    loadMore: () => {},
  },
  arrange: async () => {},
  forget: () => {},
  notice: null,
  dismissNotice: () => {},
};

type Page = { rows: ApiThread[]; hasMore: boolean };
type Mode = "exclude" | "only";

const query = (mode: Mode, size: number, before?: string) => ({
  limit: size,
  order: "rail" as const,
  ...(mode === "only" ? { archived: "only" as const } : {}),
  ...(before ? { before } : {}),
});

async function readPage(mode: Mode, size: number, before?: string): Promise<Page> {
  const { data, error } = await api.GET("/api/threads", {
    params: { query: query(mode, size, before) },
  });
  if (!data) throw error ?? new Error("no list");
  // `limit` counts the threads of the list's own level, so a full page may have more after it
  return { rows: data, hasMore: blocksOf(data).length === size };
}

/**
 * One list read in pages (`order=rail`): the first page, then each next one after the last top-level
 * thread of the page before (a block is never split). Read again, it reads as many pages as the person
 * had loaded, so a refresh does not take them away. A read that a later one has overtaken is dropped.
 */
function usePages(mode: Mode, size: number, enabled: boolean, refreshKey: string) {
  const [rows, setRows] = useState<ApiThread[]>([]);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const pages = useRef(1);
  const latest = useRef(0);
  const held = useRef<ApiThread[]>([]);
  /** Read once since it was enabled: a later read does not bring "Loading" back over a list the person sees. */
  const settled = useRef(false);

  const keep = useCallback((next: ApiThread[]) => {
    held.current = next;
    setRows(next);
  }, []);

  const refresh = useCallback(async () => {
    const mine = ++latest.current;
    try {
      const all: ApiThread[] = [];
      let more = false;
      let read = 0;
      let before: string | undefined;
      while (read < pages.current) {
        const page = await readPage(mode, size, before);
        read += 1;
        all.push(...page.rows);
        more = page.hasMore;
        before = lastRoot(page.rows);
        if (!more || !before) break;
      }
      if (mine !== latest.current) return;
      pages.current = Math.max(1, read);
      keep(all);
      setHasMore(more);
      setError(null);
    } catch (e) {
      if (mine === latest.current) setError(problemMessage(e));
    } finally {
      if (mine === latest.current) {
        settled.current = true;
        setLoading(false);
      }
    }
  }, [mode, size, keep]);

  const loadMore = useCallback(async () => {
    const before = lastRoot(held.current);
    if (!before) return;
    const mine = ++latest.current;
    try {
      const page = await readPage(mode, size, before);
      if (mine !== latest.current) return;
      pages.current += 1;
      const seen = new Set(held.current.map((t) => t.id));
      keep([...held.current, ...page.rows.filter((t) => !seen.has(t.id))]);
      setHasMore(page.hasMore);
    } catch (e) {
      if (mine === latest.current) setError(problemMessage(e));
    }
  }, [mode, size, keep]);

  /** Takes a thread out at once; what was nested under it stands on its own until the next read. */
  const drop = useCallback(
    (id: string) => {
      keep(
        held.current
          .filter((t) => t.id !== id)
          .map((t) => {
            if (t.nestedUnder !== id) return t;
            const { nestedUnder: _gone, ...alone } = t;
            return alone;
          }),
      );
    },
    [keep],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey is the refetch trigger
  useEffect(() => {
    if (!enabled) {
      latest.current += 1;
      settled.current = false;
      pages.current = 1;
      keep([]);
      setHasMore(false);
      setError(null);
      setLoading(false);
      return;
    }
    if (!settled.current) setLoading(true);
    void refresh();
  }, [enabled, refresh, refreshKey, keep]);

  return { rows, loading, error, hasMore, refresh, loadMore, drop };
}

/** The `code` of a problem, where a client acts on one. */
const codeOf = (error: unknown): string | undefined =>
  typeof error === "object" && error !== null && "code" in error
    ? String((error as { code: unknown }).code)
    : undefined;

/**
 * The person's own list of threads (`GET /api/threads?order=rail`, ADR 0042; nobody lists another's,
 * ADR 0039) and what they do to it. The server owns the order, so every arrangement is one call and a
 * read; the archived ones are read only while their section is open. Refetched on window focus and
 * whenever `refreshKey` changes (the caller passes something that changes with the open thread's state).
 */
export function useThreads(refreshKey: string, options: { page?: number } = {}): ThreadsView {
  const size = options.page ?? PAGE;
  const [archivedOpen, setArchivedOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const main = usePages("exclude", size, true, refreshKey);
  const archived = usePages("only", size, archivedOpen, refreshKey);

  const { refresh: refreshMain } = main;
  const { refresh: refreshArchived } = archived;
  useEffect(() => {
    const onFocus = () => {
      void refreshMain();
      if (archivedOpen) void refreshArchived();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshMain, refreshArchived, archivedOpen]);

  const readAgain = useCallback(async () => {
    await Promise.all([refreshMain(), archivedOpen ? refreshArchived() : Promise.resolve()]);
  }, [refreshMain, refreshArchived, archivedOpen]);

  const arrange = useCallback(
    async (thread: ApiThread, action: ArrangeAction) => {
      const body = arrangementFor(action);
      if (!body) return;
      try {
        const { error, response } = await api.PATCH("/api/threads/{threadId}/rail", {
          params: { path: { threadId: thread.id } },
          body,
        });
        if (response.ok) setNotice(null);
        else if (response.status === 404)
          setNotice(null); // gone already: the read says so
        else if (
          response.status === 422 &&
          (codeOf(error) === "bad_anchor" || codeOf(error) === "nested_row")
        ) {
          setNotice("The list changed, so it was read again.");
        } else {
          setNotice(
            `Could not ${VERB[action]} “${thread.title || "Untitled"}”: ${problemMessage(error)}`,
          );
        }
      } catch (e) {
        setNotice(
          `Could not ${VERB[action]} “${thread.title || "Untitled"}”: ${problemMessage(e)}`,
        );
      }
      await readAgain();
    },
    [readAgain],
  );

  const { drop: dropMain } = main;
  const { drop: dropArchived } = archived;
  const forget = useCallback(
    (id: string) => {
      dropMain(id);
      dropArchived(id);
      void readAgain();
    },
    [dropMain, dropArchived, readAgain],
  );

  const { loadMore: loadMoreMain } = main;
  const { loadMore: loadMoreArchived } = archived;
  return {
    threads: main.rows,
    loading: main.loading,
    error: main.error,
    hasMore: main.hasMore,
    loadMore: () => void loadMoreMain(),
    refresh: () => void refreshMain(),
    archived: {
      open: archivedOpen,
      toggle: () => setArchivedOpen((open) => !open),
      threads: archived.rows,
      loading: archived.loading,
      error: archived.error,
      hasMore: archived.hasMore,
      loadMore: () => void loadMoreArchived(),
    },
    arrange,
    forget,
    notice,
    dismissNotice: () => setNotice(null),
  };
}
