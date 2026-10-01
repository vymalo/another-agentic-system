import { expect, test } from "@playwright/test";
import { badge, framesOf, resetDb, shape, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

test("fail: a callout with the agent's reason and a Failed badge", async ({ page }) => {
  await startThread(page, "fail please", "Plain");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("scripted failure", { exact: true })).toBeVisible();
  await expect(log.locator('[data-slot="error-callout"]')).toContainText("couldn’t finish");
  await expect(badge(page)).toHaveText("Failed");
  // Pins the failure shape (docs/api/examples/agui/fail.agui.json): an agent failure is a failed
  // status with its detail and a RUN_ERROR agent_failed, and no error activity.
  await expect(log.getByText("Something went wrong")).toHaveCount(0);
  const frames = shape(await framesOf(page.request, threadId(page)));
  expect(frames).toContain("ACTIVITY_SNAPSHOT:vymalo.status:failed");
  expect(frames.at(-1)).toBe("RUN_ERROR:agent_failed");
  expect(frames.some((f) => f.includes("vymalo.error"))).toBe(false);
});
