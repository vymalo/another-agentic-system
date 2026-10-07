import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import {
  activityTab,
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  hideActivity,
  showActivity,
  startThread,
  turnFailedChip,
  turnSections,
  turnSummaries,
} from "./helpers";

/*
 * Nested steps (ADR 0025, web/DESIGN.md "A turn" and "Steps panel"), against the mock's `Delegate`,
 * `Investigate` and `steps-many` scenarios (mock/scripts.ts): the chat keeps one line per agent turn,
 * the tree is the panel's Activity tab. On a phone the panel is a sheet.
 */

const DELEGATE = "Delegate the login fix to the coding sub-agent";

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** A finished delegation: 20 steps, one of them a failed test run under OpenCode. */
async function delegated(page: Page, text = DELEGATE) {
  await startThread(page, text);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
}

test("a delegation is one line in the chat; the panel has the tree, one line per level", async ({
  page,
}) => {
  await delegated(page);

  // the chat: the turn's words and card as ever, and one line for its steps
  const line = turnSummaries(page);
  await expect(line).toHaveCount(1);
  await expect(line).toHaveAccessibleName(
    /^Adam's steps: 20 steps · \d+s, 1 failed\. Show in the side panel$/,
  );
  await expect(turnFailedChip(page)).toHaveText("1 failed");
  await expect(line).toHaveAttribute("aria-controls", "thread-panel");
  await expect(line).toHaveAttribute("aria-expanded", "false");
  await expect(conversation(page).getByRole("list", { name: "Steps" })).toHaveCount(0);
  await expect(conversation(page).getByText("cargo test -p auth login::")).toHaveCount(0);
  await expect(conversation(page).getByText("OpenCode", { exact: true })).toHaveCount(0);
  await expect(conversation(page).locator('[data-slot="agent-message"]')).toHaveCount(1);
  await expect(conversation(page).locator('[data-slot="pull-request-card"]')).toHaveCount(1);
  await expectNoHorizontalScroll(page);

  // the line opens the panel on this turn: its header has the focus
  await line.click();
  await expect(activityTab(page)).toBeVisible();
  await expect(line).toHaveAttribute("aria-expanded", "true");
  const turn = turnSections(page).first();
  const heading = turn.getByRole("heading", { level: 3 });
  await expect(heading).toHaveText(/^Turn 1 · Adam/);
  await expect(heading).toBeFocused();

  // depth 1 is always there; a step with children is one collapsed line that says what is inside
  await expect(turn.getByText("Started working")).toBeVisible();
  await expect(turn.getByText("Preparing the workspace")).toBeVisible();
  const opencode = turn.getByRole("button", { name: /^OpenCode/ });
  await expect(opencode).toHaveAttribute("aria-expanded", "false");
  await expect(opencode).toContainText("14 steps");
  const opencodeRow = turn
    .locator("li")
    .filter({ has: page.getByRole("button", { name: /^OpenCode/ }) })
    .first();
  await expect(opencodeRow.locator('[data-slot="failed-chip"]').first()).toHaveText("1 failed");
  await expect(turn.getByText("cargo test -p auth login::")).toHaveCount(0);

  // a click shows the latest three (and the one that failed), "Show 10 more" ten earlier ones
  await opencode.click();
  await expect(opencode).toHaveAttribute("aria-expanded", "true");
  const children = turn.getByRole("list", { name: "Steps of OpenCode" });
  await expect(children.getByRole("listitem")).toHaveCount(4);
  await expect(children.locator('[data-node-state="failed"]')).toHaveCount(1);
  await expect(children.getByText("1 failed, 41 passed")).toBeVisible();
  await expect(children.locator('[data-slot="command"]').first()).toContainText(
    "cargo test -p auth login::",
  );
  await turn.getByRole("button", { name: /^Show 10 more steps of OpenCode/ }).click();
  await expect(children.getByRole("listitem")).toHaveCount(13);
  await turn.getByRole("button", { name: /^Show 1 more steps of OpenCode/ }).click();
  await expect(children.getByRole("listitem")).toHaveCount(14);
  await expectNoHorizontalScroll(page);

  // closing the turn and the step is the same button
  await opencode.click();
  await expect(children).toHaveCount(0);

  // a reload is a replay: the same line, the same tree
  await hideActivity(page);
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(turnSummaries(page)).toHaveCount(1);
  await showActivity(page);
  await expect(turnSections(page)).toHaveCount(1);
  await expect(turnSections(page).getByRole("button", { name: /^OpenCode/ })).toContainText(
    "14 steps",
  );
});

