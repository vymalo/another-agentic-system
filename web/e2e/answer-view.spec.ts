import AxeBuilder from "@axe-core/playwright";
import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  activityTab,
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  hideActivity,
  showActivity,
  startThread,
  turnSections,
  turnSummaries,
} from "./helpers";

/*
 * The answer in the chat, the working text in Activity (ADR 0031, web/DESIGN.md "A turn"), against
 * the mock's `coder-notes` scenarios (mock/scripts.ts): the owner's coder chat of 2026-10-02 in its
 * shape. The coder says a sentence before each tool call (six of them), a test run fails, a
 * surface is drawn on the way, and the turn ends with one answer. The chat's column holds that
 * answer and the surface and nothing of the six sentences; they are notes among the steps of the
 * Activity tab (a sheet on a phone), in the order they were said. `coder-notes-legacy` is the same
 * turn with no word marked, as an older log says it; `coder-notes-hold` is it still working.
 */

const NOTES = [
  "I'll build something small in a scratch project first, then show you the export.",
  "Your screen draws text, cards and diagrams, not images. So Node draws it and I show its shapes.",
  "Now the tests for both.",
  "Node 24 wants an explicit glob for the test directory. I'll fix the script, not the tests.",
  "All 7 tests pass. Now I'll export the drawing.",
  "Here is the drawing. Your screen has no images, so these are its shapes.",
];
const ANSWER = "The drawing is exported, and its shape is in the cards above.";

/** What the agent says in the column: its answer, one per turn. */
const answers = (page: Page) => conversation(page).locator('[data-slot="agent-message"]');
const agentTurns = (page: Page) => conversation(page).locator('[data-slot="agent-turn"]');
const ticker = (page: Page) => conversation(page).locator('[data-slot="turn-ticker"]');
const notes = (turn: Locator) => turn.locator('li[data-kind="note"]');

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** The column of a finished owner's chat: one answer, the surface, no sentence said on the way. */
async function expectOneAnswer(page: Page) {
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(answers(page)).toHaveCount(1);
  await expect(answers(page)).toContainText(ANSWER);
  await expect(agentTurns(page)).toHaveCount(1);
  for (const note of NOTES) await expect(conversation(page).getByText(note)).toHaveCount(0);
  // nothing is folded away in the column either: no "show more", no disclosure of the sentences
  await expect(
    conversation(page).getByRole("button", {
      name: /^(show|hide) (more|less)|thinking|working note/i,
    }),
  ).toHaveCount(0);
  // a surface drawn on the way is an output of the turn: it stays
  await expect(conversation(page).getByText("What the drawing holds")).toBeVisible();
  // the line counts steps, not notes: a status, eight tools, `show` and a read, one that failed
  await expect(turnSummaries(page)).toHaveAccessibleName(/^Coder's steps: 11 steps.*1 failed/);
  await expectNoHorizontalScroll(page);
}

/** The Activity tab of the one turn: the six sentences in order, among the steps they announced. */
async function expectNotesInActivity(page: Page) {
  await showActivity(page);
  const turn = turnSections(page).first();
  await expect(notes(turn)).toHaveCount(6);
  for (const [i, note] of NOTES.entries()) await expect(notes(turn).nth(i)).toContainText(note);
  // in time order among the steps: each sentence before the tool call it announced
  const kinds = await turn
    .locator('li[data-kind="note"], li[data-kind="tool"]')
    .evaluateAll((rows) =>
      rows.map((r) =>
        r.getAttribute("data-kind") === "note"
          ? "note"
          : (r.querySelector('[data-slot="step-toggle"]')?.textContent ?? r.textContent ?? ""),
      ),
    );
  expect(kinds.map((k) => (k === "note" ? "note" : "tool"))).toEqual([
    "note",
    "tool",
    "note",
    "tool",
    "tool",
    "note",
    "tool",
    "tool",
    "note",
    "tool",
    "tool",
    "note",
    "tool",
    "note",
    "tool",
    "tool",
  ]);
  // a screen reader reads each as what it is, in order
  await expect(notes(turn).first()).toContainText("Working note:");
}

for (const scenario of ["coder-notes", "coder-notes-legacy"] as const) {
  test(`${scenario}: one answer in the chat, six notes in Activity, in order`, async ({ page }) => {
    test.setTimeout(60_000);
    await startThread(page, `${scenario} draw something in node`);
    await expectOneAnswer(page);
    await expectNotesInActivity(page);
    await hideActivity(page);

    // a replay is the same: a reload reads the log, which the legacy turn has no marks in either
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expectOneAnswer(page);
  });
}

test("every turn of a conversation has exactly one answer in the column", async ({ page }) => {
  test.setTimeout(60_000);
  await startThread(page, "coder-notes draw something in node");
  await expectOneAnswer(page);
  await page.getByLabel("Message").fill("Also add an entry to the changelog");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("Added a changelog entry")).toBeVisible({
    timeout: 20_000,
  });
  await expect(badge(page)).toHaveText("Done");
  await expect(agentTurns(page)).toHaveCount(2);
  await expect(answers(page)).toHaveCount(2);
  for (const note of NOTES) await expect(conversation(page).getByText(note)).toHaveCount(0);
});

