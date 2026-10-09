// Builds the web's static export for the desktop app (ADR 0047) into web/out-tauri, and writes its runtime configuration
// (web/src/lib/runtime-config.ts): the deployment's API, the desktop's own public client, the loopback sign-in.
//
//   AGENTIC_API_ORIGIN=https://chat.example.com node scripts/build-web.mjs
//
// AGENTIC_API_ORIGIN (default https://agentic.servers.segning.pro) must also be in the app's connect-src
// (src-tauri/tauri.conf.json, or `tauri build --config`), and this app's origins in the orchestrator's server.cors.allowedOrigins.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const web = path.resolve(here, "../../../web");
const out = "out-tauri";
const apiOrigin = process.env.AGENTIC_API_ORIGIN ?? "https://agentic.servers.segning.pro";
if (new URL(apiOrigin).origin !== apiOrigin) throw new Error(`AGENTIC_API_ORIGIN is not an origin: ${apiOrigin}`);

execFileSync("pnpm", ["install", "--frozen-lockfile"], { cwd: web, stdio: "inherit" });
execFileSync("pnpm", ["gen:api"], { cwd: web, stdio: "inherit" });
execFileSync("pnpm", ["exec", "next", "build"], {
  cwd: web,
  stdio: "inherit",
  env: { ...process.env, NEXT_DIST_DIR: out },
});
execFileSync("pnpm", ["exec", "tsx", "scripts/csp-meta.ts", out], { cwd: web, stdio: "inherit" });
const config = {
  apiOrigin,
  clientId: process.env.AGENTIC_CLIENT_ID ?? "another-agentic-desktop",
  signIn: "loopback",
};
writeFileSync(path.join(web, out, "config.json"), `${JSON.stringify(config, null, 2)}\n`);
console.log(`build-web: ${path.join("web", out)} for ${apiOrigin}`);