test("each turn's line opens its own turn in the panel, and asking again focuses it again", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await delegated(page);
  await page.getByLabel("Message").fill("Delegate the docs fix too");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(turnSummaries(page)).toHaveCount(2, { timeout: 30_000 });
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });

  const [first, second] = [turnSummaries(page).nth(0), turnSummaries(page).nth(1)];
  await first.click();
  await expect(turnSections(page)).toHaveCount(2);
  const heading = (n: number) => turnSections(page).nth(n).getByRole("heading", { level: 3 });
  await expect(heading(0)).toHaveText(/^Turn 1 · Adam/);
  await expect(heading(0)).toBeFocused();
  await expect(first).toHaveAttribute("aria-expanded", "true");
  await expect(second).toHaveAttribute("aria-expanded", "false");
  // the turn it was asked about is open
  await expect(
    turnSections(page)
      .nth(0)
      .getByRole("button", { name: /^OpenCode/ }),
  ).toBeVisible();

  await hideActivity(page);
  await second.click();
  await expect(heading(1)).toHaveText(/^Turn 2 · Adam/);
  await expect(heading(1)).toBeFocused();
  await expect(second).toHaveAttribute("aria-expanded", "true");

  // the person moved on; the same line asks again, and the focus comes back
  await heading(0).click();
  await expect(heading(1)).not.toBeFocused();
  await hideActivity(page);
  await second.click();
  await expect(heading(1)).toBeFocused();
});

test("a level of more than 50 steps is a scroll box that draws only the rows in view", async ({
  page,
}) => {
  await delegated(page, "steps-many please");
  await expect(turnSummaries(page)).toHaveAccessibleName(
    /^Adam's steps: 122 steps · .*, 1 failed\. Show in the side panel$/,
  );
  await turnSummaries(page).click();
  const turn = turnSections(page).first();
  const opencode = turn.getByRole("button", { name: /^OpenCode/ });
  await expect(opencode).toContainText("120 steps");
  await opencode.click();
  // 3 latest, then ten more at a time: past 50 the level is a scroll box
  const more = turn.getByRole("button", { name: /^Show 10 more steps of OpenCode/ });
  for (let i = 0; i < 5; i++) await more.click();
  const box = turn.locator('[data-slot="steps-scroll"]');
  await expect(box).toBeVisible();
  await expect(box).toHaveAccessibleName("Steps of OpenCode, scrolls");
  // the failed read, always listed, is the first row; only a window of the 54 rows is in the page
  await expect(box.getByText("read the file that is missing")).toBeVisible();
  const drawn = await box.getByRole("listitem").count();
  expect(drawn).toBeGreaterThan(5);
  expect(drawn).toBeLessThan(40);
  expect((await box.boundingBox())?.height).toBeLessThanOrEqual(361);
  // scrolling brings the rest in
  await box.evaluate((el) => {
    el.scrollTop = el.scrollHeight;
  });
  await expect(box.getByText("read src/module_119.rs")).toBeVisible();
  await expectNoHorizontalScroll(page);
});

test("while OpenCode works the line spins and names its step; Stop ends it and nothing spins", async ({
  page,
}) => {
  await startThread(page, "Investigate the login redirect");
  const line = turnSummaries(page);
  await expect(line).toHaveCount(1);
  await expect(line).toContainText("Running cargo test -p auth · 10 steps");
  await expect(line.locator('[data-glyph="spinner"]')).toBeVisible();

  // the pane follows the work: the live turn is open, and its sub-agent spins
  const tab = await showActivity(page);
  await expect(tab.locator('[data-slot="step"][data-state="live"]').first()).toBeVisible();
  await expect(
    turnSections(page)
      .first()
      .getByRole("button", { name: /^(Running: )?OpenCode/ }),
  ).toContainText("7 steps");
  await hideActivity(page);

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  // the stop is a step of its own: "Stopped", and the run's failed test run is still said
  await expect(line).toContainText("Stopped · 11 steps");
  await expect(turnFailedChip(page)).toHaveText("1 failed");
  await expect(line.locator('[data-glyph="spinner"]')).toHaveCount(0);
  await showActivity(page);
  await expect(activityTab(page).locator('[data-state="live"]')).toHaveCount(0);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the Activity tab with the tree open, the chat with its line", async ({ page }) => {
      await delegated(page);
      expect(await axeViolations(page)).toEqual([]);
      await turnSummaries(page).click();
      const turn = turnSections(page).first();
      await turn.getByRole("button", { name: /^OpenCode/ }).click();
      await expect(turn.getByRole("list", { name: "Steps of OpenCode" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a scroll box of steps has no serious violations", async ({ page }) => {
      await delegated(page, "steps-many please");
      await turnSummaries(page).click();
      const turn = turnSections(page).first();
      await turn.getByRole("button", { name: /^OpenCode/ }).click();
      for (let i = 0; i < 5; i++) {
        await turn.getByRole("button", { name: /^Show 10 more steps of OpenCode/ }).click();
      }
      await expect(turn.locator('[data-slot="steps-scroll"]')).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: a turn that is running, and its line", async ({ page }) => {
      await startThread(page, "Investigate the login redirect");
      await expect(turnSummaries(page)).toContainText("Running cargo test -p auth");
      await showActivity(page);
      await expect(turnSections(page).first()).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await hideActivity(page);
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });
  });
}