test("nothing in the column is hidden and still reachable: no focusable element behind a hidden one", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "coder-notes draw something in node");
  await expectOneAnswer(page);
  const stray = await conversation(page).evaluate((log) =>
    [...log.querySelectorAll<HTMLElement>("[hidden], [aria-hidden='true'], [inert]")]
      .filter((el) =>
        el.querySelector(
          "button, a[href], input, textarea, select, [tabindex]:not([tabindex='-1'])",
        ),
      )
      .map((el) => el.outerHTML.slice(0, 120)),
  );
  expect(stray).toEqual([]);
  // the keyboard goes through the column and meets the line, the actions and the surface: never a sentence
  const reachable = await conversation(page).evaluate((log) =>
    [...log.querySelectorAll<HTMLElement>("button, a[href], [tabindex]:not([tabindex='-1'])")].map(
      (el) => el.textContent ?? "",
    ),
  );
  for (const note of NOTES) expect(reachable.join("\n")).not.toContain(note);
});

test("while the turn works the line shows the last working sentence, quietly; the column has no answer yet", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "coder-notes-hold draw something in node");
  await expect(ticker(page)).toHaveText("All 7 tests pass. Now I'll export the drawing.", {
    timeout: 30_000,
  });
  // a line, under the turn's line, that is not a control and not a live region
  await expect(ticker(page)).not.toHaveAttribute("aria-live", /.*/);
  expect(await ticker(page).evaluate((el) => el.closest("[aria-live]") === null)).toBe(true);
  expect(
    await ticker(page).evaluate((el) => el.querySelector("button, a, [tabindex]") === null),
  ).toBe(true);
  const box = await ticker(page).boundingBox();
  expect(box?.height ?? 0, "one line of 12 px text").toBeLessThan(26);
  // the line above it stays one line: the ticker does not squeeze it
  const line = await turnSummaries(page).boundingBox();
  expect(line?.height ?? 0, "the turn's line is one line of 28 px").toBeLessThan(34);
  const chip = await conversation(page).locator('[data-slot="failed-chip"]').first().boundingBox();
  expect(chip?.height ?? 0, "the failed chip is one line").toBeLessThan(26);
  await expectNoHorizontalScroll(page);
  await expect(answers(page)).toHaveCount(0);
  for (const note of NOTES.slice(0, 4))
    await expect(conversation(page).getByText(note)).toHaveCount(0);
  // the notes said so far are in Activity
  await showActivity(page);
  await expect(notes(turnSections(page).first())).toHaveCount(5);
  await hideActivity(page);

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(ticker(page)).toHaveCount(0);
});

test("the words said before a tool call, streamed or not, are a note in Activity and the one reply stays", async ({
  page,
}) => {
  test.setTimeout(60_000);
  await startThread(page, "stream-words go");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  // the answer is the one message, however the words came
  await expect(answers(page)).toHaveCount(1);
  await expect(answers(page)).toHaveText("Streaming a reply, word by word, as it is written.");
  await expect(conversation(page).getByText("Let me run the tests first.")).toHaveCount(0);
  await showActivity(page);
  await expect(notes(turnSections(page).first())).toHaveText(/Let me run the tests first\./);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the column with one answer, and Activity with its notes", async ({ page }) => {
      test.setTimeout(60_000);
      await startThread(page, "coder-notes draw something in node");
      await expectOneAnswer(page);
      expect(await axeViolations(page)).toEqual([]);

      await expectNotesInActivity(page);
      // the long note opened, as a person reads it
      const turn = turnSections(page).first();
      const more = turn.getByRole("button", { name: "Show more" });
      if ((await more.count()) > 0) await more.first().click();
      expect(await axeViolations(page)).toEqual([]);
      await expect(activityTab(page)).toBeVisible();
    });

    test("axe: the line with its ticker, while the turn works", async ({ page }) => {
      test.setTimeout(60_000);
      await startThread(page, "coder-notes-hold draw something in node");
      await expect(ticker(page)).toBeVisible({ timeout: 30_000 });
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });
  });
}
