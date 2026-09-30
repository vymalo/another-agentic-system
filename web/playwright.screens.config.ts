import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

/**
 * `pnpm screens`: regenerates the screenshots of e2e/__screens__/ (e2e/screens.spec.ts) against the
 * mock server and a production build, the same servers as the mock suite. One worker, so the
 * sidebar fills with the threads of the earlier scenarios the way a real one does.
 */
export default defineConfig({
  ...base,
  testMatch: /screens\.spec\.ts/,
  testIgnore: [],
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: "list",
  projects: [
    {
      name: "desktop",
      use: {
        browserName: "chromium",
        viewport: { width: 1440, height: 900 },
        deviceScaleFactor: 1,
      },
    },
    {
      name: "mobile",
      use: {
        browserName: "chromium",
        viewport: { width: 390, height: 844 },
        deviceScaleFactor: 2,
        isMobile: true,
        hasTouch: true,
      },
    },
  ],
});
