import { expect, type Request, test } from "@playwright/test";
import { activityTab, badge, resetDb, showActivity, startThread } from "./helpers";

test.beforeEach(resetDb);

test("a finished thread opened by URL renders once from its history and nothing stays open", async ({
  page,
}) => {
  await startThread(page, "echo history", "Plain");
  await expect(badge(page)).toHaveText("Done");

  // every read of the thread the page makes from here on (its history, its connect stream), and whether it has ended
  const open = new Set<Request>();
  let started = 0;
  let pages = 0;
  const isRead = (r: Request) => /\/(connect|history)$/.test(new URL(r.url()).pathname);
  page.on("request", (r) => {
    if (!isRead(r)) return;
    started++;
    if (new URL(r.url()).pathname.endsWith("/history")) pages++;
    open.add(r);
  });
  page.on("requestfinished", (r) => void open.delete(r));
  page.on("requestfailed", (r) => void open.delete(r));

  await page.reload();
  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("echo history", { exact: true })).toHaveCount(1);
  // the steps are the side panel's
  await showActivity(page);
  await expect(activityTab(page).getByText("Started working", { exact: true })).toHaveCount(1);
  await expect(activityTab(page).getByText("Opened pull request #1")).toHaveCount(1);
  await expect(log.getByRole("link", { name: /^View pull request / })).toHaveCount(1);
  await expect(log.getByText("echo: echo history")).toHaveCount(1);

  // a finished thread opens from its history (ADR 0059), and nothing that reads it is left open afterwards
  expect(pages).toBeGreaterThanOrEqual(1);
  await expect.poll(() => open.size, { message: "open /history and /connect requests" }).toBe(0);
  const total = started;
  // a window focus refetches the thread list; it must not read the finished thread again
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await page.waitForLoadState("networkidle");
  expect(started).toBe(total);
});
