/*
 * The content security policy of every page of the web (ADR 0054, decision 10; for the static export, ADR 0047,
 * Amendment 2026-10-09). A script that runs in the page can use what the page holds, so what may run is bounded:
 * our own files and the inline scripts of the build, by their SHA-256; connections to the API and to the issuer;
 * images from this origin, `data:` and `blob:` (the object URLs of kept files); nothing in a frame, no `<base>`,
 * no plugin.
 *
 * A static export has no server to make a nonce per request, so the policy is two halves that a browser enforces
 * together (CSP3: a resource must be allowed by every policy):
 *   * the **header**, sent by the static server (`Caddyfile`, `scripts/serve-static.ts`): everything but the
 *     hashes, with the issuer's origin read when the server starts (`WEB_CSP_CONNECT_SRC`). Its `script-src`
 *     allows `'unsafe-inline'`, which the other half takes back;
 *   * a **<meta>** written into every page after the build (`scripts/csp-meta.ts`): `script-src 'self'` and the
 *     hash of each inline script of that page. A hash in a policy voids its `'unsafe-inline'` (CSP2), and the
 *     header cannot allow what the meta does not.
 * The desktop app has no header: its own policy is Tauri's (`apps/tauri/src-tauri/tauri.conf.json`), beside the
 * same meta.
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

/**
 * The header half, for the given value of `WEB_CSP_CONNECT_SRC` (origins; or, for the `Caddyfile`, the
 * placeholder Caddy fills in when it starts).
 */
export function headerPolicy(connect: string[]): string {
  const web = connect.filter((o) => !/^wss?:/i.test(o));
  return [
    "default-src 'self'",
    "script-src 'self' 'unsafe-inline'",
    "style-src 'self' 'unsafe-inline'",
    `connect-src ${["'self'", ...connect].join(" ")}`,
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    "frame-ancestors 'none'",
    "base-uri 'none'",
    `form-action ${["'self'", ...web].join(" ")}`,
    "object-src 'none'",
  ].join("; ");
}

/** The meta half of a page: its own files and the hashes of its inline scripts (`'sha256-…'`, base64). */
export function metaPolicy(hashes: readonly string[]): string {
  const sources = ["'self'", ...[...new Set(hashes)].map((h) => `'sha256-${h}'`)];
  return [`script-src ${sources.join(" ")}`, "object-src 'none'", "base-uri 'none'"].join("; ");
}

/** The inline scripts of a page: the body of every `<script>` without a `src`, as the browser hashes it. */
export function inlineScripts(html: string): string[] {
  const found: string[] = [];
  for (const match of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
    const attributes = match[1] ?? "";
    if (/\ssrc\s*=/i.test(attributes)) continue;
    // a data block (`type="application/json"`) is not run, and needs no hash
    const type = /\stype\s*=\s*["']?([^"'\s>]+)/i.exec(attributes)?.[1]?.toLowerCase();
    if (type && !["text/javascript", "module", "application/javascript"].includes(type)) continue;
    found.push(match[2] ?? "");
  }
  return found;
}

/** The `<meta>` tag of a policy, to go first in `<head>` (before any script it governs). */
export const metaTag = (policy: string): string =>
  `<meta http-equiv="Content-Security-Policy" content="${policy.replaceAll('"', "&quot;")}">`;
