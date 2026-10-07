import { expect, test } from "@playwright/test";
import {
  activityTab,
  badge,
  conversation,
  expectNoHorizontalScroll,
  hideActivity,
  showActivity,
  startThread,
  threadList,
  turnSummaries,
} from "./helpers";

/*
 * The turn as a classical chat (web/DESIGN.md), against the mock's coder scenarios
 * (mock/scripts.ts: `Fix …`, `Refactor …`, `Make …`, `Upgrade …`, `Deploy …`). The conversation has
 * the agent's words, its cards and one line for its steps; the steps are the panel's Activity tab.
 */

/** The steps of the last turn, in the panel's Activity tab (shown first). */
const steps = (page: import("@playwright/test").Page) =>
  activityTab(page).getByRole("list", { name: /^Steps of turn \d+$/ });

test("a coder's run: its steps in words, its answer as prose, its pull request as a card", async ({
  page,
}) => {
  await startThread(page, "Fix the redirect loop after signing in");
  await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
  // the conversation says it in one line
  await expect(turnSummaries(page)).toHaveCount(1);
  await expect(conversation(page).getByRole("list", { name: "Steps" })).toHaveCount(0);
  await expect(conversation(page).getByText("Preparing the workspace")).toHaveCount(0);
  await showActivity(page);
  const list = steps(page);
  await expect(list.getByText("Started working")).toBeVisible();
  await expect(list.getByText("Preparing the workspace", { exact: true })).toBeVisible();
  // a command is monospace, the prompt is not part of its text
  await expect(list.locator('[data-slot="command"]').first()).toContainText(
    "cargo test -p auth login::",
  );
  await expect(list.getByText("Pushed")).toBeVisible();
  await expect(list.locator('[data-slot="branch-chip"]')).toHaveText("agent/fix-login-redirect");
  await expect(list.getByText("Checks passed")).toBeVisible();
  await expect(list.getByText("Opened pull request #12")).toBeVisible();
  // every finished step is a check, none spins: the run is over
  await expect(list.locator('[data-slot="step"][data-state="live"]')).toHaveCount(0);

  // the agent's words, once, rendered as markdown (no status line repeats them)
  const words = conversation(page).locator('[data-slot="agent-message"]');
  await expect(words).toHaveCount(1);
  await expect(words.getByRole("heading")).toHaveCount(0);
  await expect(words.locator("strong", { hasText: "What was wrong:" })).toBeVisible();
  await expect(words.locator("pre code")).toContainText('if next.starts_with("/login")');
  await expect(conversation(page).getByText("Completed")).toHaveCount(0);

  const card = conversation(page).locator('[data-slot="pull-request-card"]');
  await expect(card.getByText("Fix the redirect loop after signing in")).toBeVisible();
  await expect(card.getByText("acme/demo#12", { exact: true })).toBeVisible();
  const open = card.getByRole("link", { name: /View pull request acme\/demo#12/ });
  await expect(open).toHaveAttribute("href", "https://github.com/acme/demo/pull/12");
  await expect(open).toHaveAttribute("target", "_blank");
  await expect(open).toHaveAttribute("rel", "noopener noreferrer");
  await expectNoHorizontalScroll(page);
});

test("while the coder runs, its current step spins and the composer offers Stop", async ({
  page,
}) => {
  await startThread(page, "Refactor the session store behind a trait");
  // the line names the step the agent is on and spins
  await expect(turnSummaries(page)).toContainText("Running cargo test -p auth login::");
  await expect(turnSummaries(page).locator('[data-glyph="spinner"]')).toBeVisible();
  await showActivity(page);
  const live = steps(page).locator('[data-slot="step"][data-state="live"]');
  await expect(live).toHaveCount(1);
  await expect(live).toContainText("cargo test -p auth login::");
  await hideActivity(page);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await showActivity(page);
  await expect(live).toHaveCount(0);
  await expect(steps(page).getByText("Stopped", { exact: true })).toBeVisible();
  await expect(turnSummaries(page)).toContainText("Stopped ·");
});

test("before the first event the agent is starting", async ({ page }) => {
  await startThread(page, "Upgrade the dependencies");
  await expect(conversation(page).getByText("adam is starting…", { exact: false })).toBeVisible();
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(conversation(page).getByText("is starting…", { exact: false })).toHaveCount(0);
});

test("checks that fail send the coder back: a rework step, then the second try passes", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "Make sessions expire after 30 idle minutes");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await showActivity(page);
  const rework = steps(page).locator('[data-slot="rework-step"]');
  await expect(rework).toHaveCount(1);
  await expect(rework).toContainText("Checks failed — trying again (2/3)");
  await expect(rework).toContainText("1 finding");
  // the failed check holds the finding, folded until asked for
  const failed = steps(page).getByRole("listitem", {
    name: "Check: Agent checks, attempt 1, failed",
  });
  const finding = failed.getByText("session::expires_after_idle: expected 401, got 200");
  await expect(finding).toBeHidden();
  await failed.getByText("Findings (1)").click();
  await expect(finding).toBeVisible();
  await expect(
    steps(page).getByRole("listitem", { name: "Check: Agent checks, attempt 2, passed" }),
  ).toBeVisible();
  // the pull request is a card in the conversation, not a step
  await hideActivity(page);
  await expect(conversation(page).locator('[data-slot="pull-request-card"]')).toHaveCount(1);
});

