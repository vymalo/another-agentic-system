import { authReady } from "@/lib/auth/config";

/*
 * Whether this browser has had a session, remembered (web/README.md "Share a conversation"). It is
 * only a hint about which route of a share link to ask first: a person with none is read by the
 * public route, so a visitor who never signed in does not meet a 401 from the signed-in one (a 401 the
 * edge answers for every request without a session). A wrong hint costs a request, never a verdict:
 * the page of a link falls back to the other route, and a 401 of the signed-in route forgets it.
 */

const KEY = "another-agentic.had-session";

/** True when this browser has been signed in. Storage that cannot be read says no. */
export function hadSession(): boolean {
  try {
    return window.localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

/** Remembers (or forgets) that this browser has a session. Storage that cannot be written is no loss. */
export function rememberSession(has: boolean): void {
  try {
    if (has) window.localStorage.setItem(KEY, "1");
    else window.localStorage.removeItem(KEY);
  } catch {
    // a private window: the hint is a convenience
  }
}

/**
 * Remembers a session, in edge mode only: where the web holds its own tokens (browser mode, ADR 0054) the
 * session is stored in IndexedDB and nothing of the sign-in is left in `localStorage`, which a test pins.
 * Forgetting is always allowed.
 */
export async function rememberEdgeSession(has: boolean): Promise<void> {
  if (!has) return rememberSession(false);
  try {
    if (!(await authReady())) rememberSession(true);
  } catch {
    // the hint is a convenience
  }
}

/** A `fetch` that remembers a session whenever the API answers a request with a success. */
export function withSessionHint(
  fetcher: (request: Request) => Promise<Response>,
): (request: Request) => Promise<Response> {
  return async (request) => {
    const response = await fetcher(request);
    if (response.ok) void rememberEdgeSession(true);
    return response;
  };
}
