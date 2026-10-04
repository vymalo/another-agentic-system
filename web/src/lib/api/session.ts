/*
 * A session that has expired: the orchestrator answers 401 (ADR 0033), and the edge (oauth2-proxy)
 * is where a person signs in again. All of it is opt-in: it needs the path of the edge's sign-in
 * in `NEXT_PUBLIC_SIGN_IN_PATH` (a build-time variable, Next inlines it: web/README.md "Signing in
 * again"). Without it a 401 is what it always was, an error line with the problem's words, which is
 * also what a deployment that has no such edge (a local run, the system e2e behind a proxy header)
 * wants. This file is where the edge is and how the page is left; `session-refresh.ts` is what a
 * 401 does first (refresh without leaving the page) and `features/session` is what the person sees.
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

/**
 * The edge's endpoint that answers 200 for a session and 401 for none, and that, called by the
 * browser itself, refreshes a session older than `--cookie-refresh` and sets the renewed cookie
 * (oauth2-proxy's `/oauth2/userinfo`): the sign-in's own prefix and `userinfo`, so a deployment
 * that moves the prefix (`--proxy-prefix`) moves both. Null where there is no sign-in to refresh.
 */
export function refreshPath(): string | null {
  const path = signInPath();
  if (!path) return null;
  const dir = path.split("?")[0]?.replace(/\/[^/]*$/, "") ?? "";
  return `${dir}/userinfo`;
}

/**
 * Where the sign-in popup ends (`app/signed-in`): a tiny page that tells the page that opened it and
 * closes itself. A path of this app, so it is `rd` of the sign-in like any other page.
 */
export const SIGNED_IN_PATH = "/signed-in";

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

/** The popup's window name and features: one sign-in window at a time, a size a sign-in form fits. */
const POPUP_NAME = "another-agentic-sign-in";
const POPUP_FEATURES = "popup=yes,width=520,height=720";

/**
 * Where the person signs in again, called from a click (a popup needs one). The page is kept: the
 * sign-in is opened in a popup that ends at `SIGNED_IN_PATH` and closes itself. Only when the
 * browser refuses the popup is the page left, for the sign-in with this page as `rd`
 * (`redirectToSignIn`, with its pause: a sign-in that does not help is not repeated at once).
 * Returns what happened: `none` (no sign-in is built in), `popup`, `redirect`, or `paused` (the
 * pause held the redirect back).
 */
export function openSignIn(now: number = Date.now()): "none" | "popup" | "redirect" | "paused" {
  const path = signInPath();
  if (!path || typeof window === "undefined") return "none";
  let popup: Window | null = null;
  try {
    // blank first, so the page that opened it can be cut off from the issuer's page that follows
    popup = window.open("", POPUP_NAME, POPUP_FEATURES);
  } catch {
    popup = null;
  }
  if (popup) {
    try {
      popup.opener = null;
    } catch {
      // a window that cannot be cut off is still the sign-in; the page hears of it by BroadcastChannel
    }
    popup.location.href = signInUrl(path, SIGNED_IN_PATH);
    return "popup";
  }
  return redirectToSignIn(now) ? "redirect" : "paused";
}

/** Forgets the pause, for tests. */
export function resetRedirectPause() {
  try {
    window.sessionStorage.removeItem(KEY);
  } catch {
    // nothing to forget
  }
}
