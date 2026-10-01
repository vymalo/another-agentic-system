import { expect, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/*
 * A thread is a conversation (ADR 0020): it never locks. A message after `Done`, `Failed` or
 * `Stopped` starts the thread's next job, in the same transcript, on the same agent.
 */

test("a follow-up after Done goes on in the same conversation", async ({ page }) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");

  const box = page.getByLabel("Message");
  await expect(box).toBeEnabled();
  await expect(box).toHaveAttribute("placeholder", "Send a follow-up…");
  await expect(page.getByText(/This thread is/)).toHaveCount(0);
  await expect(page.getByRole("link", { name: "Start a new thread" })).toHaveCount(0);

  await box.fill("echo and now the tests");
  await page.getByRole("button", { name: "Send" }).click();

  const log = conversation(page);
  await expect(log.getByText("echo and now the tests", { exact: true })).toBeVisible();
  await expect(log.getByText("echo: echo and now the tests")).toHaveCount(1);
  await expect(badge(page)).toHaveText("Done");
  // both jobs are in the transcript, the first one untouched
  await expect(log.getByText("echo: echo first")).toHaveCount(1);
  await expect(log.getByRole("link", { name: "Pull request acme/demo#1" })).toHaveCount(2);
  await expect(box).toBeEnabled();
  await expectNoHorizontalScroll(page);

  // a reload replays both jobs, in order
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("echo first", { exact: true })).toBeVisible();
  await expect(log.getByText("echo and now the tests", { exact: true })).toBeVisible();
  await expect(log.getByRole("link", { name: "Pull request acme/demo#1" })).toHaveCount(2);
});

test("a stopped thread takes the next message", async ({ page }) => {
  await startThread(page, "slow task");
  await expect(badge(page)).toHaveText("Working…");
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");

  const box = page.getByLabel("Message");
  await expect(box).toBeEnabled();
  await expect(box).toHaveAttribute("placeholder", "Tell the agent how to go on…");
  await box.fill("echo never mind, do this");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("echo: echo never mind, do this")).toHaveCount(1);
});

test("a failed thread takes the next message", async ({ page }) => {
  await startThread(page, "fail please");
  await expect(badge(page)).toHaveText("Failed");

  const box = page.getByLabel("Message");
  await expect(box).toBeEnabled();
  await box.fill("echo try again");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("echo: echo try again")).toHaveCount(1);
  // the failure of the first job is still in the transcript
  await expect(conversation(page).getByText("scripted failure", { exact: true })).toBeVisible();
});

test("while the agent works the box is for drafting, and the button is Stop", async ({ page }) => {
  await startThread(page, "slow task");
  await expect(badge(page)).toHaveText("Working…");
  const box = page.getByLabel("Message");
  await expect(box).toBeEnabled();
  await expect(page.getByRole("button", { name: "Send" })).toHaveCount(0);
  await box.fill("a thought for later");
  await box.press("Enter");
  // nothing was sent (Enter is a new line while the run is live): the draft is still there
  await expect(box).toHaveValue(/^a thought for later/);
  await expect(conversation(page).getByText("a thought for later")).toHaveCount(0);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(box).toHaveValue(/^a thought for later/);
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
});
