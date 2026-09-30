import { expect, test } from "@playwright/test";
import { allCalls, badge, callsFor, resetDb, startThread } from "./helpers";

test.beforeEach(resetDb);

test("slow: Stop reaches the agent and the thread ends Stopped", async ({ page }) => {
  await startThread(page, "slow work", "Plain");
  const cancel = page.getByRole("button", { name: "Stop" });
  await expect(cancel).toBeVisible();
  await expect(badge(page)).toHaveText("Working…");

  await cancel.click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(page.getByRole("button", { name: "Stop" })).toHaveCount(0);

  const [execution] = await callsFor(page.request, "plain", "slow work");
  const cancels = (await allCalls(page.request, "plain")).filter((c) => c.kind === "cancel");
  expect(cancels.map((c) => c.taskId)).toEqual([execution?.taskId]);
});
