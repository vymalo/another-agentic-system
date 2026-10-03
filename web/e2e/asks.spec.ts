import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import {
  animationsDone,
  badge,
  expectNoHorizontalScroll,
  hideActivity,
  MOCK_URL,
  showActivity,
  startThread,
  turnFailedChip,
  turnSections,
  turnSummaries,
} from "./helpers";

/*
 * Asked agents (ADR 0026, web/DESIGN.md "Asked agents"), against the mock's `ask-agent` and `ask-hold`
 * scenarios (mock/scripts.ts): the thread's agent (Coder) asks the Reviewer, which asks the Verifier
 * (that one searches the web, a step under its ask), both answer; then it asks the Verifier again and
 * that one fails. Each ask is a collapsed line "Asked <name>" in the panel's Activity tab, under the
 * step or the ask that asked. On a phone the panel is a sheet.
 */

const ASK = "ask-agent coordinate the review";
const HOLD = "ask-hold coordinate the review";

const threadId = (page: Page) => /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1] ?? "";

async function release(page: Page) {
  const res = await fetch(`${MOCK_URL}/__mock/release?thread=${threadId(page)}`, {
    method: "POST",
  });
  expect(res.status).toBe(204);
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** The ask's line: the button that opens it, by the agent's name. */
const askLine = (scope: ReturnType<Page["locator"]>, name: string) =>
  scope.getByRole("button", { name: new RegExp(`^Asked ${name}`) });
/** The row of a line: the `<li>` that holds it and what is under it. */
const rowOf = (line: ReturnType<Page["locator"]>) => line.locator("xpath=ancestor::li[1]");

/** The story played to its end: Coder's turn with two asks, one of which failed. */
async function asked(page: Page) {
  await startThread(page, ASK);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
}

test("an ask is one collapsed line, 'Asked Reviewer', with its answer and the ask it made under it", async ({
  page,
}) => {
  await asked(page);

  // the chat keeps its one line for the turn: six steps (the Verifier's search and the result among them), one failed
  const line = turnSummaries(page);
  await expect(line).toHaveCount(1);
  await expect(line).toHaveAccessibleName(
    /^Coder's steps: 6 steps · \d+s, 1 failed\. Show in the side panel$/,
  );
  await expect(turnFailedChip(page)).toHaveText("1 failed");
  await line.click();
  const turn = turnSections(page).first();
  await expect(turn.getByRole("heading", { level: 3 })).toHaveText(/^Turn 1 · Coder/);
  await expect(turn.getByRole("heading", { level: 3 }).getByText("1 failed")).toBeVisible();

  // depth 1: the two asks of the thread's agent, closed; the researcher's is not on the page
  const reviewer = askLine(turn, "Reviewer");
  await expect(reviewer).toHaveAttribute("aria-expanded", "false");
  await expect(reviewer).toHaveText("Asked Reviewer: Answered· 2 steps");
  await expect(rowOf(reviewer)).toHaveAttribute("data-ask-state", "completed");
  await expect(turn.getByText("Check the claims in the plan against the sources.")).toHaveCount(0);

  // the failed one says so on its own line, with why, closed
  const failed = turn.locator('[data-slot="step"][data-ask-state="failed"]');
  await expect(failed).toHaveCount(1);
  await expect(askLine(failed, "Verifier")).toHaveText("Asked Verifier: Failed");
  await expect(failed.locator('[data-slot="ask-note"]')).toHaveText(
    "Why: the verifier did not answer: connection refused",
  );

  // open the Reviewer: what it was asked and what it answered, and the Verifier it asked under it
  await reviewer.click();
  await expect(reviewer).toHaveAttribute("aria-expanded", "true");
  const details = rowOf(reviewer).locator('[data-slot="ask-details"]').first();
  await expect(
    details.getByText("Review the plan for the parser and say what is missing."),
  ).toBeVisible();
  await expect(
    details.getByText("The plan holds. Missing: a test for the empty input."),
  ).toBeVisible();
  const nested = askLine(rowOf(reviewer), "Verifier").first();
  await expect(nested).toHaveText("Asked Verifier: Answered· 1 step");
  await nested.click();
  const inner = rowOf(nested);
  await expect(inner.getByText("The claims hold: the sources agree with the plan.")).toBeVisible();
  await expect(inner.getByRole("link", { name: "sources" })).toHaveAttribute(
    "href",
    "https://github.com/acme/demo/pull/1",
  );
  // the search the Verifier made is a step under its ask
  await expect(inner.getByRole("list", { name: "Steps of Asked Verifier" })).toContainText(
    "Web search",
  );
  await expectNoHorizontalScroll(page);

  // closing is the same button
  await reviewer.click();
  await expect(reviewer).toHaveAttribute("aria-expanded", "false");
  await expect(turn.getByText("The claims hold")).toHaveCount(0);

  // a reload replays the same tree
  await hideActivity(page);
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  await expect(askLine(turnSections(page).first(), "Reviewer")).toHaveText(
    "Asked Reviewer: Answered· 2 steps",
  );
  await expect(
    turnSections(page).first().locator('[data-slot="step"][data-ask-state="failed"]'),
  ).toHaveCount(1);
});

test("while the asks run: a spinner on each, the Verifier nested in the Reviewer, the chat's line on the deepest step", async ({
  page,
}) => {
  await startThread(page, HOLD);
  // the line says the deepest step that runs: the Verifier's search, under its ask
  await expect(turnSummaries(page)).toContainText("Web search");
  await expect(turnSummaries(page)).toContainText("4 steps");
  await showActivity(page);
  const turn = turnSections(page).first();
  const reviewer = askLine(turn, "Reviewer");
  await expect(reviewer).toHaveText("Asked Reviewer: Working· 2 steps");
  await expect(rowOf(reviewer)).toHaveAttribute("data-state", "live");
  await expect(rowOf(reviewer).locator("svg.lucide-loader-circle").first()).toBeVisible();
  await reviewer.click();
  const verifier = askLine(rowOf(reviewer), "Verifier").first();
  await expect(verifier).toHaveText("Asked Verifier: Working· 1 step");
  await expect(rowOf(verifier)).toHaveAttribute("data-state", "live");
  await verifier.click();
  // its search runs too, under its ask
  await expect(rowOf(verifier).locator('[data-node-state="running"]')).toHaveCount(1);

  // the run goes on: both asks end answered, the next ask fails, the spinners are gone
  await release(page);
  await hideActivity(page);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await showActivity(page);
  await expect(turn.locator('[data-slot="step"][data-ask-state="failed"]')).toHaveCount(1);
  await expect(turn.locator("li svg.lucide-loader-circle")).toHaveCount(0);
  await expect(reviewer).toHaveText("Asked Reviewer: Answered· 2 steps");
  await expect(turnFailedChip(page)).toHaveText("1 failed");
});

test("Stop ends the asks that run as Stopped, deepest first: no spinner stays", async ({
  page,
}) => {
  await startThread(page, HOLD);
  await showActivity(page);
  const turn = turnSections(page).first();
  await expect(askLine(turn, "Reviewer")).toHaveText("Asked Reviewer: Working· 2 steps");
  // on a phone the sheet is in front of the page, and Stop is the page's
  await hideActivity(page);
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped", { timeout: 30_000 });
  await showActivity(page);
  await expect(askLine(turn, "Reviewer")).toHaveText("Asked Reviewer: Stopped· 2 steps");
  await expect(turn.locator("li svg.lucide-loader-circle")).toHaveCount(0);
  await askLine(turn, "Reviewer").click();
  const nested = askLine(turn, "Verifier");
  await expect(nested).toHaveText("Asked Verifier: Stopped· 1 step");
  await expect(rowOf(nested).locator('[data-slot="ask-note"]')).toHaveText(
    "Why: the asking task ended",
  );
});

test("the keyboard does it all: Tab to a line, Enter opens it, Space closes it", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "a physical keyboard");
  await asked(page);
  await turnSummaries(page).click();
  const turn = turnSections(page).first();
  const reviewer = askLine(turn, "Reviewer");
  await reviewer.focus();
  await expect(reviewer).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(reviewer).toHaveAttribute("aria-expanded", "true");
  await expect(reviewer).toBeFocused();
  await page.keyboard.press("Space");
  await expect(reviewer).toHaveAttribute("aria-expanded", "false");
  await expect(reviewer).toBeFocused();
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the asks, closed and open, the failed one and the chat's line", async ({ page }) => {
      await asked(page);
      expect(await axeViolations(page)).toEqual([]);
      await turnSummaries(page).click();
      const turn = turnSections(page).first();
      expect(await axeViolations(page)).toEqual([]);
      await askLine(turn, "Reviewer").click();
      await askLine(turn, "Verifier").first().click();
      await expect(turn.getByText("The claims hold")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: the asks while they run", async ({ page }) => {
      await startThread(page, HOLD);
      await showActivity(page);
      const turn = turnSections(page).first();
      await askLine(turn, "Reviewer").click();
      await expect(askLine(turn, "Verifier")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await release(page);
      await hideActivity(page);
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
    });
  });
}
