/** What `GET /api/public/auth` says: the issuer the web signs in at (ADR 0054, decision 7). */
export type BrowserAuthConfig = {
  issuer: string;
  clientId: string;
  scope: string;
};

/** The claims the page needs and nothing more. */
export type Claims = { sub: string; email?: string };

/** `keys` table: the DPoP key pair, non-extractable, one per browser profile. */
export type KeyRow = { id: "dpop"; pair: CryptoKeyPair; createdAt: number };

/**
 * `session` table, one row per issuer and client. `ended` is a refused refresh token: the row stays
 * as a tombstone (no token in it) so that the page can tell "signed in once, and refused" (the
 * banner) from "never signed in" (the redirect).
 */
export type SessionRow = {
  id: string;
  accessToken: string;
  /** Local epoch milliseconds: when the access token stops being good. */
  expiresAt: number;
  refreshToken?: string;
  claims: Claims;
  ended?: true;
};

/** `pending` table: a sign-in under way, by its `state`. */
export type PendingRow = {
  state: string;
  verifier: string;
  returnTo: string;
  mode: "redirect" | "popup";
  createdAt: number;
};
