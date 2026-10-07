import createClient from "openapi-fetch";
import { authenticatedFetch, readerFetch } from "@/lib/auth/fetch";
import type { paths } from "./schema";
import { withSessionRefresh } from "./session-refresh";

/**
 * Relative base URL: every call goes to `/api/*` on the page's own origin. A 401 is a session that
 * has ended: the call refreshes it and goes again, or waits for the person to sign in again, when
 * the deployment has an edge to sign in at (session-refresh.ts); without one it is the 401 itself.
 */
export const api = createClient<paths>({
  baseUrl: "",
  fetch: withSessionRefresh((request) => authenticatedFetch(request)),
});

const sessionFetch = withSessionRefresh((request) => authenticatedFetch(request));

/**
 * `fetch` for the routes of the signed-in app that are not called through `api` (a kept file's
 * bytes): the same session and the same DPoP proofs. Only for browser mode; edge mode links to
 * the file and lets the cookie do it.
 */
export const apiFetch: typeof fetch = (input, init) => sessionFetch(new Request(input, init));

/**
 * The client of a shared page (ADR 0040): the same API, and a 401 is **not** sent to sign in. A
 * reader who is not signed in is first tried as a public reader, and only when the link is not
 * public is the browser sent to sign in (`features/sharing/lib/resolve.ts`). In browser mode its
 * signed-in route carries DPoP like `api`, but only for a reader who has a stored session; the
 * public routes carry nothing and open nothing (ADR 0054).
 */
export const readerApi = createClient<paths>({
  baseUrl: "",
  // the signed-in route is sent with a token only when one is stored; the public routes never carry one
  fetch: (request) => readerFetch(request),
});

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
