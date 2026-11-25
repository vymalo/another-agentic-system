import { expect, test } from "@playwright/test";
import { agentPicker, MOCK_URL } from "./helpers";

/*
 * A deployment that serves only a newer application (`server.minClientVersion`, ADR 0076): the mock answers every call of
 * a client below `?minClient=` with the problem `client_update_required` (426), and the page shows the update screen in
 * place of an error line. The session is a person of its own, so no other spec sees the minimum.
 */

let sessions = 0;
async function join(context: import("@playwright/test").BrowserContext, settings: string) {
  const session = `e2e-update-${process.pid}-${Date.now()}-${++sessions}`;
  const res = await fetch(`${MOCK_URL}/__mock/config?session=${session}&${settings}`, {
    method: "POST",
  });
  expect(res.ok).toBe(true);
  await context.clearCookies({ name: "mock-registry" });
  await context.addCookies([
    { name: "mock-registry", value: session, url: "http://127.0.0.1:3000" },
  ]);
}

test("update_required_shows_update_screen", async ({ page, context }) => {
  await join(context, "minClient=999.0.0");
  await page.goto("/");
  const screen = page.locator('[data-slot="update-required"]');
  await expect(screen).toBeVisible();
  await expect(screen.getByRole("heading", { name: "Update the application" })).toBeVisible();
  await expect(screen.getByRole("button", { name: "Reload" })).toBeVisible();
  // the screen takes the whole page: nothing behind it can be reached
  const box = await screen.boundingBox();
  const viewport = page.viewportSize();
  expect(box?.width).toBe(viewport?.width);
  expect(box?.height).toBe(viewport?.height);
});

test("a version at or above the minimum is served and shows no update screen", async ({
  page,
  context,
}) => {
  await join(context, "minClient=0.1.0");
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await expect(page.locator('[data-slot="update-required"]')).toHaveCount(0);
});

test("every call to the API states the version", async ({ page, context }) => {
  await join(context, "minClient=none");
  const stated: Array<string | undefined> = [];
  page.on("request", (request) => {
    // `/api/public/…` is outside identity and the version check (the page's own sign-in configuration)
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/") && !path.startsWith("/api/public/")) {
      stated.push(request.headers()["x-client-version"] ?? `MISSING ${request.url()}`);
    }
  });
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  expect(stated.length).toBeGreaterThan(0);
  for (const version of stated) expect(version).toMatch(/^\d+(\.\d+){0,3}$/);
});
