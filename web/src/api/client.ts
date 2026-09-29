import createClient from "openapi-fetch";
import type { paths } from "./schema";

/** Relative base URL: every call goes to `/api/*` on the page's own origin. */
export const api = createClient<paths>({ baseUrl: "" });

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
