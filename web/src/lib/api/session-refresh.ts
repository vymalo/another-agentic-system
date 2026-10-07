import { authReady, browserAuth } from "@/lib/auth/config";
import { SIGNED_IN_CHANNEL } from "@/lib/auth/constants";
import { lastRejectedToken } from "@/lib/auth/fetch";
import { getAccessToken, SessionEndedError, whoOf as whoOfClaims } from "@/lib/auth/tokens";
import { navigation, REDIRECT_PAUSE_MS, refreshPath } from "./session";

export { SIGNED_IN_CHANNEL };

/*
 * A session that is about to end, or has, without the page being lost (web/README.md "Signing in
 * again"). Four things, all opt-in with the edge's sign-in (`NEXT_PUBLIC_SIGN_IN_PATH`):
 *
 *   1. keep it warm: while the app is open, visible and used, the browser itself asks the edge about
 *      the session (`refreshPath()`) on an interval shorter than `--cookie-refresh` and when the
 *      window is back. oauth2-proxy refreshes a session older than that on such a request AND the
 *      renewed cookie reaches the browser, which it does not on the edge's own `forward_auth`
 *      subrequests (Caddy drops the subrequest's `Set-Cookie`). Not for ever: every such refresh
 *      resets the issuer's idle timeout, so an unattended tab stops asking after `IDLE_LIMIT_MS`;
 *   2. on a 401, refresh and send the request again, the stream's reconnect and a run's POST
 *      included;
 *   3. when that finds no session, say so (`sessionStatus` is `ended`, a banner reads it) and hold
 *      the request until the person has signed in again, then send it once more. A request is thus
 *      sent up to THREE times: the original, after the refresh, and after being held;
 *   4. when the person who signed in is not the one who was signed in, send nothing: the held
 *      requests are rejected and the page is read again (`SessionChangedError`).
 *
 * The state is one per page. `lib/api/client.ts` and the connect stream's client wrap their `fetch`
 * in `withSessionRefresh`; the shared page's reader client does not (ADR 0040).
 *
 * Two kinds of deployment, told by `GET /api/public/auth` (ADR 0054). With an edge (above) the
 * session is oauth2-proxy's cookie and a "ping" asks the edge. In **browser mode** the web holds its
 * own tokens (`lib/auth`): a ping is a token refresh (`getAccessToken`: the refresh token is spent
 * once, in a Web Lock, only when the access token has under a minute left or the orchestrator
 * refused it), "gone" is a refresh token the issuer refused, and whose session it is comes from the
 * token's `email`, else its `sub`. The banner, the park, the three sends and `SessionChangedError`
 * are the same. Keeping warm is not a timer there: the access token lives five minutes and every
 * request refreshes it when it needs to, so the page only learns whose session it is at the start
 * and checks again when the window is back; a tab nobody looks at asks nothing, ever.
 */

/** Shorter than oauth2-proxy's `--cookie-refresh` (10 minutes in the chart), so a refresh is never skipped. */
export const KEEP_WARM_MS = 4 * 60_000;
/** A window that comes back within this of the last proof of life is not asked again. */
export const FOCUS_GAP_MS = 60_000;
/**
 * How long without a key, a pointer or a return to the window before the page stops keeping the
 * session warm. A trade-off, the deployment's to move: each keep-warm question after
 * `--cookie-refresh` is a refresh grant, which resets the issuer's SSO Session Idle, so a tab nobody
 * touches would otherwise hold a session open for ever. A stream's reconnect that meets a 401 still
 * refreshes (that is a person's work being followed, not an idle page).
 */
export const IDLE_LIMIT_MS = 30 * 60_000;
/** How long a request waits for a sign-in before its 401 is let through. */
export const PARK_MS = 10 * 60_000;
/** What counts as the person being there. */
const INPUT_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart"] as const;

/** `changed`: the session is another person's than the page's; the page is being read again. */
export type SessionStatus = "ok" | "ended" | "changed";
/**
 * What the edge said: a session (`alive`), none (`gone`), nothing that is a verdict (`unknown`), or a
 * session that is another person's than the one this page has been (`changed`).
 */
export type Ping = "alive" | "gone" | "unknown" | "changed";

/** Thrown to a request that was held for a person who then signed in as somebody else. */
export class SessionChangedError extends Error {
  constructor() {
    super("The session is now another person's. The page is being read again.");
    this.name = "SessionChangedError";
  }
}

let status: SessionStatus = "ok";
let signedBackAt = 0;
const listeners = new Set<() => void>();

function setStatus(next: SessionStatus) {
  if (next === status) return;
  if (status === "ended" && next === "ok") signedBackAt = Date.now();
  status = next;
  for (const listener of [...listeners]) listener();
}

/** `ended` while the edge has said there is no session and nobody has signed in since. */
export const sessionStatus = (): SessionStatus => status;

