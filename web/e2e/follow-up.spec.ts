import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("a blocked thread waits for the answer, and the follow-up resumes it", async ({ page }) => {
  await startThread(page, "question: pick a branch");

  await expect(page.getByText("Waiting for your answer.")).toBeVisible();
  await expect(page.getByText("Which branch should I use?").first()).toBeVisible();
  await expect(badge(page)).toHaveText("Waiting for you");

  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("main", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("Thanks, continuing on that branch.")).toBeVisible();
});
