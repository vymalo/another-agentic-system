import { expect, type Request, test } from "@playwright/test";
import { badge, resetDb, startThread } from "./helpers";

test.beforeEach(resetDb);

test("a finished thread opened by URL renders once and the stream closes", async ({ page }) => {
  await startThread(page, "echo history", "Plain");
  await expect(badge(page)).toHaveText("Done");

  // every /stream request the page makes from here on, and whether it has ended
  const open = new Set<Request>();
  let started = 0;
  const isStream = (r: Request) => new URL(r.url()).pathname.endsWith("/stream");
  page.on("request", (r) => {
    if (!isStream(r)) return;
    started++;
    open.add(r);
  });
  page.on("requestfinished", (r) => void open.delete(r));
  page.on("requestfailed", (r) => void open.delete(r));

  await page.reload();
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("echo history", { exact: true })).toHaveCount(1);
  await expect(log.getByText("Working")).toHaveCount(1);
  await expect(log.getByText("Completed")).toHaveCount(1);
  await expect(log.getByRole("link", { name: /^Pull request / })).toHaveCount(1);
  await expect(log.getByText("echo: echo history")).toHaveCount(1);

  // the history was replayed through the stream once, and the stream is closed afterwards
  expect(started).toBeGreaterThanOrEqual(1);
  await expect.poll(() => open.size, { message: "open /stream requests" }).toBe(0);
  const total = started;
  // a window focus refetches the thread list; it must not reopen the finished thread's stream
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForLoadState("networkidle");
  expect(started).toBe(total);
});
