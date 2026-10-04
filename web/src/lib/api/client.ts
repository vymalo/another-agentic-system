import createClient from "openapi-fetch";
import type { paths } from "./schema";
import { withSessionRefresh } from "./session-refresh";

/**
 * Relative base URL: every call goes to `/api/*` on the page's own origin. A 401 is a session that
 * has ended: the call refreshes it and goes again, or waits for the person to sign in again, when
 * the deployment has an edge to sign in at (session-refresh.ts); without one it is the 401 itself.
 */
export const api = createClient<paths>({
  baseUrl: "",
  fetch: withSessionRefresh((request) => globalThis.fetch(request)),
});

/**
 * The client of a shared page (ADR 0040): the same API, and a 401 is **not** sent to sign in. A
 * reader who is not signed in is first tried as a public reader, and only when the link is not
 * public is the browser sent to sign in (`features/sharing/lib/resolve.ts`).
 */
export const readerApi = createClient<paths>({ baseUrl: "" });

/** Best-effort human message from an RFC 9457 problem body or anything else. */
export function problemMessage(error: unknown): string {
  if (error && typeof error === "object") {
    const e = error as { detail?: unknown; title?: unknown };
    if (typeof e.detail === "string" && e.detail) return e.detail;
    if (typeof e.title === "string" && e.title) return e.title;
  }
  if (error instanceof Error && error.message) return error.message;
  return "Something went wrong. Try again.";
}
