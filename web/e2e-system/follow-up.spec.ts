import { expect, test } from "@playwright/test";
import { badge, resetDb, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

test("a finished thread refuses a follow-up with the 409 problem", async ({ page }) => {
  await startThread(page, "echo done", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByLabel("Message")).toBeDisabled();

  const res = await page.request.post(`/api/threads/${threadId(page)}/messages`, {
    data: { text: "one more thing" },
  });
  expect(res.status()).toBe(409);
  expect(res.headers()["content-type"]).toContain("application/problem+json");
  expect(await res.json()).toMatchObject({ status: 409 });

  // nothing was appended
  await expect(
    page.getByRole("log", { name: "Conversation" }).getByText("one more thing"),
  ).toHaveCount(0);
});
