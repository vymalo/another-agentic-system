import { expect, test } from "@playwright/test";
import { badge, resetDb, startThread, threadId } from "./helpers";

test.beforeEach(resetDb);

test("a finished thread refuses a follow-up with the 409 problem", async ({ page }) => {
  await startThread(page, "echo done", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(page.getByLabel("Message")).toBeDisabled();

  const res = await page.request.post("/agui/agents/plain", {
    headers: { Accept: "text/event-stream" },
    data: {
      threadId: threadId(page),
      runId: crypto.randomUUID(),
      messages: [{ id: crypto.randomUUID(), role: "user", content: "one more thing" }],
    },
  });
  expect(res.status()).toBe(409);
  expect(res.headers()["content-type"]).toContain("application/problem+json");
  expect(await res.json()).toMatchObject({ status: 409 });

  // nothing was appended
  await expect(
    page.getByRole("log", { name: "Conversation" }).getByText("one more thing"),
  ).toHaveCount(0);
});
