import { defineConfig, devices } from "@playwright/test";

const CI = Boolean(process.env.CI);

export default defineConfig({
  testDir: "e2e",
  fullyParallel: true,
  forbidOnly: CI,
  retries: CI ? 1 : 0,
  reporter: CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: "http://127.0.0.1:3000",
    trace: "on-first-retry",
  },
  webServer: [
    {
      command: "pnpm mock",
      url: "http://127.0.0.1:4010/healthz",
      reuseExistingServer: !CI,
    },
    {
      // A production build, so the dev overlay does not skew the accessibility checks.
      command: "pnpm build:e2e && pnpm start:e2e",
      url: "http://127.0.0.1:3000",
      timeout: 240_000,
      reuseExistingServer: !CI,
    },
  ],
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "mobile", use: { ...devices["Pixel 7"] }, testMatch: /create-thread|follow-up|cancel/ },
  ],
});
