/*
 * Which file of the static export answers an address (ADR 0047, decision 1). A thread and a share link are one
 * exported page each, `/threads/_` and `/s/_`, which read the id or the token from the address: any
 * `/threads/<id>` is that page, its HTML and the data Next fetches for it when the app navigates there (`.txt`,
 * and the per-segment files of a prefetch under `/threads/<id>/`). The web image's `Caddyfile` does the same
 * with its rewrites, and `static-server.test.ts` holds the two together; the desktop app maps its assets the
 * same way (`apps/tauri/src-tauri/src/assets.rs`). Every other address is itself.
 */

const SHELLS: ReadonlyArray<readonly [RegExp, string]> = [
  [/^\/threads\/[^/.]+$/, "/threads/_.html"],
  [/^\/threads\/[^/.]+\.txt$/, "/threads/_.txt"],
  [/^\/threads\/[^/.]+\/([^/]+\.txt)$/, "/threads/_/$1"],
  [/^\/s\/[^/.]+$/, "/s/_.html"],
  [/^\/s\/[^/.]+\.txt$/, "/s/_.txt"],
  [/^\/s\/[^/.]+\/([^/]+\.txt)$/, "/s/_/$1"],
];

/** The path of the file to try first for `pathname` (then `.html` and `/index.html` after it, as `try_files`). */
export function staticTarget(pathname: string): string {
  for (const [shape, target] of SHELLS) {
    const match = shape.exec(pathname);
    if (match) return target.replace("$1", match[1] ?? "");
  }
  return pathname;
}

/** The rewrites, for the test that compares them with the `Caddyfile`. */
export const STATIC_SHELLS = SHELLS;
