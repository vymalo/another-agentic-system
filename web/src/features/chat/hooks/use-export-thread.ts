import { useCallback, useState } from "react";
import { fetchThreadExport, saveFile } from "@/features/chat/lib/export-thread";
import { problemMessage } from "@/lib/api/client";

export type ThreadExporter = {
  exporting: boolean;
  /** Why the last export failed; null after a success or before any. */
  error: string | null;
  run: () => void;
};

/** Downloads one thread as a JSON file (`GET /api/threads/{id}/export`). */
export function useExportThread(threadId: string | null): ThreadExporter {
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(() => {
    if (threadId === null) return;
    setExporting(true);
    setError(null);
    fetchThreadExport(threadId)
      .then(saveFile)
      .catch((e: unknown) => setError(problemMessage(e)))
      .finally(() => setExporting(false));
  }, [threadId]);

  return { exporting, error, run };
}
