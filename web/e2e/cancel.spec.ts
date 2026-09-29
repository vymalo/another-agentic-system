import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("cancel calls the endpoint and the thread ends cancelled", async ({ page }) => {
  await startThread(page, "a slow task");

  const cancel = page.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeVisible();
  await expect(badge(page)).toHaveText("Working");

  const request = page.waitForRequest(
    (r) => r.method() === "POST" && /\/api\/threads\/[^/]+\/cancel$/.test(r.url()),
  );
  await cancel.click();
  await request;

  await expect(badge(page)).toHaveText("Cancelled");
  await expect(
    page.getByRole("log").getByText("Cancelled", { exact: false }).first(),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Cancel" })).toHaveCount(0);
});
