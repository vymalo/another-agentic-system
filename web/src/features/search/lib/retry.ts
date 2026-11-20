/** The longest wait the web honours for a `Retry-After`, in seconds. */
export const MAX_WAIT_S = 30;

/**
 * Seconds to wait from a `Retry-After` header: the contract says seconds, and an HTTP-date (what a proxy may rewrite it to) is read
 * too. Anything else is one second; never less than one, never more than {@link MAX_WAIT_S}.
 */
export function retryAfterSeconds(header: string | null, now: number): number {
  const text = (header ?? "").trim();
  let seconds = 1;
  if (/^\d+$/.test(text)) seconds = Number.parseInt(text, 10);
  else if (text !== "") {
    const at = Date.parse(text);
    if (Number.isFinite(at)) seconds = Math.ceil((at - now) / 1000);
  }
  return Math.min(MAX_WAIT_S, Math.max(1, seconds));
}
