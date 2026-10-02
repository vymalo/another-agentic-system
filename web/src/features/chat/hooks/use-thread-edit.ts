import { type RefObject, useCallback, useRef, useState } from "react";
import { api, problemMessage } from "@/lib/api/client";
import type { ApiThread } from "@/lib/api/types";

/** What a person writes about a thread (`PATCH /api/threads/{id}`, `patchThread`). */
export type ThreadTextMember = "title" | "description";

export type ThreadTextEditor = {
  /** The text as typed so far while the person is editing; null when they are not. */
  draft: string | null;
  saving: boolean;
  /** Why the last save failed (the server's reason); null before any and while typing again. */
  error: string | null;
  /** The field while it is open: whoever closes the menu that started the edit gives it the focus. */
  field: RefObject<HTMLInputElement | null>;
  /** Starts editing, from the text the thread has. */
  start: () => void;
  change: (text: string) => void;
  /** Gives the edit up (Escape): nothing is sent. */
  cancel: () => void;
  /** Sends the text (Enter, or leaving the field). The same text is no edit. */
  save: () => void;
};

type Options = {
  member: ThreadTextMember;
  /**
   * An empty text is an edit: it clears the member (a description). Else it is no edit at all (a
   * title is never empty).
   */
  allowEmpty: boolean;
};

/**
 * Editing one thing a person writes about the open thread in the header, a title or a
 * description (`PATCH /api/threads/{id}`): what a person writes is final, so nothing else replaces
 * it afterwards. The state belongs to the thread it was started for, like the export's: the header
 * keeps this component when another thread opens.
 *
 * The edit ends once, whichever way it ends: Enter, leaving the field and Escape can follow each
 * other (a removed field blurs), and only the first counts. A refused edit keeps the field and
 * says why, so the person can fix it or give it up.
 */
export function useThreadEdit(
  thread: ApiThread | null,
  onSaved: (thread: ApiThread) => void,
  { member, allowEmpty }: Options,
): ThreadTextEditor {
  const threadId = thread?.id ?? null;
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The edit is over (saved, given up) or being saved: later calls of the same edit do nothing. */
  const settled = useRef(true);
  const field = useRef<HTMLInputElement | null>(null);
  const text = useRef("");
  const current = useRef("");
  current.current = (member === "title" ? thread?.title : thread?.description) ?? "";

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
    text.current = current.current;
    setDraft(current.current);
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
    if ((next === "" && !allowEmpty) || next === current.current) return cancel();
    settled.current = true;
    setSaving(true);
    setError(null);
    api
      .PATCH("/api/threads/{threadId}", {
        params: { path: { threadId } },
        body: member === "title" ? { title: next } : { description: next },
      })
      .then(({ data, error: err }) => {
        if (data) {
          setDraft(null);
          onSaved(data);
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
  }, [threadId, member, allowEmpty, cancel, onSaved]);

  return { draft, saving, error, field, start, change, cancel, save };
}
