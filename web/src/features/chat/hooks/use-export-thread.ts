import { useCallback, useState } from "react";
import { fetchThreadExport, saveFile } from "@/features/chat/lib/export-thread";
import { problemMessage } from "@/lib/api/client";

export type ThreadExporter = {
  exporting: boolean;
  /** Why the last export failed; null after a success or before any. */
  error: string | null;
  run: () => void;
};

/**
 * Downloads one thread as a JSON file (`GET /api/threads/{id}/export`).
 *
 * The state belongs to the thread it was started for: the header keeps this component instance
 * when the person opens another thread, so a failure of the first must not show under the second,
 * neither one that was there when they moved nor one that arrives from a request still in flight.
 */
export function useExportThread(threadId: string | null): ThreadExporter {
  const [running, setRunning] = useState<string | null>(null);
  const [failure, setFailure] = useState<{ threadId: string; message: string } | null>(null);

  // Another thread starts clean (a failure of the one left is no news about this one). Adjusting
  // state while rendering, the way React documents it, rather than an effect one frame late.
  const [seen, setSeen] = useState(threadId);
  if (seen !== threadId) {
    setSeen(threadId);
    setFailure(null);
  }

  const run = useCallback(() => {
    if (threadId === null) return;
    setRunning(threadId);
    setFailure(null);
    fetchThreadExport(threadId)
      .then(saveFile)
      .catch((e: unknown) => setFailure({ threadId, message: problemMessage(e) }))
      .finally(() => setRunning((now) => (now === threadId ? null : now)));
  }, [threadId]);

  return {
    exporting: threadId !== null && running === threadId,
    error: failure !== null && failure.threadId === threadId ? failure.message : null,
    run,
  };
}
