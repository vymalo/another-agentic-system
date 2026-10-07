import { apiFetch } from "@/lib/api/client";
import type { BrowserAuthConfig } from "@/lib/auth/types";

/*
 * How a kept file's bytes reach the page (ADR 0054, decision 9). With the edge's cookie a file is
 * a link: `<img src>` and `<a download>` carry it. Where the web holds its own tokens a link
 * cannot carry `Authorization` or a proof, so the page **fetches** the file, with DPoP, and shows an
 * object URL, revoked when its component goes. A `blob:` URL has the page's origin and none of the
 * server's headers, so it is only ever an image's `src` or a download's `href`, never a page to
 * navigate to: "open" shows an image inside the app, and downloads anything else. The routes of a
 * public share link carry no token and stay plain links.
 */

/** A file of a public share link: no token, so no fetch; a plain link as before. */
export const isPublicFile = (href: string): boolean => href.startsWith("/api/public/");

/** Whether this file has to be fetched rather than linked, for a deployment of this kind. */
export const mustFetch = (cfg: BrowserAuthConfig | null | undefined, href: string): boolean =>
  !!cfg && !isPublicFile(href);

/** The file's bytes through the signed-in session; throws when the API does not answer 200. */
export async function fetchFileBlob(href: string, signal?: AbortSignal): Promise<Blob> {
  const res = await apiFetch(href, {
    credentials: "same-origin",
    ...(signal ? { signal } : {}),
  });
  if (!res.ok) {
    void res.body?.cancel();
    throw new Error(`the file could not be read (${res.status})`);
  }
  return res.blob();
}

/** Saves bytes under a name, through an object URL that is revoked at once: a download, not a navigation. */
export function saveBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.rel = "noopener";
  link.hidden = true;
  document.body.append(link);
  link.click();
  link.remove();
  // the download has started by the time the task ends
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
