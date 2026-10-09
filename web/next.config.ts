import path from "node:path";
import type { NextConfig } from "next";
import { PHASE_DEVELOPMENT_SERVER } from "next/constants";

// A build is a static export (ADR 0047): `out/` (or `NEXT_DIST_DIR`) holds every page as HTML, which the web image serves
// with Caddy (`Caddyfile`), the e2e with `scripts/serve-static.ts`, and the desktop app from its own assets. An export has no
// server: no rewrites, no headers, no proxy. The headers and the content security policy are the static server's
// (`Caddyfile`; `scripts/csp-meta.mjs` writes each page's script hashes into it after the build).
//
// `next dev` is not an export: it forwards /api/* (the resource API), /agui/* (AG-UI) and, against the mock, /oauth2/* (the
// edge's routes) to the mock server (MOCK_API_ORIGIN) or a real orchestrator (API_ORIGIN), as the edge does in a deployment.
const apiOrigin = process.env.API_ORIGIN ?? process.env.MOCK_API_ORIGIN;
const mockEdge = !process.env.API_ORIGIN && !!process.env.MOCK_API_ORIGIN;
// `pnpm build:session` and `pnpm build:browser` export beside the build the other specs use.
const distDir = process.env.NEXT_DIST_DIR;

const shared: NextConfig = {
  turbopack: { root: path.resolve(".") },
  poweredByHeader: false,
  reactStrictMode: true,
  // Next 16.3 dev writes AGENTS.md/CLAUDE.md into the project; agent context lives in the repo root.
  agentRules: false,
  images: { unoptimized: true },
};

export default function config(phase: string): NextConfig {
  if (phase === PHASE_DEVELOPMENT_SERVER) {
    return {
      ...shared,
      async rewrites() {
        return apiOrigin
          ? ["api", "agui", ...(mockEdge ? ["oauth2"] : [])].map((prefix) => ({
              source: `/${prefix}/:path*`,
              destination: `${apiOrigin}/${prefix}/:path*`,
            }))
          : [];
      },
    };
  }
  return {
    ...shared,
    output: "export",
    // with an export, `distDir` is where the pages are written (Next keeps its build in .next)
    ...(distDir ? { distDir } : {}),
  };
}
