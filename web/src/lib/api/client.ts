import createClient from "openapi-fetch";
import type { paths } from "./schema";
import { signInAgain } from "./session";

/** Relative base URL: every call goes to `/api/*` on the page's own origin. */
export const api = createClient<paths>({ baseUrl: "" });
// a 401 is an expired session: the edge's sign-in, when the deployment has one (session.ts)
api.use(signInAgain);

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
