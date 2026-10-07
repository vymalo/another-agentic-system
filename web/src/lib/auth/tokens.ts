import * as oauth from "oauth4webapi";
import { observeIssuedAt } from "./clock";
import { authReady } from "./config";
import { PENDING_TTL_MS, REFRESH_LOCK, REFRESH_SKEW_MS } from "./constants";
import { authDb, authDbExists } from "./db";
import { storedKeyPair } from "./keys";
import { withLock } from "./lock";
import { clientOf, discover, isRefused, network, withNonceRetry } from "./oidc";
import type { BrowserAuthConfig, Claims, SessionRow } from "./types";

/*
 * The session the web holds (ADR 0054, decisions 3 and 4): the access token, spent on every
 * request; the refresh token, spent once by one tab. Everything is read from IndexedDB at the
 * moment it is needed, never kept in memory, because another tab may have changed it.
 */

/** No usable session: `none` is nobody signed in here, `refused` is a refresh token the issuer will not take. */
export class SessionEndedError extends Error {
  readonly reason: "none" | "refused";
  constructor(reason: "none" | "refused") {
    super(
      reason === "none" ? "Not signed in." : "The sign-in has ended: the issuer refused the token.",
    );
    this.name = "SessionEndedError";
    this.reason = reason;
  }
}

/** The issuer could not be asked (a network error, a 5xx): says nothing about the session. */
export class AuthUnavailableError extends Error {
  constructor(cause?: unknown) {
    super("The sign-in service could not be reached.", { cause });
    this.name = "AuthUnavailableError";
  }
}

export const sessionId = (cfg: BrowserAuthConfig): string => `${cfg.issuer} ${cfg.clientId}`;

async function readRow(cfg: BrowserAuthConfig): Promise<SessionRow | undefined> {
  return authDb().session.get(sessionId(cfg));
}

/** The payload of a JWT, read and not verified (what a name or a clock is taken from, nothing more). */
const payloadOf = (token: string): Record<string, unknown> | undefined => {
  try {
    const payload = token.split(".")[1];
    if (!payload) return undefined;
    const bytes = Uint8Array.from(atob(payload.replaceAll("-", "+").replaceAll("_", "/")), (c) =>
      c.charCodeAt(0),
    );
    const body: unknown = JSON.parse(new TextDecoder().decode(bytes));
    return typeof body === "object" && body !== null
      ? (body as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
};

const claimsOfPayload = (token: string): Claims | undefined => {
  const { sub, email } = payloadOf(token) ?? {};
  if (typeof sub !== "string" || !sub) return undefined;
  return { sub, ...(typeof email === "string" && email ? { email } : {}) };
};

/** The claims of a token response: the ID token's, else the access token's (read, never trusted for more than a name). */
export function claimsOf(result: oauth.TokenEndpointResponse): Claims | undefined {
  const id = oauth.getValidatedIdTokenClaims(result);
  if (id) {
    const email = id.email;
    return { sub: id.sub, ...(typeof email === "string" && email ? { email } : {}) };
  }
  return claimsOfPayload(result.access_token);
}

/** The row a token response makes; a refresh token that was not rotated stays. */
export function rowFrom(
  cfg: BrowserAuthConfig,
  result: oauth.TokenEndpointResponse,
  previous?: SessionRow,
): SessionRow {
  if (result.token_type !== "dpop") {
    throw new Error("The issuer did not bind the token to the browser's key (DPoP is required).");
  }
  // the server's clock at the moment it issued this token: the next proof is stamped with it
  const issuedAt = payloadOf(result.access_token)?.iat;
  observeIssuedAt(typeof issuedAt === "number" ? issuedAt : undefined);
  const claims = claimsOf(result) ?? previous?.claims;
  if (!claims) throw new Error("The token says nobody: it has no subject.");
  const refreshToken = result.refresh_token ?? previous?.refreshToken;
  return {
    id: sessionId(cfg),
    accessToken: result.access_token,
    expiresAt: Date.now() + (result.expires_in ?? 300) * 1000,
    ...(refreshToken ? { refreshToken } : {}),
    claims,
  };
}

/** Who the person is, for the page's person-switch check: the e-mail, else the subject, lower-cased. */
export const whoOf = (claims: Claims): string => (claims.email ?? claims.sub).trim().toLowerCase();

export type Held = { accessToken: string; claims: Claims };

const stale = (row: SessionRow, rejected: string | undefined, now: number): boolean =>
  row.expiresAt - now < REFRESH_SKEW_MS || (rejected !== undefined && row.accessToken === rejected);

/** The refresh token's grant; the row stays a tombstone when the issuer refuses it. */
async function spend(cfg: BrowserAuthConfig, row: SessionRow): Promise<SessionRow> {
  const keyPair = await storedKeyPair();
  if (!keyPair || !row.refreshToken) return end(row);
  const as = await discover(cfg);
  const client = clientOf(cfg);
  const DPoP = oauth.DPoP(client, keyPair);
  try {
    const result = await withNonceRetry(async () => {
      const res = await oauth.refreshTokenGrantRequest(
        as,
        client,
        oauth.None(),
        row.refreshToken as string,
        { DPoP, ...network(as.token_endpoint as string) },
      );
      return oauth.processRefreshTokenResponse(as, client, res);
    });
    const next = rowFrom(cfg, result, row);
    await authDb().session.put(next);
    return next;
  } catch (e) {
    if (isRefused(e)) return end(row);
    throw new AuthUnavailableError(e);
  }
}

/** The refresh token is dead: nothing of the session stays but the fact that there was one. */
async function end(row: SessionRow): Promise<never> {
  await authDb().session.put({
    id: row.id,
    accessToken: "",
    expiresAt: 0,
    claims: row.claims,
    ended: true,
  });
  throw new SessionEndedError("refused");
}

/**
 * A good access token, refreshing first when it has less than a minute left or the one the
 * orchestrator just refused (`rejected`). The refresh runs inside the Web Lock and reads the row
 * again there: with rotation on, two tabs that redeemed the same refresh token would end the
 * session, so the second finds what the first stored and spends nothing.
 */
export async function getAccessToken(options: { rejected?: string } = {}): Promise<Held> {
  const cfg = await authReady();
  if (!cfg) throw new SessionEndedError("none");
  const row = await readRow(cfg);
  if (!row) throw new SessionEndedError("none");
  if (row.ended) throw new SessionEndedError("refused");
  if (!stale(row, options.rejected, Date.now())) return row;
  return withLock(REFRESH_LOCK, async () => {
    const again = await readRow(cfg);
    if (!again) throw new SessionEndedError("none");
    if (again.ended) throw new SessionEndedError("refused");
    if (!stale(again, options.rejected, Date.now())) return again;
    return spend(cfg, again);
  });
}

/** Whether a sign-in is stored that could still be used, **without creating the database**. */
export async function hasUsableSession(): Promise<boolean> {
  const cfg = await authReady();
  if (!cfg || !(await authDbExists())) return false;
  const row = await readRow(cfg);
  return row !== undefined && row.ended !== true;
}

/** Forgets sign-ins that were never finished (decision 3: gone after ten minutes). */
export async function prunePending(now: number = Date.now()): Promise<void> {
  await authDb()
    .pending.where("createdAt")
    .below(now - PENDING_TTL_MS)
    .delete();
}
