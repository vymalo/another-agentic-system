/*
 * Whether the app must ask the person to sign in before anything else (browser mode, ADR 0054): set
 * when the page finds nobody signed in here (`none`: never, or signed out in another tab) or a stored
 * sign-in the issuer has refused since (`ended`), and read by the sign-in screen (`SignInGate`), which
 * then stands in for the app. The page never leaves for the issuer by itself: the person presses
 * **Sign in**. A session that ends while the page is in use is the banner's (`session-refresh.ts`),
 * which keeps the page; this is for a page that has had no session yet.
 */

export type SignInNeed = "none" | "ended";

let need: SignInNeed | null = null;
const listeners = new Set<() => void>();

/** What the app needs: `null` while it has (or may have) a session. */
export const signInNeed = (): SignInNeed | null => need;

/** The app needs a sign-in; `ended` wins over `none` (the person had one, and is told so). */
export function requireSignIn(reason: SignInNeed) {
  if (need === reason || need === "ended") return;
  need = reason;
  for (const listener of [...listeners]) listener();
}

export function subscribeSignInNeed(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** Forgets the need, for tests. */
export function resetSignInNeed() {
  need = null;
  listeners.clear();
}
