import { defineConfig, devices } from "@playwright/test";

/**
 * The web that holds its own tokens (ADR 0054, web/README.md "Signing in again"), in a real browser
 * against the mock playing the issuer: `e2e/browser-auth.spec.ts`. It needs its own build and its own
 * mock: the mock answers `/api/public/auth` (browser mode) and wants DPoP on every API request
 * only when started with `MOCK_BROWSER_AUTH=1`, and the policy of the app names the issuer's origin
 * (`WEB_CSP_CONNECT_SRC`, read when the server starts). The build also carries the edge's sign-in
 * path (`NEXT_PUBLIC_SIGN_IN_PATH`), as the deployed image does, to show that browser mode wins. The
 * build goes into `.next-browser`, on port 3002.
 */
const CI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "e2e",
  testMatch: /browser-auth\.spec\.ts/,
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: CI
    ? [["list"], ["html", { open: "never", outputFolder: "playwright-report-browser" }]]
    : "list",
  use: {
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    baseURL: "http://127.0.0.1:3002",
    trace: "on-first-retry",
  },
  webServer: [
    {
      command: "MOCK_BROWSER_AUTH=1 pnpm mock",
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:4010/healthz",
      reuseExistingServer: !CI,
    },
    {
      command: "pnpm build:browser && pnpm start:browser",
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:3002",
      timeout: 240_000,
      reuseExistingServer: !CI,
    },
  ],
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
