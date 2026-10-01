import { expect, test } from "@playwright/test";
import { badge, startThread } from "./helpers";

test("a blocked thread waits for the answer, and the follow-up resumes it", async ({ page }) => {
  await startThread(page, "ask pick a branch");

  await expect(page.getByText("Waiting for your answer.")).toBeVisible();
  await expect(page.getByText("Which branch?").first()).toBeVisible();
  await expect(badge(page)).toHaveText("Your turn");

  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(log.getByText("main", { exact: true })).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("answered: main")).toHaveCount(1);
  await expect(log.getByRole("link", { name: /^View pull request / })).toBeVisible();
});
