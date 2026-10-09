/**
 * The event a link to a message names: `/threads/<id>#m-<seq>` (the versions of a message, an edit just sent, a shared
 * link). Read in the browser; undefined without the hash or on the server.
 */
export function linkedMessage(): number | undefined {
  if (typeof window === "undefined") return undefined;
  const seq = /^#m-(\d+)$/.exec(window.location.hash)?.[1];
  const n = seq === undefined ? undefined : Number(seq);
  return n !== undefined && Number.isSafeInteger(n) && n >= 1 ? n : undefined;
}
