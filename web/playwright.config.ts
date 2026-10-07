import { defineConfig, devices } from "@playwright/test";

const CI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "e2e",
  // the screenshots are `pnpm screens` (playwright.screens.config.ts), not a test; the session spec
  // needs a build with the edge's sign-in built in (playwright.session.config.ts), and the browser-auth spec a mock that
  // is the issuer and a build of its own (playwright.browser-auth.config.ts)
  testIgnore: /screens\.spec\.ts|session-refresh\.spec\.ts|browser-auth\.spec\.ts/,
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    baseURL: "http://127.0.0.1:3000",
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
      // A production build, so the dev overlay does not skew the accessibility checks.
      command: "pnpm build:e2e && pnpm start:e2e",
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:3000",
      timeout: 240_000,
      reuseExistingServer: !CI,
    },
  ],
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    {
      name: "mobile",
      use: { ...devices["Pixel 7"] },
      testMatch:
        /create-thread|follow-up|cancel|failure|verification|ci-card|export|choices|cards\.spec|files\.spec|tools\.spec|agent-menu|registry\.spec|panel|steps\.spec|step-io|stream\.spec|thinking\.spec|fork\.spec|branches\.spec|answer-view|description\.spec|roles\.spec|steer\.spec|mentions\.spec|asks\.spec|share\.spec/,
    },
  ],
});
