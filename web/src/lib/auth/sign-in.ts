import * as oauth from "oauth4webapi";
import { authReady } from "./config";
import { CALLBACK_PATH, SIGNED_IN_CHANNEL } from "./constants";
import { authDb } from "./db";
import { isLoopback, loopbackAuthorize, loopbackRedirect } from "./desktop";
import { dpopKeyPair } from "./keys";
import { navigation } from "./navigation";
import { clientOf, discover, network, withNonceRetry } from "./oidc";
import { prunePending, rowFrom } from "./tokens";
import type { BrowserAuthConfig, PendingRow } from "./types";

/*
 * Signing in (ADR 0054, decision 2): Authorization Code with PKCE S256 and a state, a full-page
 * redirect to the issuer and back to `/auth/callback`, or the same flow in a popup that ends there
 * and tells the page over a BroadcastChannel. The callback exchanges the code with a DPoP proof.
 */

/** A page of this origin only: anything else (a URL, `//host`, `/\host`) is the start page. */
export function safeReturnTo(value: unknown): string {
  if (typeof value !== "string" || !/^\/(?![/\\])/.test(value)) return "/";
  // biome-ignore lint/suspicious/noControlCharactersInRegex: refusing control characters is the point
  return /[\u0000-\u001f\u007f]/.test(value) ? "/" : value;
}

export const redirectUri = (origin: string = window.location.origin): string =>
  `${origin}${CALLBACK_PATH}`;

/** The page the person is on, for the return trip. */
export const hereAsReturnTo = (): string =>
  safeReturnTo(`${window.location.pathname}${window.location.search}${window.location.hash}`);

async function configured(): Promise<BrowserAuthConfig> {
  const cfg = await authReady();
  if (!cfg) throw new Error("This deployment has no sign-in of its own.");
  return cfg;
}

export type SignInOptions = {
  returnTo?: string;
  /** A window opened by the click that asked: the sign-in goes there and the page stays. */
  popup?: Window;
  /**
   * The desktop app: keep this page once signed in (the banner's sign-in, whose held requests then go on). Without it the
   * page loads again at `returnTo`, as a browser's does after the callback: what asked for a session before there was
   * one waits for that load (`fetch.ts`, `signInFirst`).
   */
  stay?: boolean;
};

/**
 * Sends the browser (or the popup) to the issuer; resolves once it has been sent. In the desktop app (`signIn: loopback`,
 * ADR 0047) the person's browser is opened instead, the code comes back to this page, and this resolves once the sign-in
 * is finished: the other windows are told (the channel the banner listens to), and the page loads again at `returnTo`
 * unless `stay`.
 */
export async function startSignIn(options: SignInOptions = {}): Promise<void> {
  const cfg = await configured();
  const as = await discover(cfg);
  if (!as.authorization_endpoint) throw new Error("The issuer has no authorization endpoint.");
  await dpopKeyPair();
  const loopback = isLoopback();
  const redirect = loopback ? await loopbackRedirect() : redirectUri();
  const state = oauth.generateRandomState();
  const verifier = oauth.generateRandomCodeVerifier();
  const challenge = await oauth.calculatePKCECodeChallenge(verifier);
  await prunePending();
  await authDb().pending.put({
    state,
    verifier,
    returnTo: safeReturnTo(options.returnTo ?? hereAsReturnTo()),
    mode: loopback ? "loopback" : options.popup ? "popup" : "redirect",
    createdAt: Date.now(),
    ...(loopback ? { redirectUri: redirect } : {}),
  });
  const url = new URL(as.authorization_endpoint);
  url.searchParams.set("client_id", cfg.clientId);
  url.searchParams.set("response_type", "code");
  url.searchParams.set("redirect_uri", redirect);
  url.searchParams.set("scope", cfg.scope);
  url.searchParams.set("code_challenge", challenge);
  url.searchParams.set("code_challenge_method", "S256");
  url.searchParams.set("state", state);
  if (loopback) {
    const done = await completeSignIn(await loopbackAuthorize(url.href));
    announceSignedIn();
    if (!options.stay) navigation.go(done.returnTo);
  } else if (options.popup) options.popup.location.href = url.href;
  else navigation.go(url.href);
}

export type Completed = { returnTo: string; mode: PendingRow["mode"] };

/**
 * The callback page's work: the state says which sign-in this is (and is spent either way), the
 * code is exchanged with a proof of the stored key, and the session is stored. Throws with the
 * issuer's words on any refusal; nothing is stored then.
 */
export async function completeSignIn(callbackUrl: string): Promise<Completed> {
  const cfg = await configured();
  const url = new URL(callbackUrl);
  const state = url.searchParams.get("state");
  const pending = state ? await authDb().pending.get(state) : undefined;
  if (!state || !pending) throw new Error("This sign-in is unknown or has expired. Try again.");
  await authDb().pending.delete(state);
  if (Date.now() - pending.createdAt > 10 * 60_000) {
    throw new Error("This sign-in has expired. Try again.");
  }
  const as = await discover(cfg);
  const client = clientOf(cfg);
  const params = oauth.validateAuthResponse(as, client, url.searchParams, pending.state);
  const keyPair = await dpopKeyPair();
  const DPoP = oauth.DPoP(client, keyPair);
  const result = await withNonceRetry(async () => {
    const res = await oauth.authorizationCodeGrantRequest(
      as,
      client,
      oauth.None(),
      params,
      pending.redirectUri ?? redirectUri(url.origin),
      pending.verifier,
      { DPoP, ...network(as.token_endpoint as string) },
    );
    return oauth.processAuthorizationCodeResponse(as, client, res);
  });
  await authDb().session.put(rowFrom(cfg, result));
  return { returnTo: safeReturnTo(pending.returnTo), mode: pending.mode };
}

/** The popup's last act: tells the tabs that wait (they check the session, they do not take its word). */
export function announceSignedIn() {
  try {
    const channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
    channel.postMessage("signed-in");
    channel.close();
  } catch {
    // no channel: the waiting page asks again when its window is back
  }
}
