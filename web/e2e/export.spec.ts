import { readFile } from "node:fs/promises";
import { expect, test } from "@playwright/test";
import {
  badge,
  expectNoHorizontalScroll,
  exportMenuItem,
  startThread,
  THREAD_URL,
} from "./helpers";

test("Export JSON downloads the thread as thread-<id>.json", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1];
  expect(page.url()).toMatch(THREAD_URL);

  // Export JSON is an item of the thread's overflow menu
  const button = await exportMenuItem(page);
  await expect(button).toBeEnabled();
  await expectNoHorizontalScroll(page);
  const [download] = await Promise.all([page.waitForEvent("download"), button.click()]);

  expect(download.suggestedFilename()).toBe(`thread-${id}.json`);
  const path = await download.path();
  const doc = JSON.parse(await readFile(path, "utf8"));
  expect(doc.format).toBe("another-agentic-system/thread-export");
  expect(doc.version).toBe(1);
  expect(doc.thread.id).toBe(id);
  expect(doc.thread.state).toBe("done");
  expect(doc.events.length).toBeGreaterThan(0);
  // the web's catalog is the first event of the thread it creates (ADR 0023), then the message
  expect(doc.events[0].kind).toBe("ui_catalog");
  expect(doc.events[1].kind).toBe("user_message");
  expect(doc.events[1].data.text).toBe("echo hello");
  await expect(await exportMenuItem(page)).toBeEnabled();
});

test("a failed export says so and keeps the page usable", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  await page.route("**/api/threads/*/export", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Service Unavailable",
        status: 503,
        detail: "storage is unavailable",
      }),
    }),
  );
  await (await exportMenuItem(page)).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not export the thread" }),
  ).toContainText("storage is unavailable");
  await expect(await exportMenuItem(page)).toBeEnabled();
});
