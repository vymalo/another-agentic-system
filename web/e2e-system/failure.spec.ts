import { expect, test } from "@playwright/test";
import { badge, eventsOf, resetDb, shape, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

test("fail: Failed status with the agent's detail and a Failed badge", async ({ page }) => {
  await startThread(page, "fail please", "Plain");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("Failed: scripted failure")).toBeVisible();
  await expect(badge(page)).toHaveText("Failed");
  // Pins the failure shape (docs/api/examples/fail.events.json): an agent failure is an
  // `agent_status: failed` with its detail, and no `error` event.
  await expect(log.getByText("Error:")).toHaveCount(0);
  expect(shape(await eventsOf(page.request, threadId(page)))).toEqual([
    "user_message",
    "agent_status:working",
    "agent_status:failed",
    "thread_state:failed",
  ]);
});
