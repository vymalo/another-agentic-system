import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { animationsDone, badge, conversation, startThread } from "./helpers";
import {
  cancels,
  endChunk,
  endVoice,
  failChunk,
  failVoice,
  pending,
  removeVoice,
  speaking,
  spoken,
  stubVoice,
  voiceLog,
} from "./voice";

/*
 * Read aloud (ADR 0067 S1), against the mock orchestrator in a real browser: a button under an agent's answer that reads
 * it with the device's own voice (`window.speechSynthesis`), the words the page shows and not the Markdown, Stop, one
 * answer at a time across the page, a refusal said in words, a failure half way through an answer, and no button at all
 * in a browser without a voice. The voice is the queue of `e2e/voice.ts`: headless Chromium has no voice to say anything
 * with.
 */

const turn = (page: Page, at = 0) => conversation(page).locator('[data-slot="agent-turn"]').nth(at);
// its name never changes: `aria-pressed` carries the state, and the tooltip says what a press does
const readAloud = (page: Page, at = 0) =>
  turn(page, at).getByRole("button", { name: "Read aloud" });
const reading = (page: Page, at = 0) =>
  expect(readAloud(page, at)).toHaveAttribute("aria-pressed", "true");
const idle = (page: Page, at = 0) =>
  expect(readAloud(page, at)).toHaveAttribute("aria-pressed", "false");

/** A finished thread whose answer is the mock's `code-math`: prose, two fenced blocks and two display formulas. */
async function codeMath(page: Page) {
  await startThread(page, "code-math please", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  await expect(turn(page)).toContainText("Here is the function");
}

test("reads the answer's visible words: no Markdown marks, no fence, no URL, a formula as a word", async ({
  page,
}) => {
  await stubVoice(page);
  await codeMath(page);
  await readAloud(page).click();
  await reading(page);
  await expect.poll(() => speaking(page)).not.toBeNull();
  const said = (await spoken(page)).join(" ");
  // the words on the screen
  expect(said).toContain("Here is the function and the rule it follows.");
  expect(said).toContain("fn total(prices: &[u32]) -> u32 {");
  expect(said).toContain("The two plans cost $5 and $10 a month");
  // and not the Markdown: no fence, no language tag, no formula's TeX
  expect(said).not.toContain("```");
  expect(said).not.toContain("brainfuck");
  expect(said).not.toContain("\\int");
  expect(said).toContain("formula");
  // `$$` beside other words is text, as the contract says: only a block of its own is a formula
  expect(said).toContain("In bash, $$ is the process id; print $$ twice.");
  // the end of the words is the end of the answer: the button is Read aloud again
  await endVoice(page);
  await idle(page);
});

test("Stop cuts the voice, and Esc does too", async ({ page }) => {
  await stubVoice(page);
  await codeMath(page);
  await readAloud(page).click();
  await reading(page);
  await readAloud(page).click();
  await idle(page);
  // one for the press that cuts whatever the device still held, one for Stop
  expect(await cancels(page)).toBe(2);

  // Esc stops a read-aloud too (and only while something is read)
  await readAloud(page).click();
  await reading(page);
  await page.keyboard.press("Escape");
  await idle(page);
  expect(await cancels(page)).toBe(4);
});

test("one answer at a time, app-wide: a second answer cuts the first", async ({ page }) => {
  await stubVoice(page);
  await codeMath(page);
  await page.getByLabel("Message", { exact: true }).fill("talk");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(badge(page)).toHaveText("Done");
  await expect(turn(page, 1)).toContainText("Plan: add a test");

  await readAloud(page, 0).click();
  await reading(page, 0);
  await expect.poll(() => speaking(page)).not.toBeNull();
  await readAloud(page, 1).click();
  await reading(page, 1);
  // the first answer is not read any more
  await idle(page, 0);
  // and the second was queued after the cut: nothing of it was lost to the cancel
  const log = await voiceLog(page);
  const cut = log.map((entry) => entry.op).lastIndexOf("cancel");
  const second = log.findIndex(
    (entry) => entry.op === "speak" && (entry.text ?? "").includes("Plan: add a test"),
  );
  expect(second).toBeGreaterThan(cut);
  expect(log.filter((entry) => entry.op === "dropped")).toEqual([]);
});

test("a refusal is said in words, and never thrown", async ({ page }) => {
  await stubVoice(page);
  await codeMath(page);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await readAloud(page).click();
  await reading(page);
  await failVoice(page, "not-allowed");
  await expect(turn(page).getByText("Could not read aloud")).toBeVisible();
  await idle(page);
  expect(errors).toEqual([]);
});

test("a chunk that fails half way stops the rest, and Esc still works after it", async ({
  page,
}) => {
  await stubVoice(page);
  await codeMath(page);
  // the answer is many chunks and the second one fails when it starts, the rest already queued behind it
  await failChunk(page, 1);
  await readAloud(page).click();
  await reading(page);
  await expect.poll(() => speaking(page)).not.toBeNull();
  await endChunk(page);
  await expect(turn(page).getByText("Could not read aloud")).toBeVisible();
  // nothing is left queued behind the failure, and nothing is still being said
  expect(await pending(page)).toBe(0);
  expect(await speaking(page)).toBeNull();
  await idle(page);

  // and the next read is a clean one, which Esc stops
  const before = await cancels(page);
  await readAloud(page).click();
  await reading(page);
  await expect.poll(() => speaking(page)).not.toBeNull();
  await page.keyboard.press("Escape");
  await idle(page);
  expect(await cancels(page)).toBeGreaterThan(before);
  expect(await speaking(page)).toBeNull();
});

test("there is no Read aloud where the browser has no voice, and Copy is still there", async ({
  page,
}) => {
  await removeVoice(page);
  await codeMath(page);
  // the turn's own Copy, not a code block's
  const actions = turn(page).locator('[data-slot="turn-actions"]');
  await expect(actions.getByRole("button", { name: "Copy" })).toBeVisible();
  await expect(actions.getByRole("button", { name: "Read aloud" })).toHaveCount(0);
});

test("the turn actions with Read aloud pass axe, idle and while it reads, in both schemes", async ({
  page,
}) => {
  await stubVoice(page);
  await codeMath(page);
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await turn(page).hover();
    await animationsDone(page);
    const idleResults = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(
      idleResults.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
      `${scheme}, idle`,
    ).toEqual([]);

    // the state a person sees while an answer is read is part of the page: the square, the tooltip, the pressed state
    await readAloud(page).click();
    await reading(page);
    await animationsDone(page);
    const readingResults = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(
      readingResults.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
      `${scheme}, reading`,
    ).toEqual([]);
    await readAloud(page).click();
    await idle(page);
  }
});
