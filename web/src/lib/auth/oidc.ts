import * as oauth from "oauth4webapi";
import { clockSkewSeconds, observeDate } from "./clock";
import type { BrowserAuthConfig } from "./types";

/*
 * The glue to `oauth4webapi` (a library with no storage and no state of its own): discovery of the
 * issuer, the client, and a `fetch` that listens to the issuer's clock. Nothing here is kept
 * beyond the page: the metadata is read again on every page load, never cached in storage.
 */

/** `fetch` that learns the server's clock from every answer (the `Date` header, when it is readable). */
const tracedFetch: typeof fetch = async (input, init) => {
  const res = await globalThis.fetch(input, init);
  observeDate(res.headers.get("Date"));
  return res;
};

/** The options every request of the library takes: our `fetch`, and plain http only on loopback. */
export function network(url: string) {
  const insecure = new URL(url).protocol === "http:";
  return {
    [oauth.customFetch]: tracedFetch,
    ...(insecure ? { [oauth.allowInsecureRequests]: true as const } : {}),
  };
}

/** The client of this web: public, no secret, and the server's clock as far as it is known. */
export function clientOf(cfg: BrowserAuthConfig): oauth.Client {
  return { client_id: cfg.clientId, [oauth.clockSkew]: clockSkewSeconds() };
}

const found = new Map<string, Promise<oauth.AuthorizationServer>>();

/** The issuer's metadata (OpenID Connect discovery), read once per page. */
export function discover(cfg: BrowserAuthConfig): Promise<oauth.AuthorizationServer> {
  let asked = found.get(cfg.issuer);
  if (!asked) {
    const issuer = new URL(cfg.issuer);
    asked = oauth
      .discoveryRequest(issuer, { algorithm: "oidc", ...network(cfg.issuer) })
      .then((res) => oauth.processDiscoveryResponse(issuer, res));
    found.set(cfg.issuer, asked);
    asked.catch(() => found.delete(cfg.issuer));
  }
  return asked;
}

/** Forgets the metadata, for tests. */
export function resetDiscovery() {
  found.clear();
}

/** A DPoP nonce the issuer asks for is answered once (RFC 9449 section 8), with the same handle. */
export async function withNonceRetry<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (e) {
    if (oauth.isDPoPNonceError(e)) return run();
    throw e;
  }
}

/** The issuer refused the grant itself: the refresh token (or its key) is no good, whatever the network says. */
export const isRefused = (e: unknown): boolean =>
  e instanceof oauth.ResponseBodyError && e.error === "invalid_grant";
