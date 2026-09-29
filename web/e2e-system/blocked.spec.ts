import { expect, test } from "@playwright/test";
import { allCalls, badge, resetDb, startThread } from "./helpers";

test.beforeEach(resetDb);

test("ask blocks, the answer resumes the same A2A task", async ({ page }) => {
  await startThread(page, "ask which branch", "Plain");

  await expect(page.getByText("Waiting for your answer.")).toBeVisible();
  await expect(page.getByText("Which branch?").first()).toBeVisible();
  await expect(badge(page)).toHaveText("Waiting for you");

  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();

  const log = page.getByRole("log", { name: "Conversation" });
  await expect(badge(page)).toHaveText("Done");
  await expect(log.getByText("answered: main")).toHaveCount(1);
  await expect(log.getByText("main", { exact: true })).toBeVisible();

  const executions = (await allCalls(page.request, "plain")).filter(
    (c) => c.kind === "execute" && ["ask which branch", "main"].includes(c.text),
  );
  expect(executions.map((c) => c.text)).toEqual(["ask which branch", "main"]);
  expect(executions[1]?.taskId).toBe(executions[0]?.taskId);
  expect(executions.map((c) => c.resuming)).toEqual([false, true]);
});
