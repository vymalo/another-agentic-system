import { observeDate } from "./clock";
import type { BrowserAuthConfig } from "./types";

/*
 * Which kind of deployment this is, asked once per page load before the first API call (ADR 0054,
 * decision 7): `GET /api/public/auth` answers `{issuer, clientId, scope}` when the orchestrator has
 * the web sign in itself (browser mode), and 404 when it does not (edge mode: the cookie of
 * oauth2-proxy, as before). A network error is edge mode too.
 */

export const AUTH_CONFIG_PATH = "/api/public/auth";

let known: BrowserAuthConfig | null | undefined;
let asking: Promise<BrowserAuthConfig | null> | undefined;
const listeners = new Set<() => void>();

const isLoopback = (host: string) => ["localhost", "127.0.0.1", "[::1]"].includes(host);

/** An issuer is https, or http on the loopback address (a local run, the mock): nothing else is trusted with a code. */
export function isUsableIssuer(issuer: string): boolean {
  try {
    const url = new URL(issuer);
    return url.protocol === "https:" || (url.protocol === "http:" && isLoopback(url.hostname));
  } catch {
    return false;
  }
}

function parse(body: unknown): BrowserAuthConfig | null {
  if (typeof body !== "object" || body === null) return null;
  const { issuer, clientId, scope } = body as Record<string, unknown>;
  if (typeof issuer !== "string" || typeof clientId !== "string" || typeof scope !== "string") {
    return null;
  }
  if (!clientId || !isUsableIssuer(issuer)) return null;
  return { issuer, clientId, scope };
}

function settle(next: BrowserAuthConfig | null) {
  known = next;
  for (const listener of [...listeners]) listener();
  return next;
}

async function load(): Promise<BrowserAuthConfig | null> {
  try {
    const res = await globalThis.fetch(AUTH_CONFIG_PATH, {
      cache: "no-store",
      credentials: "omit",
      headers: { Accept: "application/json" },
    });
    observeDate(res.headers.get("Date"));
    if (res.status !== 200) {
      void res.body?.cancel();
      return settle(null);
    }
    return settle(parse(await res.json()));
  } catch {
    return settle(null);
  }
}

/** The answer, once it is in: the page's mode. Every call waits for the same single request. */
export function authReady(): Promise<BrowserAuthConfig | null> {
  if (known !== undefined) return Promise.resolve(known);
  asking ??= load();
  return asking;
}

/** The answer if the page has it: `undefined` while it is being asked, `null` in edge mode. */
export const browserAuth = (): BrowserAuthConfig | null | undefined => known;

export function subscribeAuthConfig(listener: () => void): () => void {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** Fixes the answer (a test, or a page that was told): `null` is edge mode, `undefined` asks again. */
export function setBrowserAuth(config: BrowserAuthConfig | null | undefined) {
  asking = undefined;
  if (config === undefined) {
    known = undefined;
    return;
  }
  settle(config);
}
