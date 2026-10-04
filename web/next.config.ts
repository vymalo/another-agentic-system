import path from "node:path";
import type { NextConfig } from "next";

// Dev/e2e only: forwards /api/* (the resource API) and /agui/* (AG-UI: run, connect, capabilities)
// to the mock server (MOCK_API_ORIGIN) or, for the system e2e tests, to a real orchestrator
// (API_ORIGIN). Never set in the production image, where oauth2-proxy / the ingress routes both
// to the orchestrator (same origin).
const apiOrigin = process.env.API_ORIGIN ?? process.env.MOCK_API_ORIGIN;
// The mock also stands in for the edge's own routes (`/oauth2/userinfo`, `/oauth2/start`) so that the page's
// session refresh and sign-in can be run against it (web/README.md "Signing in again"); a real orchestrator has none.
const mockEdge = !process.env.API_ORIGIN && !!process.env.MOCK_API_ORIGIN;
// `pnpm build:session` builds with the sign-in built in, beside the build the other specs use.
const distDir = process.env.NEXT_DIST_DIR;

const config: NextConfig = {
  output: "standalone",
  ...(distDir ? { distDir } : {}),
  outputFileTracingRoot: path.resolve("."),
  turbopack: { root: path.resolve(".") },
  poweredByHeader: false,
  reactStrictMode: true,
  // Next 16.3 dev writes AGENTS.md/CLAUDE.md into the project; agent context lives in the repo root.
  agentRules: false,
  images: { unoptimized: true },
  async rewrites() {
    return apiOrigin
      ? [
          ...["api", "agui", ...(mockEdge ? ["oauth2"] : [])].map((prefix) => ({
            source: `/${prefix}/:path*`,
            destination: `${apiOrigin}/${prefix}/:path*`,
          })),
        ]
      : [];
  },
  async headers() {
    return [
      {
        source: "/:path*",
        headers: [
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "Referrer-Policy", value: "same-origin" },
        ],
      },
    ];
  },
};

export default config;
