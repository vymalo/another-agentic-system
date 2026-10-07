/*
 * The server's clock, as the page last heard it (ADR 0054, decision 5). The issuer allows seconds
 * of skew in a proof's `iat` and no nonce, so a proof is stamped with the server's time: the
 * offset is learned from the `Date` header of the orchestrator's answers (same origin, so readable:
 * `/api/public/auth` is the first) and from the `iat` of every access token the issuer hands out.
 * The issuer's own `Date` is never relied on.
 */
let offsetMs = 0;

/** Learns the offset from a `Date` header value; anything that is not a date is ignored. */
export function observeDate(header: string | null | undefined, localNow: number = Date.now()) {
  if (!header) return;
  const server = Date.parse(header);
  if (Number.isFinite(server)) offsetMs = server - localNow;
}

/**
 * Learns the offset from a claim the server wrote when it issued a token (`iat` of the access
 * token just received). A cross-origin answer's `Date` header is not readable by a page (it is not
 * CORS-safelisted, and an issuer need not expose it), so the token endpoint's own clock is heard
 * this way, before the next proof is made. It is a moment older than now by the answer's trip:
 * seconds of skew are allowed, a trip is not seconds.
 */
export function observeIssuedAt(iat: number | undefined, localNow: number = Date.now()) {
  if (typeof iat === "number" && Number.isFinite(iat)) offsetMs = iat * 1000 - localNow;
}

/** The server's time in whole seconds since the epoch. */
export function serverNowSeconds(localNow: number = Date.now()): number {
  return Math.floor((localNow + offsetMs) / 1000);
}

/** The offset in whole seconds, for `oauth4webapi`'s `clockSkew`. */
export const clockSkewSeconds = (): number => Math.round(offsetMs / 1000);

/** Forgets what was heard, for tests. */
export function resetClock() {
  offsetMs = 0;
}
