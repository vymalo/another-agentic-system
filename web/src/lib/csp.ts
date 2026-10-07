/*
 * The content security policy of every page of the web (ADR 0054, decision 10). A script that runs
 * in the page can use what the page holds, so what may run is bounded here: our own scripts, by a
 * per-request nonce and `strict-dynamic`; connections to this origin and to the issuer; images from
 * this origin, `data:` and `blob:` (the object URLs of kept files); nothing in a frame, no `<base>`,
 * no plugin. The issuer's origin is not known at build time, so it comes from the environment at
 * request time (`WEB_CSP_CONNECT_SRC`, a space-separated list, empty by default), which the chart sets.
 */

/** A source expression that is a scheme and a host and nothing a policy could be broken out of. */
const ORIGIN = /^(?:https?|wss?):\/\/(?:\*\.)?[a-z0-9.-]+(?::\d{1,5})?$/i;

/** The origins of `WEB_CSP_CONNECT_SRC`: whitespace-separated, anything that is not an origin is dropped. */
export function connectSources(raw: string | undefined): string[] {
  return (raw ?? "")
    .split(/\s+/)
    .map((token) => token.trim())
    .filter((token) => ORIGIN.test(token));
}

export type PolicyInput = {
  nonce: string;
  /** `WEB_CSP_CONNECT_SRC` */
  connect?: string | undefined;
  /** `next dev` needs `eval` for React's debugging information and a socket for hot reload. */
  dev?: boolean;
};

export function contentSecurityPolicy({ nonce, connect, dev = false }: PolicyInput): string {
  const origins = connectSources(connect);
  const web = origins.filter((o) => /^https?:/i.test(o));
  const directives = [
    "default-src 'self'",
    `script-src 'self' 'nonce-${nonce}' 'strict-dynamic'${dev ? " 'unsafe-eval'" : ""}`,
    "style-src 'self' 'unsafe-inline'",
    `connect-src ${["'self'", ...origins, ...(dev ? ["ws:", "wss:"] : [])].join(" ")}`,
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    `form-action ${["'self'", ...web].join(" ")}`,
    "object-src 'none'",
  ];
  return directives.join("; ");
}

/** A nonce: 16 random bytes, base64 (a fresh one for every request). */
export function makeNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
}
