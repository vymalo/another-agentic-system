import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("an agent failure is a callout with the agent's reason", async ({ page }) => {
  await startThread(page, "fail please");

  const log = page.getByRole("log", { name: "Conversation" });
  const callout = log.locator('[data-slot="error-callout"]');
  await expect(callout).toContainText("coder couldn’t finish", { ignoreCase: true });
  await expect(callout.getByText("scripted failure", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Failed");
  // The orchestrator reports an agent failure as `agent_status: failed`, not as an error event.
  await expect(log.getByText("Something went wrong")).toHaveCount(0);
  // not locked: the box stays open, and nothing tells the person to start over
  await expect(page.getByLabel("Message")).toBeEnabled();
  await expect(page.getByText("This thread is failed.")).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Start a new thread" })).toHaveCount(0);
});

test("an undeliverable message shows the error line and waits for the user", async ({ page }) => {
  await startThread(page, "unreachable agent");

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("Something went wrong, and it may pass")).toBeVisible();
  await expect(log.getByText("the agent could not be reached", { exact: false })).toBeVisible();
  await expect(log.getByText("You can send a message to retry.")).toBeVisible();
  // the agent asked nothing: the thread needs a look, not an answer
  await expect(badge(page)).toHaveText("Needs attention");
});
