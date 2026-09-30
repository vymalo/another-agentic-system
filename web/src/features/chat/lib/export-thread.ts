import { api, problemMessage } from "@/lib/api/client";

export type ThreadExportFile = { blob: Blob; filename: string };

/**
 * The name the server gave the file (`Content-Disposition: attachment; filename="thread-<id>.json"`),
 * else `thread-<id>.json`. Only a plain file name is taken from the header: anything with a path or
 * odd characters in it is ignored, because the browser would otherwise save what the server said.
 */
export function exportFilename(threadId: string, disposition: string | null): string {
  const named = disposition ? /filename="([A-Za-z0-9._-]{1,200})"/.exec(disposition)?.[1] : null;
  return named && !named.startsWith(".") ? named : `thread-${threadId}.json`;
}

/**
 * `GET /api/threads/{id}/export` through the API client, like every other call of the app: the
 * whole thread (messages, agent statuses, artifacts, check, CI and verifier cards, reworks, the
 * job) as one JSON document. Throws an `Error` with a readable message when the server refuses.
 */
export async function fetchThreadExport(threadId: string): Promise<ThreadExportFile> {
  const { data, error, response } = await api.GET("/api/threads/{threadId}/export", {
    params: { path: { threadId } },
    parseAs: "blob",
  });
  if (!data) throw new Error(problemMessage(error));
  return {
    blob: data,
    filename: exportFilename(threadId, response.headers.get("content-disposition")),
  };
}

/** Hands `file` to the browser as a download. */
export function saveFile({ blob, filename }: ThreadExportFile): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.rel = "noopener";
  link.hidden = true;
  document.body.append(link);
  link.click();
  link.remove();
  // The download has started by the time the click returns; give the browser a moment anyway.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
