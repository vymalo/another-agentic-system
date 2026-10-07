/*
 * The server's clock, as the page last heard it (ADR 0054, decision 5). The issuer allows seconds
 * of skew in a proof's `iat` and no nonce, so a proof is stamped with the server's time: the
 * offset is learned from the `Date` header of any answer (the orchestrator's, which the page can
 * always read; the issuer's only when it exposes the header across origins).
 */
let offsetMs = 0;

/** Learns the offset from a `Date` header value; anything that is not a date is ignored. */
export function observeDate(header: string | null | undefined, localNow: number = Date.now()) {
  if (!header) return;
  const server = Date.parse(header);
  if (Number.isFinite(server)) offsetMs = server - localNow;
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
