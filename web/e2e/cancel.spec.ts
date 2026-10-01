import { expect, test } from "@playwright/test";
import { activityTab, badge, hideActivity, showActivity, startThread } from "./helpers";

test("cancel calls the endpoint and the thread ends cancelled", async ({ page }) => {
  await startThread(page, "slow task");

  const cancel = page.getByRole("button", { name: "Stop" });
  await expect(cancel).toBeVisible();
  await expect(badge(page)).toHaveText("Working…");

  const request = page.waitForRequest(
    (r) => r.method() === "POST" && /\/api\/threads\/[^/]+\/cancel$/.test(r.url()),
  );
  await cancel.click();
  await request;

  await expect(badge(page)).toHaveText("Stopped");
  // the stop is a step of the turn, so the panel's
  await showActivity(page);
  await expect(activityTab(page).getByText("Stopped", { exact: true }).first()).toBeVisible();
  await hideActivity(page);
  await expect(page.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
});
