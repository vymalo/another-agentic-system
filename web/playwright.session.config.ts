import { defineConfig, devices } from "@playwright/test";

/**
 * The page's session refresh and sign-in (web/README.md "Signing in again"), in a real browser against
 * the mock: `e2e/session-refresh.spec.ts`. It needs its own build of the app, with the edge's sign-in
 * built in (`NEXT_PUBLIC_SIGN_IN_PATH`, which Next inlines at build time): the other specs run on a build
 * without one, because several of them say what a page does when there is none. The mock stands in
 * for oauth2-proxy's `/oauth2/userinfo` and `/oauth2/start` (mock/server.ts), and the build goes into
 * `.next-session`, beside the other build, on port 3001.
 */
const CI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "e2e",
  testMatch: /session-refresh\.spec\.ts/,
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: CI
    ? [["list"], ["html", { open: "never", outputFolder: "playwright-report-session" }]]
    : "list",
  use: {
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    baseURL: "http://127.0.0.1:3001",
    trace: "on-first-retry",
  },
  webServer: [
    {
      command: "pnpm mock",
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:4010/healthz",
      reuseExistingServer: !CI,
    },
    {
      command: "pnpm build:session && pnpm start:session",
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:3001",
      timeout: 600_000,
      reuseExistingServer: !CI,
    },
  ],
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