test("the agent's question waits for a reply, under it and in the box", async ({ page }) => {
  await startThread(page, "Deploy the new login page");
  await expect(badge(page)).toHaveText("Your turn");
  const question = conversation(page).locator('[data-slot="agent-message"]').last();
  await expect(question).toContainText("Should I deploy to staging or straight to production?");
  await expect(question.getByText("Waiting for your reply")).toBeVisible();
  await expect(page.getByLabel("Message")).toHaveAttribute("placeholder", "Reply…");
  await page.getByLabel("Message").fill("staging");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("Waiting for your reply")).toHaveCount(0);
  await expect(
    conversation(page).getByText("Deployed to staging.", { exact: false }),
  ).toBeVisible();
});

test("a suggestion puts its words in the box, and sends nothing", async ({ page }) => {
  await page.goto("/");
  const requests: string[] = [];
  page.on("request", (r) => {
    if (r.method() === "POST") requests.push(r.url());
  });
  await page.getByRole("button", { name: "Write tests for the payment module" }).click();
  await expect(page.getByLabel("Message")).toHaveValue("Write tests for the payment module");
  await expect(page.getByLabel("Message")).toBeFocused();
  expect(requests).toEqual([]);
});

test("desktop: the sidebar closes and opens, and remembers it", async ({ page, isMobile }) => {
  test.skip(isMobile, "a phone has the sheet");
  await startThread(page, "echo remember the sidebar");
  await expect(badge(page)).toHaveText("Done");
  await expect(threadList(page)).toBeVisible();
  await page.getByRole("button", { name: "Close sidebar" }).click();
  await expect(threadList(page)).toBeHidden();
  // the focus moves to the control that now exists
  await expect(page.getByRole("button", { name: "Open sidebar" })).toBeFocused();
  await page.reload();
  // marked before the first paint (no flash of an open sidebar), and hidden all along
  await expect(page.locator("html")).toHaveAttribute("data-sidebar", "closed");
  await expect(threadList(page)).toBeHidden();
  await expect(badge(page)).toHaveText("Done");
  await expect(threadList(page)).toBeHidden();
  await page.getByRole("button", { name: "Open sidebar" }).click();
  await expect(threadList(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "Close sidebar" })).toBeFocused();
  await expect(page.locator("html")).not.toHaveAttribute("data-sidebar", "closed");
  // the open thread is the current page of the list
  await expect(threadList(page).locator('a[aria-current="page"]')).toHaveText(
    "echo remember the sidebar",
  );
  await expect(threadList(page).getByText("Today")).toBeVisible();
});
