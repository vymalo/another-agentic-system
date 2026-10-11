import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import * as oauth from "oauth4webapi";
import { runtimeReady } from "@/lib/runtime-config";
import { authReady } from "./config";
import { CALLBACK_PATH, SIGNED_IN_CHANNEL } from "./constants";
import { authDb } from "./db";
import {
  isClaimed,
  isLoopback,
  loopbackAuthorize,
  loopbackRedirect,
  openInBrowser,
} from "./desktop";
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

/**
 * The only mobile OAuth redirect registered with the issuer and claimed by the Android and iOS apps.
 * This stays in code rather than runtime configuration: a deployment file chooses the public client and API
 * origin, never an arbitrary receiver for an authorization code.
 */
export const CLAIMED_CALLBACK_URI = "https://agentic.servers.segning.pro/mobile/auth/callback";

/** A deep-link event is useful only for the one HTTPS app/universal link this app claims. */
export function isClaimedCallback(value: string): boolean {
  try {
    const url = new URL(value);
    const claimed = new URL(CLAIMED_CALLBACK_URI);
    return (
      url.origin === claimed.origin &&
      url.pathname === claimed.pathname &&
      url.search !== "" &&
      url.hash === ""
    );
  } catch {
    return false;
  }
}

type ClaimedWaiter = {
  resolve: (value: Completed) => void;
  reject: (reason: unknown) => void;
  clear: () => void;
  cancel: () => void;
};

const claimedWaiters = new Map<string, ClaimedWaiter>();
let claimedListener: Promise<void> | undefined;
const CLAIMED_SIGN_IN_TIMEOUT_MS = 5 * 60_000;

async function receiveClaimedCallbacks(urls: string[]) {
  for (const callbackUrl of urls) {
    if (!isClaimedCallback(callbackUrl)) continue;
    const state = new URL(callbackUrl).searchParams.get("state");
    const waiter = state ? claimedWaiters.get(state) : undefined;
    try {
      const done = await completeSignIn(callbackUrl);
      waiter?.resolve(done);
      if (!waiter) {
        announceSignedIn();
        navigation.go(done.returnTo);
      }
    } catch (error) {
      waiter?.reject(error);
    } finally {
      if (state) claimedWaiters.delete(state);
    }
  }
}

/**
 * Starts the Tauri deep-link receiver once per page. It checks `getCurrent()` for a cold app launch and
 * `onOpenUrl()` for a running one. The plugin only delivers URLs that the OS handed to the claimed app;
 * `isClaimedCallback` then requires the registered HTTPS URI before any token exchange.
 */
export async function startClaimedCallbackListener(): Promise<void> {
  await runtimeReady();
  if (!isClaimed() || claimedListener) return claimedListener;
  claimedListener = (async () => {
    const unlisten = await onOpenUrl((urls) => void receiveClaimedCallbacks(urls));
    await receiveClaimedCallbacks((await getCurrent()) ?? []);
    // The app has one WebView for its whole lifetime. Keep its native event receiver alive with it.
    void unlisten;
  })();
  return claimedListener;
}

function waitForClaimedCallback(state: string): Promise<Completed> {
  return new Promise((resolve, reject) => {
    const clear = () => {
      clearTimeout(timeout);
      claimedWaiters.delete(state);
    };
    const cancel = () => {
      clear();
      reject(new Error("cancelled"));
    };
    const timeout = setTimeout(async () => {
      clear();
      await authDb().pending.delete(state);
      reject(new Error("The sign-in did not finish. Try again."));
    }, CLAIMED_SIGN_IN_TIMEOUT_MS);
    claimedWaiters.set(state, {
      resolve: (value) => {
        clear();
        resolve(value);
      },
      reject: (reason) => {
        clear();
        reject(reason);
      },
      clear,
      cancel,
    });
  });
}

/** Gives up a claimed-link sign-in after the person closes or abandons the native browser. */
export async function cancelClaimed(): Promise<void> {
  for (const [state, waiter] of claimedWaiters) {
    await authDb().pending.delete(state);
    waiter.cancel();
  }
}

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
 * Sends the browser (or the popup) to the issuer; resolves once it has been sent. In native apps the person's browser is
 * opened instead. Desktop receives its loopback URL; mobile receives only the fixed claimed HTTPS URL through Tauri's
 * deep-link plugin. Both exchange the code in this page and then load again at `returnTo` unless `stay`.
 */
export async function startSignIn(options: SignInOptions = {}): Promise<void> {
  const cfg = await configured();
  const as = await discover(cfg);
  if (!as.authorization_endpoint) throw new Error("The issuer has no authorization endpoint.");
  await dpopKeyPair();
  const loopback = isLoopback();
  const claimed = isClaimed();
  if (claimed) await startClaimedCallbackListener();
  const redirect = loopback
    ? await loopbackRedirect()
    : claimed
      ? CLAIMED_CALLBACK_URI
      : redirectUri();
  const state = oauth.generateRandomState();
  const verifier = oauth.generateRandomCodeVerifier();
  const challenge = await oauth.calculatePKCECodeChallenge(verifier);
  await prunePending();
  await authDb().pending.put({
    state,
    verifier,
    returnTo: safeReturnTo(options.returnTo ?? hereAsReturnTo()),
    mode: loopback ? "loopback" : claimed ? "claimed" : options.popup ? "popup" : "redirect",
    createdAt: Date.now(),
    ...(loopback || claimed ? { redirectUri: redirect } : {}),
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
  } else if (claimed) {
    const callback = waitForClaimedCallback(state);
    try {
      await openInBrowser(url.href);
    } catch (error) {
      claimedWaiters.get(state)?.clear();
      await authDb().pending.delete(state);
      throw error;
    }
    const done = await callback;
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
