import { useEffect, useState } from "react";
import { api } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

/** What is said when the parent's title is not known (yet, or at all). */
export const PARENT_FALLBACK = "the parent chat";

/**
 * The title of a subagent's parent: from the person's rail rows when they hold it, else one lazy
 * `GET /api/threads/{id}` (a parent the rail has not loaded, or one that is archived). Never throws: a
 * parent that cannot be read is "the parent chat".
 */
export function useParentTitle(parent: string | undefined, rows: readonly ApiThread[]): string {
  const known = parent === undefined ? undefined : rows.find((r) => r.id === parent)?.title;
  const [fetched, setFetched] = useState<{ id: string; title: string } | null>(null);
  const need = parent !== undefined && known === undefined;
  useEffect(() => {
    if (!need || parent === undefined) return;
    let live = true;
    api
      .GET("/api/threads/{threadId}", { params: { path: { threadId: parent } } })
      .then(({ data }) => {
        if (live && data) setFetched({ id: parent, title: data.title });
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [need, parent]);
  if (known !== undefined && known.trim() !== "") return known;
  return fetched !== null && fetched.id === parent && fetched.title.trim() !== ""
    ? fetched.title
    : PARENT_FALLBACK;
}
