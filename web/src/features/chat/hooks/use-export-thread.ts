import { useCallback, useState } from "react";
import {
  fetchThreadExport,
  fetchThreadMarkdown,
  saveFile,
} from "@/features/chat/lib/export-thread";
import { problemMessage } from "@/lib/api/client";

export type ExportFormat = "json" | "markdown";

export type ThreadExporter = {
  /** A download is being fetched (either format): both items of the menu wait. */
  exporting: boolean;
  /** Which one, for the item that says "Exporting…". */
  format: ExportFormat | null;
  /** Why the last export failed; null after a success or before any. */
  error: string | null;
  /** Export JSON. */
  run: () => void;
  /** Export Markdown (docs/api/thread-markdown-v1.md). */
  runMarkdown: () => void;
};

/**
 * Downloads one thread as a JSON file or as Markdown (`GET /api/threads/{id}/export`, both).
 *
 * The state belongs to the thread it was started for: the header keeps this component instance
 * when the person opens another thread, so a failure of the first must not show under the second,
 * neither one that was there when they moved nor one that arrives from a request still in flight.
 */
export function useExportThread(threadId: string | null): ThreadExporter {
  const [running, setRunning] = useState<{ threadId: string; format: ExportFormat } | null>(null);
  const [failure, setFailure] = useState<{ threadId: string; message: string } | null>(null);

  // Another thread starts clean (a failure of the one left is no news about this one). Adjusting
  // state while rendering, the way React documents it, rather than an effect one frame late.
  const [seen, setSeen] = useState(threadId);
  if (seen !== threadId) {
    setSeen(threadId);
    setFailure(null);
  }

  const start = useCallback(
    (format: ExportFormat) => {
      if (threadId === null) return;
      setRunning({ threadId, format });
      setFailure(null);
      (format === "json" ? fetchThreadExport : fetchThreadMarkdown)(threadId)
        .then(saveFile)
        .catch((e: unknown) => setFailure({ threadId, message: problemMessage(e) }))
        .finally(() => setRunning((now) => (now?.threadId === threadId ? null : now)));
    },
    [threadId],
  );
  const run = useCallback(() => start("json"), [start]);
  const runMarkdown = useCallback(() => start("markdown"), [start]);
  const mine = threadId !== null && running?.threadId === threadId ? running : null;

  return {
    exporting: mine !== null,
    format: mine?.format ?? null,
    error: failure !== null && failure.threadId === threadId ? failure.message : null,
    run,
    runMarkdown,
  };
}
