/*
 * The deployment's runtime configuration (ADR 0047, decision 1): `/config.json` next to the page, read once
 * per page load before the first request to the API, so that one build of the web (the image, the desktop
 * app) serves any deployment. Every key is optional; a file that is missing, unreadable or wrong is the web
 * on the edge's own origin, as before:
 *
 *   apiOrigin     where the API is, `https://chat.example.com` (the desktop and mobile apps, whose own
 *                 origin is `tauri://localhost`). Absent: the page's own origin.
 *   clientId      the public OAuth client this build signs in as (`another-agentic-desktop`), in place of the
 *                 one `GET /api/public/auth` names (the browser web's). The issuer and the scope stay the
 *                 orchestrator's: its tokens are what it verifies.
 *   signIn        `redirect` (the browser: a full-page redirect and back to `/auth/callback`) or `loopback`
 *                 (the desktop app: the system browser and a listener on 127.0.0.1, RFC 8252).
 *   organisation  the name the sign-in screen gives the account ("Sign in with your … account").
 */

export type SignInMode = "redirect" | "loopback";

export type RuntimeConfig = {
  apiOrigin?: string;
  clientId?: string;
  signIn?: SignInMode;
  organisation?: string;
};

export const RUNTIME_CONFIG_PATH = "/config.json";

const isLoopbackHost = (host: string) => ["localhost", "127.0.0.1", "[::1]"].includes(host);

/** An origin the page may send its tokens to: https, or http on this machine (a local run). */
export function isApiOrigin(value: unknown): value is string {
  if (typeof value !== "string") return false;
  try {
    const url = new URL(value);
    return (
      value === url.origin &&
      (url.protocol === "https:" || (url.protocol === "http:" && isLoopbackHost(url.hostname)))
    );
  } catch {
    return false;
  }
}

const isClientId = (value: unknown): value is string =>
  typeof value === "string" && /^[\x21-\x7e]{1,200}$/.test(value);

/** The keys of a parsed file that are right; the others are dropped. */
export function parseRuntimeConfig(body: unknown): RuntimeConfig {
  if (typeof body !== "object" || body === null || Array.isArray(body)) return {};
  const { apiOrigin, clientId, signIn, organisation } = body as Record<string, unknown>;
  return {
    ...(isApiOrigin(apiOrigin) ? { apiOrigin } : {}),
    ...(isClientId(clientId) ? { clientId } : {}),
    ...(signIn === "redirect" || signIn === "loopback" ? { signIn } : {}),
    ...(typeof organisation === "string" && organisation.trim() && organisation.length <= 120
      ? { organisation: organisation.trim() }
      : {}),
  };
}

let known: RuntimeConfig | undefined;
let asking: Promise<RuntimeConfig> | undefined;

async function load(): Promise<RuntimeConfig> {
  try {
    const res = await globalThis.fetch(RUNTIME_CONFIG_PATH, {
      cache: "no-store",
      credentials: "omit",
      headers: { Accept: "application/json" },
    });
    known = res.ok ? parseRuntimeConfig(await res.json()) : {};
  } catch {
    known = {};
  }
  return known;
}

/** The configuration, once it is in: every request to the API waits for the same single read. */
export function runtimeReady(): Promise<RuntimeConfig> {
  if (known !== undefined) return Promise.resolve(known);
  asking ??= load();
  return asking;
}

/** The configuration if it has been read, else the defaults. */
export const runtimeConfig = (): RuntimeConfig => known ?? {};

const pageOrigin = (): string | undefined =>
  typeof window === "undefined" ? undefined : window.location.origin;

/** The origin the API is at: the configured one, else the page's own. */
export const apiOrigin = (): string | undefined => runtimeConfig().apiOrigin ?? pageOrigin();

/** The API's routes: the resource API and AG-UI. */
export const isApiPath = (pathname: string): boolean =>
  pathname.startsWith("/api/") || pathname.startsWith("/agui/");

/** Whether a request the clients made with a path of this page goes to another origin (`atApi`). */
function elsewhere(request: Request): string | undefined {
  const configured = runtimeConfig().apiOrigin;
  const here = pageOrigin();
  if (!configured || !here || configured === here) return undefined;
  try {
    const url = new URL(request.url);
    if (url.origin !== here || !isApiPath(url.pathname)) return undefined;
    return `${configured}${url.pathname}${url.search}`;
  } catch {
    return undefined;
  }
}

/**
 * A request the clients made with a path of this page (`/api/...`, `/agui/...`), sent to the API's origin
 * when that is another one; any other request as it is. The body is read and sent again (a request whose
 * body is a stream cannot be given a new URL without it), and so are the headers and the signal.
 */
export async function atApi(request: Request): Promise<Request> {
  await runtimeReady();
  const target = elsewhere(request);
  if (!target) return request;
  const body = request.body ? await request.arrayBuffer() : undefined;
  return new Request(target, {
    method: request.method,
    headers: request.headers,
    signal: request.signal,
    cache: request.cache,
    redirect: request.redirect,
    credentials: "omit",
    ...(body === undefined ? {} : { body }),
  });
}

/** The URL of an API path, at the API's origin (`/api/public/auth`, a file's `href`). */
export function apiUrl(path: string): string {
  const configured = runtimeConfig().apiOrigin;
  return configured && path.startsWith("/") ? `${configured}${path}` : path;
}

/** Fixes the configuration (a test): `undefined` reads the file again. */
export function setRuntimeConfig(config: RuntimeConfig | undefined) {
  known = config;
  asking = undefined;
}
