import { authReady } from "./config";
import { authDb, authDbExists } from "./db";
import { isLoopback, openInBrowser } from "./desktop";
import { dpopProof } from "./dpop";
import { storedKeyPair } from "./keys";
import { navigation } from "./navigation";
import { discover } from "./oidc";
import { sessionId } from "./tokens";

/*
 * Signing out (ADR 0054, decision 11): the refresh token is revoked at the issuer (RFC 7009, with
 * a proof, when the issuer has the endpoint), the rows and the key are deleted, and the page goes
 * to the issuer's end-session endpoint, coming back to the start page.
 */

let leaving = false;
/** True from the moment a sign-out has begun: nothing else may start a sign-in meanwhile. */
export const isSigningOut = (): boolean => leaving;

async function revoke(token: string, endpoint: string, clientId: string) {
  const keyPair = await storedKeyPair();
  if (!keyPair) return;
  try {
    const proof = await dpopProof(keyPair, { method: "POST", url: endpoint });
    await globalThis.fetch(endpoint, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", DPoP: proof },
      body: new URLSearchParams({
        token,
        token_type_hint: "refresh_token",
        client_id: clientId,
      }),
      credentials: "omit",
      cache: "no-store",
    });
  } catch {
    // best effort: the rows are deleted whatever the issuer says
  }
}

export async function signOut(): Promise<void> {
  const cfg = await authReady();
  if (!cfg || leaving) return;
  leaving = true;
  let endSession: string | undefined;
  try {
    const as = await discover(cfg);
    const exists = await authDbExists();
    const row = exists ? await authDb().session.get(sessionId(cfg)) : undefined;
    if (row?.refreshToken && as.revocation_endpoint) {
      await revoke(row.refreshToken, as.revocation_endpoint, cfg.clientId);
    }
    if (typeof as.end_session_endpoint === "string") endSession = as.end_session_endpoint;
  } catch {
    // the issuer is not reachable: the person is still signed out here
  }
  if (await authDbExists()) {
    const db = authDb();
    await db.transaction("rw", db.keys, db.session, db.pending, async () => {
      await db.keys.clear();
      await db.session.clear();
      await db.pending.clear();
    });
  }
  const home = `${window.location.origin}/`;
  if (endSession && isLoopback()) {
    // the desktop app: the issuer's page opens in the person's browser, never in the app, and comes back nowhere
    const url = new URL(endSession);
    url.searchParams.set("client_id", cfg.clientId);
    await openInBrowser(url.href).catch(() => {});
    navigation.go("/");
  } else if (endSession) {
    const url = new URL(endSession);
    url.searchParams.set("client_id", cfg.clientId);
    url.searchParams.set("post_logout_redirect_uri", home);
    navigation.go(url.href);
  } else {
    navigation.go("/");
  }
}
