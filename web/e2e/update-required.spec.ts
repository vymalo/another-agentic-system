import { test as base, expect } from "@playwright/test";
import { agentPicker, MOCK_URL } from "./helpers";

/*
 * The minimum application version (docs/api/client-version.md): every call of the API states the
 * build's version, and a deployment that needs a newer one gets the "update the application" screen in
 * place of the page. The mock keeps the minimum per session, named by a cookie.
 */

const ORIGIN = "http://127.0.0.1:3000";

const test = base.extend<{ minimum: (version: string) => Promise<void> }>({
  minimum: async ({ context }, use, info) => {
    const session = `e2e-${info.workerIndex}-${info.testId}`;
    await use(async (version) => {
      const res = await fetch(
        `${MOCK_URL}/__mock/config?minClientVersion=${version}&session=${session}`,
        { method: "POST" },
      );
      expect(res.ok).toBe(true);
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    });
  },
});

test("every call of the API states the application's version", async ({ page }) => {
  const stated: (string | undefined)[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/api/agents") {
      stated.push(request.headers()["x-client-version"]);
    }
  });
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  expect(stated.length).toBeGreaterThan(0);
  for (const version of stated) expect(version).toMatch(/^\d+(\.\d+){0,3}$/);
});

test("update_required_shows_update_screen", async ({ page, minimum }) => {
  await minimum("99.0.0");
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Update the application" })).toBeVisible();
  await expect(page.getByText("Needs version 99.0.0 or later.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Reload" })).toBeVisible();
  // the screen stands in for the app: no picker, no error line
  await expect(agentPicker(page)).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("a version the deployment accepts opens the app", async ({ page, minimum }) => {
  await minimum("0.0.1");
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await expect(page.getByRole("heading", { name: "Update the application" })).toHaveCount(0);
});
