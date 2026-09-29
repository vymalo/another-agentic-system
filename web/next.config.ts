import path from "node:path";
import type { NextConfig } from "next";

// Dev/e2e only: forwards /api/* to the mock server. Never set in the production image,
// where oauth2-proxy / the ingress routes /api/* to the orchestrator (same origin).
const mockOrigin = process.env.MOCK_API_ORIGIN;

const config: NextConfig = {
  output: "standalone",
  outputFileTracingRoot: path.resolve("."),
  turbopack: { root: path.resolve(".") },
  poweredByHeader: false,
  reactStrictMode: true,
  images: { unoptimized: true },
  async rewrites() {
    return mockOrigin ? [{ source: "/api/:path*", destination: `${mockOrigin}/api/:path*` }] : [];
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
