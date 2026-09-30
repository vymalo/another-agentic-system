import { expect, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

test("a second tab follows a run it did not start, and sees it cancelled from the first", async ({
  page,
  context,
}) => {
  await startThread(page, "slow task");
  await expect(badge(page)).toHaveText("Working…");

  const other = await context.newPage();
  await other.goto(page.url());
  await expect(badge(other)).toHaveText("Working…");
  await expect(conversation(other).getByText("slow task", { exact: true })).toBeVisible();
  await expect(conversation(other).getByText("Working", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(other)).toHaveText("Stopped");
  await expect(conversation(other).getByText("Cancelled", { exact: false }).first()).toBeVisible();
  await expect(badge(page)).toHaveText("Stopped");
});

test("an answer given in one tab reaches the other, which was waiting for it", async ({
  page,
  context,
}) => {
  await startThread(page, "ask pick a branch");
  await expect(page.getByText("Waiting for your answer.")).toBeVisible();

  const other = await context.newPage();
  await other.goto(page.url());
  await expect(other.getByText("Waiting for your answer.")).toBeVisible();

  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();

  // the other tab never sent anything: the answer and the run it caused arrive on its stream
  const log = conversation(other);
  await expect(log.getByText("main", { exact: true })).toBeVisible();
  await expect(badge(other)).toHaveText("Done");
  await expect(log.getByText("answered: main")).toHaveCount(1);
  await expect(log.getByText("Needs input: Which branch?")).toHaveCount(1);
});

test("reloading a blocked thread offers the answer again", async ({ page }) => {
  await startThread(page, "ask pick a branch");
  await expect(page.getByText("Waiting for your answer.")).toBeVisible();
  await page.reload();
  await expect(page.getByText("Waiting for your answer.")).toBeVisible();
  await expect(page.getByText("Which branch?").first()).toBeVisible();
  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("answered: main")).toHaveCount(1);
});
