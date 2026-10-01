import { type RefObject, useCallback, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

export type ThreadRenamer = {
  /** The title as typed so far while the person is renaming; null when they are not. */
  draft: string | null;
  saving: boolean;
  /** Why the last save failed (the server's reason); null before any and while typing again. */
  error: string | null;
  /** The title field while it is open: whoever closes the menu that started the edit gives it the focus. */
  field: RefObject<HTMLInputElement | null>;
  /** Starts renaming, from the title the thread has. */
  start: () => void;
  change: (text: string) => void;
  /** Gives the edit up (Escape): nothing is sent. */
  cancel: () => void;
  /** Sends the title (Enter, or leaving the field). The same or an empty one is no rename. */
  save: () => void;
};

/**
 * Renaming the open thread in its header (`PATCH /api/threads/{id}`, `patchThread`): a person's
 * title is final, so nothing else replaces it afterwards. The state belongs to the thread it was
 * started for, like the export's: the header keeps this component when another thread opens.
 *
 * The edit ends once, whichever way it ends: Enter, leaving the field and Escape can follow each
 * other (a removed field blurs), and only the first counts. A refused rename keeps the field and
 * says why, so the person can fix it or give it up.
 */
export function useRenameThread(
  thread: ApiThread | null,
  onRenamed: (thread: ApiThread) => void,
): ThreadRenamer {
  const threadId = thread?.id ?? null;
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The edit is over (saved, given up) or being saved: later calls of the same edit do nothing. */
  const settled = useRef(true);
  const field = useRef<HTMLInputElement | null>(null);
  const text = useRef("");
  const title = useRef("");
  title.current = thread?.title ?? "";

  const [seen, setSeen] = useState(threadId);
  if (seen !== threadId) {
    setSeen(threadId);
    setDraft(null);
    setSaving(false);
    setError(null);
    settled.current = true;
  }

  const start = useCallback(() => {
    if (threadId === null) return;
    settled.current = false;
    text.current = title.current;
    setDraft(title.current);
    setError(null);
  }, [threadId]);

  const change = useCallback((next: string) => {
    text.current = next;
    setDraft(next);
    setError(null);
  }, []);

  const cancel = useCallback(() => {
    settled.current = true;
    setDraft(null);
    setError(null);
  }, []);

  const save = useCallback(() => {
    if (threadId === null || settled.current) return;
    const next = text.current.trim();
    if (next === "" || next === title.current) return cancel();
    settled.current = true;
    setSaving(true);
    setError(null);
    api
      .PATCH("/api/threads/{threadId}", { params: { path: { threadId } }, body: { title: next } })
      .then(({ data, error: err }) => {
        if (data) {
          setDraft(null);
          onRenamed(data);
        } else {
          settled.current = false; // the field is still there: another try
          setError(problemMessage(err));
        }
      })
      .catch((e: unknown) => {
        settled.current = false;
        setError(problemMessage(e));
      })
      .finally(() => setSaving(false));
  }, [threadId, cancel, onRenamed]);

  return { draft, saving, error, field, start, change, cancel, save };
}