/** The session ended again soon after a sign-in: the sign-in did not help, whatever else is said. */
export const endedAgainSoon = (now: number = Date.now()): boolean =>
  status === "ended" && signedBackAt > 0 && now - signedBackAt < REDIRECT_PAUSE_MS;

export function subscribeSession(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

let lastAlive = 0;
let inflight: Promise<Ping> | null = null;
/** Who the edge said the session is, the first time and every time since: lower-cased, never shown. */
let who: string | null = null;
let reloading = false;

/**
 * One question to the edge, by the browser, with its cookie. 2xx is a session (and says whose),
 * 401 (or a redirect to a sign-in, which `manual` shows as an opaque answer) is none, and anything
 * else (the edge is down, the network is) says nothing about the session. The body is read or let go.
 */
export async function pingSession(): Promise<{ ping: Ping; who: string | null }> {
  if (browserAuth()) return pingTokens();
  const path = refreshPath();
  if (!path) return { ping: "unknown", who: null };
  try {
    const res = await globalThis.fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      redirect: "manual",
      headers: { Accept: "application/json" },
    });
    if (res.ok) return { ping: "alive", who: await whoOf(res) };
    void res.body?.cancel();
    if (res.status === 401 || res.type === "opaqueredirect") return { ping: "gone", who: null };
    return { ping: "unknown", who: null };
  } catch {
    return { ping: "unknown", who: null };
  }
}

/**
 * Browser mode's question: is there a session this page can use? A good access token is one (and
 * says whose); a refresh token the issuer refuses, or no sign-in at all, is none; an issuer that
 * cannot be reached says nothing. The token the orchestrator refused is never handed out again.
 */
async function pingTokens(): Promise<{ ping: Ping; who: string | null }> {
  try {
    const rejected = lastRejectedToken();
    const held = await getAccessToken(rejected === undefined ? {} : { rejected });
    return { ping: "alive", who: whoOfClaims(held.claims) };
  } catch (e) {
    return { ping: e instanceof SessionEndedError ? "gone" : "unknown", who: null };
  }
}

/** oauth2-proxy's userinfo says `email` and `user`; an answer that says neither is a session of nobody in particular. */
async function whoOf(res: Response): Promise<string | null> {
  try {
    const body: unknown = await res.json();
    if (typeof body !== "object" || body === null) return null;
    const { email, user } = body as { email?: unknown; user?: unknown };
    const name = typeof email === "string" && email ? email : typeof user === "string" ? user : "";
    return name.trim().toLowerCase() || null;
  } catch {
    return null;
  }
}

/**
 * Asks the edge, once however many ask at once, and takes the answer for what it says about the
 * page: a session clears `ended`, none sets it, and a session of another person than the page has
 * been ends in a reload (nothing held is sent as them). `fresh`: a question that starts after any
 * that is already on its way (a cookie that is set now was not there for the one that started before).
 */
