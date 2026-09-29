import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, errorLine, startThread } from "./helpers";

const problem = (status: number, title: string, detail: string) => ({
  status,
  contentType: "application/problem+json",
  body: JSON.stringify({ title, status, detail }),
});

async function failNext(
  page: Page,
  urlGlob: string,
  method: string,
  status: number,
  detail: string,
) {
  await page.route(urlGlob, (route) =>
    route.request().method() === method
      ? route.fulfill(problem(status, "Failure", detail))
      : route.fallback(),
  );
}

test("a failed agent list shows the problem and Retry loads it", async ({ page }) => {
  await failNext(page, "**/api/agents", "GET", 500, "the agent directory is down");
  await page.goto("/");
  const alert = errorLine(page);
  await expect(alert).toContainText("Could not load agents: the agent directory is down");

  await page.unroute("**/api/agents");
  await alert.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByLabel("Agent")).toBeVisible();
  await expect(errorLine(page)).toHaveCount(0);
});

test("a rejected new thread shows the problem and keeps the text", async ({ page }) => {
  await failNext(page, "**/agui/agents/*", "POST", 400, "text must be 1 to 100000 characters");
  await page.goto("/");
  await expect(page.getByLabel("Agent")).toBeVisible();
  await page.getByLabel("Message").fill("keep me");
  await page.getByRole("button", { name: "Send" }).click();

  await expect(errorLine(page)).toContainText("text must be 1 to 100000 characters");
  await expect(page.getByLabel("Message")).toHaveValue("keep me");
  await expect(page).toHaveURL(/\/$/);
});

test("a failing follow-up shows the problem and keeps the text", async ({ page }) => {
  await startThread(page, "ask pick a branch");
  await expect(badge(page)).toHaveText("Waiting for you");

  await failNext(page, "**/agui/agents/*", "POST", 503, "the store is unavailable");
  await page.getByLabel("Message").fill("main");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(errorLine(page)).toContainText("the store is unavailable");
  await expect(page.getByLabel("Message")).toHaveValue("main");
  await expect(badge(page)).toHaveText("Waiting for you");
});

test("an answer the orchestrator refuses with 409 shows its reason and keeps the text", async ({
  page,
}) => {
  await startThread(page, "ask pick a branch");
  await expect(badge(page)).toHaveText("Waiting for you");

  await failNext(
    page,
    "**/agui/agents/*",
    "POST",
    409,
    "the thread is finished (Done); start a new thread",
  );
  await page.getByLabel("Message").fill("late answer");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(errorLine(page)).toContainText("the thread is finished (Done); start a new thread");
  await expect(page.getByLabel("Message")).toHaveValue("late answer");
  // the refused answer is not left in the transcript
  await expect(conversation(page).getByText("late answer", { exact: true })).toHaveCount(0);
});

test("an unknown thread shows the not-found message", async ({ page }) => {
  await page.goto("/threads/00000000-0000-4000-8000-000000000000");
  await expect(page.getByText("Thread not found.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Start a new thread" })).toBeVisible();
});
