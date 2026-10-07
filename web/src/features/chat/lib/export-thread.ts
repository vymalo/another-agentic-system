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
 * The commit this web build was made from (`NEXT_PUBLIC_BUILD_REVISION`, which the image build sets and Next
 * inlines), or `undefined` for a build that was given none. It is what the file says about the web that saved it
 * (ADR 0053): the file is assembled by the server, so the revision goes with the request, as a header.
 */
export function webRevision(): string | undefined {
  const revision = process.env.NEXT_PUBLIC_BUILD_REVISION?.trim();
  return revision ? revision : undefined;
}

/**
 * `GET /api/threads/{id}/export` through the API client, like every other call of the app: the
 * whole thread (messages, agent statuses, artifacts, check, CI and verifier cards, reworks, the
 * job, and the builds that made it) as one JSON document. Throws an `Error` with a readable message when the server refuses.
 */
export async function fetchThreadExport(threadId: string): Promise<ThreadExportFile> {
  const revision = webRevision();
  const { data, error, response } = await api.GET("/api/threads/{threadId}/export", {
    params: { path: { threadId }, header: revision ? { "X-Web-Revision": revision } : {} },
    parseAs: "blob",
  });
  if (!data) throw new Error(problemMessage(error));
  return {
    blob: data,
    filename: exportFilename(threadId, response.headers.get("content-disposition")),
  };
}

/** How long the object URL of a download stays valid. */
export const REVOKE_AFTER_MS = 40_000;

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
  // The click only starts the download: Firefox and Safari abort a large one whose object URL is
  // revoked too early, so it lives as long as FileSaver.js keeps it (40 s).
  setTimeout(() => URL.revokeObjectURL(url), REVOKE_AFTER_MS);
}
