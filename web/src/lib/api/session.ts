import type { Middleware } from "openapi-fetch";

/*
 * A session that has expired: the orchestrator answers 401 (ADR 0033), and the edge (oauth2-proxy)
 * is where a person signs in again. The redirect is opt-in: it needs the path of the edge's
 * sign-in in `NEXT_PUBLIC_SIGN_IN_PATH` (a build-time variable, Next inlines it: web/README.md
 * "Signing in again"). Without it a 401 is what it always was, an error line with the problem's
 * words, which is also what a deployment that has no such edge (a local run, the system e2e behind
 * a proxy header) wants.
 */

/** How long a redirect is remembered: a sign-in that does not fix the 401 is not tried again at once. */
export const REDIRECT_PAUSE_MS = 30_000;
const KEY = "another-agentic.sign-in-redirect";

/**
 * The edge's sign-in path, or null when none is configured. A path of this origin only
 * (`/oauth2/sign_in`): anything else, a full URL or a `//host`, is a misconfiguration and counts as none.
 */
export function signInPath(): string | null {
  const raw = process.env.NEXT_PUBLIC_SIGN_IN_PATH?.trim();
  return raw && /^\/(?!\/)/.test(raw) ? raw : null;
}

/** Where to send the person: the sign-in with `rd`, the page to come back to (oauth2-proxy's `rd`). */
export function signInUrl(path: string, here: string): string {
  return `${path}${path.includes("?") ? "&" : "?"}rd=${encodeURIComponent(here)}`;
}

/** The one place the page is left; tests replace it, as jsdom cannot navigate. */
export const navigation = {
  go(url: string) {
    window.location.assign(url);
  },
};

function recentlyRedirected(now: number): boolean {
  try {
    const at = Number(window.sessionStorage.getItem(KEY));
    return Number.isFinite(at) && at > 0 && now - at < REDIRECT_PAUSE_MS;
  } catch {
    return false;
  }
}

/**
 * A 401 was answered. Sends the person to the edge's sign-in, coming back to this page, when that
 * is configured; returns whether it did. At most once in `REDIRECT_PAUSE_MS`: a sign-in that is
 * answered by another 401 is a deployment that is wrong, and a loop of redirects would hide it
 * where the error line says it.
 */
export function redirectToSignIn(now: number = Date.now()): boolean {
  const path = signInPath();
  if (!path || typeof window === "undefined") return false;
  if (recentlyRedirected(now)) return false;
  try {
    window.sessionStorage.setItem(KEY, String(now));
  } catch {
    // no storage: the pause cannot be kept, and a redirect that fails to fix the 401 may repeat
  }
  const { pathname, search, hash } = window.location;
  navigation.go(signInUrl(path, `${pathname}${search}${hash}`));
  return true;
}

/** For every client of the API: a 401 sends the person to sign in again. Never changes the response. */
export const signInAgain: Middleware = {
  onResponse({ response }) {
    if (response.status === 401) redirectToSignIn();
    return undefined;
  },
};

/** Forgets the pause, for tests. */
export function resetRedirectPause() {
  try {
    window.sessionStorage.removeItem(KEY);
  } catch {
    // nothing to forget
  }
}
