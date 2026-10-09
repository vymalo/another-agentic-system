/*
 * The web image's server (`Caddyfile`) for the e2e and the system tests, where there is no Caddy: the static export, the same
 * rewrites, headers and content security policy, and, like the edge of a deployment, `/api/*`, `/agui/*` (and with `--edge`
 * the edge's own `/oauth2/*`) forwarded to the API on the same origin. `src/lib/static-server.test.ts` holds it and the
 * `Caddyfile` to the same routes and headers.
 *
 *   tsx scripts/serve-static.ts --dir out --port 3000 [--api http://127.0.0.1:4010] [--edge] [--connect <origins>]
 *
 * `--connect` (else `WEB_CSP_CONNECT_SRC`) is what the policy lets the page connect to besides itself: the issuer.
 */
import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { connectSources, headerPolicy } from "../src/lib/csp";
import { staticTarget } from "../src/lib/static-routes";

function flag(name: string): string | undefined {
  const at = process.argv.indexOf(`--${name}`);
  return at >= 0 ? process.argv[at + 1] : undefined;
}

const dir = path.resolve(flag("dir") ?? process.env.NEXT_DIST_DIR ?? "out");
const port = Number(flag("port") ?? 3000);
const api = flag("api");
const edge = process.argv.includes("--edge");
const policy = headerPolicy(connectSources(flag("connect") ?? process.env.WEB_CSP_CONNECT_SRC));

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".webmanifest": "application/manifest+json",
  ".txt": "text/plain; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".map": "application/json",
};

function headers(pathname: string): Record<string, string> {
  return {
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "same-origin",
    "Content-Security-Policy": policy,
    ...(pathname === "/config.json" ? { "Cache-Control": "no-cache" } : {}),
    ...(pathname.startsWith("/_next/static/")
      ? { "Cache-Control": "public, max-age=31536000, immutable" }
      : {}),
  };
}

async function file(relative: string): Promise<string | undefined> {
  const full = path.join(dir, relative);
  if (!full.startsWith(dir + path.sep)) return undefined;
  try {
    return (await stat(full)).isFile() ? full : undefined;
  } catch {
    return undefined;
  }
}

/** `try_files {path} {path}.html {path}/index.html`, after the shells' rewrites. */
async function resolve(pathname: string): Promise<string | undefined> {
  const target = staticTarget(pathname);
  for (const candidate of [target, `${target}.html`, `${target}/index.html`]) {
    const found = await file(candidate);
    if (found) return found;
  }
  return undefined;
}

function forward(req: http.IncomingMessage, res: http.ServerResponse, origin: string) {
  const target = new URL(req.url ?? "/", origin);
  const upstream = http.request(
    target,
    { method: req.method, headers: { ...req.headers, host: target.host } },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers);
      res.flushHeaders();
      answer.pipe(res);
    },
  );
  upstream.on("error", () => {
    if (!res.headersSent) res.writeHead(502, { "Content-Type": "text/plain" });
    res.end("the API could not be reached\n");
  });
  req.pipe(upstream);
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url ?? "/", "http://static.invalid");
  const { pathname } = url;
  const proxied =
    pathname.startsWith("/api/") ||
    pathname.startsWith("/agui/") ||
    (edge && pathname.startsWith("/oauth2/"));
  if (api && proxied) return forward(req, res, api);
  if (req.method !== "GET" && req.method !== "HEAD") {
    res.writeHead(405, { Allow: "GET, HEAD" });
    return void res.end();
  }
  let found = await resolve(decodeURIComponent(pathname));
  let status = 200;
  if (!found) {
    found = await file("404.html");
    status = 404;
  }
  if (!found) {
    res.writeHead(404, headers(pathname));
    return void res.end("not found\n");
  }
  res.writeHead(status, {
    ...headers(pathname),
    "Content-Type": TYPES[path.extname(found)] ?? "application/octet-stream",
  });
  if (req.method === "HEAD") return void res.end();
  createReadStream(found).pipe(res);
});

server.listen(port, "127.0.0.1", () => {
  console.log(
    `serve-static: ${path.relative(process.cwd(), dir) || "."} on http://127.0.0.1:${port}${api ? `, API ${api}` : ""}`,
  );
});
