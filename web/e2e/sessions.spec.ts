import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  animationsDone,
  conversation,
  expectNoHorizontalScroll,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * Subagents in the parent's chat (child-sessions/v1, ADR 0072, web/README.md "Subagents"), against the
 * mock: the script `helpers` opens two helper sessions, which the agent's run draws as one card each
 * (a card is said again in the runs of its later events and is drawn once, in the run that opened it,
 * with the state said last), and "Open" goes to the helper's own thread.
 */

const cards = (page: import("@playwright/test").Page) =>
  conversation(page).locator('[data-slot="session-card"]');

test("each subagent is one card, in its state, and Open goes to its thread", async ({ page }) => {
  await startThread(page, "helpers");
  const parentUrl = page.url();
  await expect(cards(page)).toHaveCount(2);

  const planner = cards(page).filter({ hasText: "planner" });
  await expect(planner).toContainText("Plan the migration");
  // the late reports (a round, a block, a done, a close) were said as runs of their own: no turn for them
  await expect(planner.locator('[data-slot="session-state"]')).toHaveText("Closed");
  const researcher = cards(page).filter({ hasText: "researcher" });
  await expect(researcher).toContainText("Check the sources");
  await expect(researcher.locator('[data-slot="session-state"]')).toHaveText("Closed");
  await expect(conversation(page).locator('[data-slot="agent-turn"]')).toHaveCount(1);

  await planner.getByRole("link", { name: /^Open/ }).click();
  await expect(page).toHaveURL(THREAD_URL);
  expect(page.url()).not.toBe(parentUrl);
  await expect(page.getByRole("log", { name: "Conversation" })).toBeVisible();
});

test("the cards are keyboard-reachable and pass axe in the page's scheme, without sideways scroll", async ({
  page,
}) => {
  await startThread(page, "helpers");
  await expect(cards(page)).toHaveCount(2);
  await animationsDone(page);
  const link = cards(page).first().getByRole("link", { name: /^Open/ });
  await link.focus();
  await expect(link).toBeFocused();
  await expectNoHorizontalScroll(page);
  const { violations } = await new AxeBuilder({ page })
    .include('[data-slot="session-card"]')
    .analyze();
  expect(violations).toEqual([]);
});

test("a start a person asked for is one muted line, which turns into a link to the subagent", async ({
  page,
}) => {
  await startThread(page, "hello");
  await expect(page.getByRole("status", { name: /^Thread state:/ })).toContainText("Done");
  const parent = THREAD_URL.exec(page.url()) ? (page.url().split("/").pop() ?? "") : "";
  const answered = await page.request.post(`/api/threads/${parent}/sessions`, {
    data: { id: uuidv7(), agent: "planner", description: "Plan the docs", text: "Go." },
  });
  expect(answered.status()).toBe(202);

  const line = conversation(page).locator('[data-slot="start-line"]');
  await expect(line).toHaveCount(1);
  await expect(line).toContainText("You started planner: Plan the docs");
  const open = line.getByRole("link", { name: /^Open/ });
  await expect(open).toBeVisible();
  // the session it opened is a card of its own, once, with the words "Started by you"
  await expect(cards(page)).toHaveCount(1);
  await expect(cards(page)).toContainText("Started by you");
  await open.click();
  await expect(page).not.toHaveURL(new RegExp(`${parent}$`));
});