export function renewSession(now: () => number = Date.now, fresh = false): Promise<Ping> {
  if (fresh && inflight) return inflight.then(() => renewSession(now));
  inflight ??= pingSession()
    .then(({ ping, who: asked }) => {
      if (ping === "alive") {
        if (who !== null && asked !== null && asked !== who) {
          switched();
          return "changed" as const;
        }
        if (asked !== null) who = asked;
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

/** The session is another person's: everything held is rejected, and the page is read again, once. */
function switched() {
  setStatus("changed");
  if (reloading) return;
  reloading = true;
  navigation.reload();
}

/** `back`: the status is `ok` again; `changed`: it is another person's; `gave-up`: aborted, or `PARK_MS` passed. */
function untilSignedIn(signal: AbortSignal): Promise<"back" | "changed" | "gave-up"> {
  return new Promise((resolve) => {
    if (status === "ok") return resolve("back");
    if (status === "changed") return resolve("changed");
    if (signal.aborted) return resolve("gave-up");
    const finish = (outcome: "back" | "changed" | "gave-up") => {
      off();
      clearTimeout(timer);
      signal.removeEventListener("abort", aborted);
      resolve(outcome);
    };
    const aborted = () => finish("gave-up");
    const off = subscribeSession(() => {
      if (status === "ok") finish("back");
      else if (status === "changed") finish("changed");
    });
    const timer = setTimeout(() => finish("gave-up"), PARK_MS);
    signal.addEventListener("abort", aborted, { once: true });
  });
}

type Fetch = (request: Request) => Promise<Response>;

/**
 * A `fetch` that does not hand a 401 to its caller before the session has had its chance. A request
 * is sent **up to three times**: the original; again after a refresh, when the edge has a session;
 * and again after being held, when it had none and the person has signed in. A 401 that is still a
 * 401 after that is the caller's, as it always was, and so is any answer that is not a verdict on
 * the session (the edge could not be asked). Without a sign-in path it is `inner`. If the person who
 * signed in is not the one who was, nothing held is sent: it is rejected (`SessionChangedError`).
 *
 * Why a request may be sent again, a POST included. A 401 means no handler ran: the edge's
 * `forward_auth` refuses before the request reaches the orchestrator, and the orchestrator's own 401
 * (an ID token that expired in between) is the identity layer's, which wraps every route and refuses
 * before a handler runs (`require_identity`, orchestrator/crates/api/src/auth.rs). No handler
 * answers 401 itself, and an agent's own 401 reaches the page as a 502
 * (orchestrator/crates/api/src/problem.rs), so a 401 never follows work that was done. The body is
 * cloned before the first send, never after.
 */
export function withSessionRefresh(inner: Fetch): Fetch {
  return async (request) => {
    await authReady();
    if (!browserAuth() && !refreshPath()) return inner(request);
    if (status === "changed") throw new SessionChangedError();
    const spare = request.body ? request.clone() : null;
    const again = () => (spare ? spare.clone() : request);

    let last = await inner(request);
    if (last.status !== 401) return last;

    if (status === "ok") {
      const ping = await renewSession();
      if (ping === "changed") throw new SessionChangedError();
      if (ping === "unknown") return last;
      if (ping === "alive") {
        void last.body?.cancel();
        last = await inner(again());
        if (last.status !== 401) return last;
        // the edge knows the person and the orchestrator does not accept it: only a new sign-in is left
        setStatus("ended");
      }
    }
    // the person is asked (the banner reads `ended`); everything that needs the session waits for it
    const outcome = await untilSignedIn(request.signal);
    if (outcome === "changed") throw new SessionChangedError();
    if (outcome === "gave-up") return last;
    void last.body?.cancel();
    return inner(again());
  };
}

/**
 * While the app is open, visible and used, asks the edge about the session every `KEEP_WARM_MS` and
 * when the window comes back (focus, visible) after `FOCUS_GAP_MS`; once at the start of the page, which is how
 * it learns whose session it is. Used: a key, a pointer or a return to the window within
 * `IDLE_LIMIT_MS`; a hidden or untouched page asks nothing, so it lets a session idle out.
 * Returns what stops it. Nothing happens without a sign-in path.
 */
export function keepSessionWarm(now: () => number = Date.now): () => void {
  if (typeof window === "undefined") return () => {};
  const mode = browserAuth();
  if (mode === undefined) {
    // the deployment's kind is not known yet: start when it is, unless the page is gone by then
    let stop = () => {};
    let live = true;
    void authReady().then(() => {
      if (live) stop = keepSessionWarm(now);
    });
    return () => {
      live = false;
      stop();
    };
  }
  if (mode) return keepTokensFresh(now);
  if (!refreshPath()) return () => {};
  let lastInput = now();
  const touch = () => {
    lastInput = now();
  };
  const warm = () => document.visibilityState !== "hidden" && now() - lastInput < IDLE_LIMIT_MS;
  const ask = () => void renewSession(now);
  const back = () => {
    if (document.visibilityState === "hidden") return;
    touch();
    if (now() - lastAlive >= FOCUS_GAP_MS) ask();
  };
  const tick = () => {
    if (warm()) ask();
  };
  // once per page load: the chat remounts on every navigation, and the page needs to learn whose session it is once
  if (who === null) ask();
  const timer = setInterval(tick, KEEP_WARM_MS);
  for (const name of INPUT_EVENTS) window.addEventListener(name, touch, { passive: true });
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  return () => {
    clearInterval(timer);
    for (const name of INPUT_EVENTS) window.removeEventListener(name, touch);
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  };
}

/**
 * Browser mode's keep-warm: once at the start of the page, which is how it learns whose session it
 * is, and when the window is back after `FOCUS_GAP_MS`. No timer: see the top of this file.
 */
function keepTokensFresh(now: () => number): () => void {
  const ask = () => void renewSession(now);
  const back = () => {
    if (document.visibilityState !== "hidden" && now() - lastAlive >= FOCUS_GAP_MS) ask();
  };
  if (who === null) ask();
  window.addEventListener("focus", back);
  document.addEventListener("visibilitychange", back);
  return () => {
    window.removeEventListener("focus", back);
    document.removeEventListener("visibilitychange", back);
  };
}

/**
 * While the person is asked to sign in, listens for them to have done it: the popup's last page says
 * so on a channel, and a window that is back is asked. Neither is believed, both ask the edge, and
 * the message asks anew (a question already on its way started before the cookie was set).
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
    channel.onmessage = () => void renewSession(Date.now, true);
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

/** Forgets the page's session state, and who is listening (a call still held by an earlier test), for tests. */
export function resetSessionState() {
  listeners.clear();
  status = "ok";
  lastAlive = 0;
  signedBackAt = 0;
  inflight = null;
  who = null;
  reloading = false;
}
