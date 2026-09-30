import { expect, test } from "@playwright/test";
import { allCalls, badge, conversation, resetDb, startThread } from "./helpers";

test.beforeEach(resetDb);

const surface = (page: import("@playwright/test").Page) =>
  conversation(page).getByRole("region", { name: /^Interface from plain/ });

test("a surface from the agent is drawn, and its button reaches the agent as an action on the same task", async ({
  page,
}) => {
  await startThread(page, "ui pick one", "Plain");
  await expect(badge(page)).toHaveText("Your turn");

  const ui = surface(page);
  await expect(ui).toBeVisible();
  await expect(ui.getByText("Pick one")).toBeVisible();
  await expect(ui.locator('[data-slot="actor-label"]')).toHaveText("plain");
  const go = ui.getByRole("button", { name: "Go" });
  await expect(go).toBeEnabled();

  // the agent's question is open (an interrupt): the button answers it, with no message
  const posts: Record<string, unknown>[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST" && r.url().includes("/agui/agents/")) {
      posts.push(r.postDataJSON() as Record<string, unknown>);
    }
  });
  await go.click();
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("answered: ui-action go")).toHaveCount(1);
  await expect(conversation(page).getByText("Chose")).toBeVisible();
  expect(posts).toHaveLength(1);
  expect(posts[0]).toMatchObject({
    messages: [],
    forwardedProps: { a2uiAction: { userAction: { name: "go", surfaceId: "s1" } } },
  });
  expect(posts[0]?.resume).toBeUndefined();

  // the agent saw the action as a message on the task it had asked its question on
  const executions = (await allCalls(page.request, "plain")).filter(
    (c) => c.kind === "execute" && ["ui pick one", "ui-action go"].includes(c.text),
  );
  expect(executions.map((c) => c.text)).toEqual(["ui pick one", "ui-action go"]);
  expect(executions[1]?.taskId).toBe(executions[0]?.taskId);
  expect(executions.map((c) => c.resuming)).toEqual([false, true]);

  // finished: the surface is still there, read-only, also after a reload
  await expect(ui.getByRole("button", { name: "Go" })).toBeDisabled();
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(surface(page).getByRole("button", { name: "Go" })).toBeDisabled();
  await expect(conversation(page).getByRole("region", { name: /^Interface from/ })).toHaveCount(1);
});

test("a surface the agent deletes is not shown", async ({ page }) => {
  await startThread(page, "ui-delete now", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByRole("region", { name: /^Interface from/ })).toHaveCount(0);
});

test("a payload the orchestrator refuses shows as an error line, and no surface", async ({
  page,
}) => {
  await startThread(page, "ui-bad now", "Plain");
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText(/Error:/)).toBeVisible();
  await expect(conversation(page).getByRole("region", { name: /^Interface from/ })).toHaveCount(0);
});
