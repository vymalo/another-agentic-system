// Builds the web's static export for the desktop app (ADR 0047) into web/out-tauri, and writes what the app is built for from
// one place, `deployment.json` (or the environment, which wins):
//
//   AGENTIC_API_ORIGIN     the API's origin (deployment.json `apiOrigin`)
//   AGENTIC_ISSUER_ORIGIN  the issuer's origin (`issuerOrigin`), the one address the app opens in the browser (build.rs reads it too)
//   AGENTIC_CLIENT_ID      the public client (another-agentic-desktop)
//
// It writes the web's runtime configuration (web/out-tauri/config.json) and the app's content security policy
// (src-tauri/tauri.deployment.json, which `pnpm build` passes to `tauri build --config`): `connect-src` names exactly those two
// origins. The API's orchestrator must list this app's origins in server.cors.allowedOrigins.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const app = path.resolve(here, "..");
const web = path.resolve(app, "../../web");
const out = "out-tauri";
const deployment = JSON.parse(readFileSync(path.join(app, "deployment.json"), "utf8"));

/** An origin exactly as a browser writes it, or the build stops. */
function origin(name, value) {
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error(`${name} is not a URL: ${value}`);
  }
  if (parsed.origin !== value) throw new Error(`${name} is not an origin (scheme://host[:port], lower case): ${value}`);
  return value;
}
const apiOrigin = origin("AGENTIC_API_ORIGIN", process.env.AGENTIC_API_ORIGIN || deployment.apiOrigin);
const issuerOrigin = origin("AGENTIC_ISSUER_ORIGIN", process.env.AGENTIC_ISSUER_ORIGIN || deployment.issuerOrigin);
const clientId = process.env.AGENTIC_CLIENT_ID || "another-agentic-desktop";

execFileSync("pnpm", ["install", "--frozen-lockfile"], { cwd: web, stdio: "inherit" });
execFileSync("pnpm", ["gen:api"], { cwd: web, stdio: "inherit" });
execFileSync("pnpm", ["exec", "next", "build"], {
  cwd: web,
  stdio: "inherit",
  env: { ...process.env, NEXT_DIST_DIR: out },
});
execFileSync("pnpm", ["exec", "tsx", "scripts/csp-meta.ts", out], { cwd: web, stdio: "inherit" });
const config = { apiOrigin, clientId, signIn: "loopback" };
writeFileSync(path.join(web, out, "config.json"), `${JSON.stringify(config, null, 2)}\n`);
const connect = ["'self'", "ipc:", "http://ipc.localhost", ...new Set([apiOrigin, issuerOrigin])].join(" ");
const overlay = { app: { security: { csp: { "connect-src": connect } } } };
writeFileSync(path.join(app, "src-tauri", "tauri.deployment.json"), `${JSON.stringify(overlay, null, 2)}\n`);
console.log(`build-web: ${path.join("web", out)} for ${apiOrigin}, signing in at ${issuerOrigin} as ${clientId}`);
