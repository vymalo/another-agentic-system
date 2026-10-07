import { observeDate } from "./clock";
import { authReady } from "./config";
import { dpopProof } from "./dpop";
import { storedKeyPair } from "./keys";
import { hereAsReturnTo, startSignIn } from "./sign-in";
import { isSigningOut } from "./sign-out";
import {
  AuthUnavailableError,
  getAccessToken,
  hasUsableSession,
  SessionEndedError,
} from "./tokens";

/*
 * Every request to the orchestrator in browser mode (ADR 0054, decision 5): `Authorization: DPoP
 * <access token>` and a `DPoP` proof of the stored key for this request. In edge mode, and for the
 * public routes of a share link, the request is sent as it is: those never carry a token and never
 * open IndexedDB.
 */

export type Send = (request: Request) => Promise<Response>;
const plain: Send = (request) => globalThis.fetch(request);

/** The routes outside identity (ADR 0040): a reader of a public link has no token to send. */
export const isPublicRoute = (pathname: string): boolean =>
  pathname.startsWith("/api/public/") || pathname.startsWith("/agui/public/");

function targetsApi(request: Request): boolean {
  let url: URL;
  try {
    url = new URL(request.url);
  } catch {
    return false;
  }
  if (typeof window !== "undefined" && url.origin !== window.location.origin) return false;
  return (
    (url.pathname.startsWith("/api/") || url.pathname.startsWith("/agui/")) &&
    !isPublicRoute(url.pathname)
  );
}

/** What the orchestrator says about a 401: `invalid_token` (the token) or `invalid_dpop_proof` (the proof). */
export function dpopErrorOf(res: Response): string | undefined {
  const header = res.headers.get("WWW-Authenticate") ?? "";
  if (!/^DPoP\b/i.test(header)) return undefined;
  return /\berror="?([a-z_]+)"?/i.exec(header)?.[1];
}

/** A 401 made here, when there is no token to send: what the orchestrator would have said. */
export function unauthorized(): Response {
  return new Response(
    JSON.stringify({
      type: "about:blank",
      title: "Unauthorized",
      status: 401,
      detail: "sign in to continue",
    }),
    {
      status: 401,
      headers: {
        "Content-Type": "application/problem+json",
        "WWW-Authenticate": 'DPoP error="invalid_token"',
      },
    },
  );
}

/** The token the orchestrator last refused: the next refresh must not hand it out again. */
let rejected: string | undefined;
export const lastRejectedToken = (): string | undefined => rejected;
export const forgetRejectedToken = () => {
  rejected = undefined;
};

const never = <T>(): Promise<T> => new Promise<T>(() => {});
let signingIn = false;

/** Nobody is signed in here: the browser goes to the issuer, and the request waits for the page to leave. */
async function leaveForSignIn(): Promise<Response> {
  if (isSigningOut()) return never();
  if (!signingIn) {
    signingIn = true;
    try {
      await startSignIn({ returnTo: hereAsReturnTo() });
    } catch {
      signingIn = false;
      return unauthorized();
    }
  }
  return never();
}

export type AuthOptions = {
  /** Go to the issuer when nobody is signed in (the app); a share reader answers 401 instead. */
  redirect?: boolean;
};

export async function authenticatedFetch(
  request: Request,
  base: Send = plain,
  options: AuthOptions = {},
): Promise<Response> {
  const cfg = await authReady();
  if (!cfg || !targetsApi(request)) return base(request);
  if (isSigningOut()) return never();
  const spare = request.body ? request.clone() : null;
  let attempt: Request = request;
  for (let tries = 0; ; tries++) {
    let held: Awaited<ReturnType<typeof getAccessToken>>;
    try {
      held = await getAccessToken(rejected === undefined ? {} : { rejected });
    } catch (e) {
      if (e instanceof SessionEndedError) {
        if (e.reason === "none" && options.redirect !== false) return leaveForSignIn();
        return unauthorized();
      }
      if (e instanceof AuthUnavailableError) throw new TypeError(e.message, { cause: e });
      throw e;
    }
    const keyPair = await storedKeyPair();
    if (!keyPair) return unauthorized();
    const proof = await dpopProof(keyPair, {
      method: attempt.method,
      url: attempt.url,
      accessToken: held.accessToken,
    });
    const headers = new Headers(attempt.headers);
    headers.set("Authorization", `DPoP ${held.accessToken}`);
    headers.set("DPoP", proof);
    const res = await base(new Request(attempt, { headers }));
    observeDate(res.headers.get("Date"));
    if (res.status !== 401) return res;
    if (dpopErrorOf(res) === "invalid_dpop_proof" && tries === 0) {
      // the proof was refused (a clock that is out, learned from this very answer): once more, with a new one
      void res.body?.cancel();
      attempt = spare ? spare.clone() : request;
      continue;
    }
    rejected = held.accessToken;
    return res;
  }
}

/**
 * The reader of a share link, who is signed in first and a public reader after (ADR 0040): a
 * request to a signed-in route is sent with a token only when one is stored, and answers 401
 * without a word to anybody when none is, so that a reader who never signed in opens nothing.
 */
export async function readerFetch(request: Request, base: Send = plain): Promise<Response> {
  const cfg = await authReady();
  if (!cfg || !targetsApi(request)) return base(request);
  if (!(await hasUsableSession())) return unauthorized();
  return authenticatedFetch(request, base, { redirect: false });
}
