import { refreshPath } from "./session";

/*
 * A session that is about to end, or has, without the page being lost (web/README.md "Signing in
 * again"). Three things, all opt-in with the edge's sign-in (`NEXT_PUBLIC_SIGN_IN_PATH`):
 *
 *   1. keep it warm: while the app is open, the browser itself asks the edge about the session
 *      (`refreshPath()`) on an interval shorter than `--cookie-refresh` and when the window is
 *      back. oauth2-proxy refreshes a session older than that on such a request AND the renewed
 *      cookie reaches the browser, which it does not on the edge's own `forward_auth` subrequests
 *      (Caddy drops the subrequest's `Set-Cookie`);
 *   2. on a 401, refresh once and send the request again, the stream's reconnect and a run's POST
 *      included;
 *   3. when that finds no session, say so (`sessionStatus` is `ended`, a banner reads it) and hold
 *      the request until the person has signed in again, then send it once more.
 *
 * The state is one per page. `lib/api/client.ts` and the connect stream's client wrap their `fetch`
 * in `withSessionRefresh`; the shared page's reader client does not (ADR 0040).
 */

/** Shorter than oauth2-proxy's `--cookie-refresh` (10 minutes in the chart), so a refresh is never skipped. */
export const KEEP_WARM_MS = 4 * 60_000;
/** A window that comes back within this of the last proof of life is not asked again. */
export const FOCUS_GAP_MS = 60_000;
/** How long a request waits for a sign-in before its 401 is let through. */
export const PARK_MS = 10 * 60_000;
/** The channel the sign-in popup's last page calls on, and the tabs listen to. */
export const SIGNED_IN_CHANNEL = "another-agentic.signed-in";

export type SessionStatus = "ok" | "ended";
/** What the edge said: a session (`alive`), none (`gone`), or nothing that is a verdict (`unknown`). */
export type Ping = "alive" | "gone" | "unknown";

let status: SessionStatus = "ok";
const listeners = new Set<() => void>();

function setStatus(next: SessionStatus) {
  if (next === status) return;
  status = next;
  for (const listener of [...listeners]) listener();
}

/** `ended` while the edge has said there is no session and nobody has signed in since. */
export const sessionStatus = (): SessionStatus => status;

export function subscribeSession(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

let lastAlive = 0;
let inflight: Promise<Ping> | null = null;

/**
 * One question to the edge, by the browser, with its cookie. 2xx is a session, 401 (or a redirect to a
 * sign-in, which `manual` shows as an opaque answer) is none, and anything else (the edge is down, the
 * network is) says nothing about the session.
 */
export async function pingSession(): Promise<Ping> {
  const path = refreshPath();
  if (!path) return "unknown";
  try {
    const res = await globalThis.fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      redirect: "manual",
      headers: { Accept: "application/json" },
    });
    if (res.ok) return "alive";
    if (res.status === 401 || res.type === "opaqueredirect") return "gone";
    return "unknown";
  } catch {
    return "unknown";
  }
}

/**
 * Asks the edge, once however many ask at once, and takes the answer for what it says about the
 * page: a session clears `ended`, none sets it.
 */
export function renewSession(now: () => number = Date.now): Promise<Ping> {
  inflight ??= pingSession()
    .then((ping) => {
      if (ping === "alive") {
        lastAlive = now();
        setStatus("ok");
      } else if (ping === "gone") {
        setStatus("ended");
      }
      return ping;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

/** Resolves true once the status is `ok` again, false when `signal` aborts or `PARK_MS` passes. */
function untilSignedIn(signal: AbortSignal): Promise<boolean> {
  return new Promise((resolve) => {
    if (status === "ok") return resolve(true);
    if (signal.aborted) return resolve(false);
    const finish = (signedIn: boolean) => {
      off();
      clearTimeout(timer);
      signal.removeEventListener("abort", aborted);
      resolve(signedIn);
    };
    const aborted = () => finish(false);
    const off = subscribeSession(() => {
      if (status === "ok") finish(true);
    });
    const timer = setTimeout(() => finish(false), PARK_MS);
    signal.addEventListener("abort", aborted, { once: true });
  });
}

type Fetch = (request: Request) => Promise<Response>;

/**
 * A `fetch` that does not hand a 401 to its caller before the session has had its chance: refresh and
 * send the request again (once), and when there is no session to refresh, hold the request until the
 * person has signed in and send it again (once). A 401 that is still a 401 after that is the
 * caller's, as it always was, and so is any answer that is not a verdict on the session (the edge
 * could not be asked). Without a sign-in path it is `inner`.
 *
 * Safe for a POST: a 401 is the edge refusing the request before the orchestrator saw it, so a
 * request sent again is not a second one. The body is cloned before it is sent, never after.
 */
export function withSessionRefresh(inner: Fetch): Fetch {
  return async (request) => {
    if (!refreshPath()) return inner(request);
    const spare = request.body ? request.clone() : null;
    const again = () => (spare ? spare.clone() : request);

    const first = await inner(request);
    if (first.status !== 401) return first;

    if (status === "ok") {
      const ping = await renewSession();
      if (ping === "unknown") return first;
      if (ping === "alive") {
        const second = await inner(again());
        if (second.status !== 401) return second;
        // the edge knows the person and the orchestrator does not accept it: only a new sign-in is left
        setStatus("ended");
      }
    }
    // the person is asked (the banner reads `ended`); everything that needs the session waits for it
    if (!(await untilSignedIn(request.signal))) return first;
    return inner(again());
  };
}

/**
 * While the app is open, asks the edge about the session every `KEEP_WARM_MS` and when the window
 * comes back (focus, visible) after `FOCUS_GAP_MS`. Returns what stops it. Nothing happens
 * without a sign-in path.
 */
export function keepSessionWarm(now: () => number = Date.now): () => void {
  if (!refreshPath() || typeof window === "undefined") return () => {};
  lastAlive = Math.max(lastAlive, now());
  const ask = () => void renewSession(now);
  const back = () => {
    if (document.visibilityState === "hidden") return;
    if (now() - lastAlive >= FOCUS_GAP_MS) ask();
  };
  const timer = setInterval(ask, KEEP_WARM_MS);
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  return () => {
    clearInterval(timer);
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  };
}

/**
 * While the person is asked to sign in, listens for them to have done it: the popup's last page says
 * so on a channel, and a window that is back is asked. Neither is believed, both ask the edge.
 * Returns what stops it.
 */
export function watchForSignIn(): () => void {
  if (typeof window === "undefined") return () => {};
  const ask = () => void renewSession();
  const back = () => {
    if (document.visibilityState !== "hidden") ask();
  };
  let channel: BroadcastChannel | undefined;
  try {
    channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
    channel.onmessage = ask;
  } catch {
    // no channel (an old browser): the focus below is enough
  }
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  return () => {
    channel?.close();
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  };
}

/** Forgets the page's session state, for tests. */
export function resetSessionState() {
  status = "ok";
  lastAlive = 0;
  inflight = null;
}
