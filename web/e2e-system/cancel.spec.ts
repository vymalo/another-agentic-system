import { expect, test } from "@playwright/test";
import { allCalls, badge, callsFor, resetDb, startThread } from "./helpers";

test.beforeEach(resetDb);

test("slow: Cancel reaches the agent and the thread ends Cancelled", async ({ page }) => {
  await startThread(page, "slow work", "Plain");
  const cancel = page.getByRole("button", { name: "Cancel" });
  await expect(cancel).toBeVisible();
  await expect(badge(page)).toHaveText("Working");

  await cancel.click();
  await expect(badge(page)).toHaveText("Cancelled");
  await expect(page.getByRole("button", { name: "Cancel" })).toHaveCount(0);

  const [execution] = await callsFor(page.request, "plain", "slow work");
  const cancels = (await allCalls(page.request, "plain")).filter((c) => c.kind === "cancel");
  expect(cancels.map((c) => c.taskId)).toEqual([execution?.taskId]);
});
