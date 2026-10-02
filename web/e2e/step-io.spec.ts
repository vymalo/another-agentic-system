import AxeBuilder from "@axe-core/playwright";
import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  animationsDone,
  activityTab,
  badge,
  expectNoHorizontalScroll,
  hideActivity,
  showActivity,
  startThread,
  turnFailedChip,
  turnSections,
  turnSummaries,
} from "./helpers";

/*
 * Opening a tool step (ADR 0030, web/DESIGN.md "Steps panel"), against the mock's `steps-io` scenario
 * (mock/scripts.ts): one leaf step of each shape a tool step can have. The steps are in the panel's
 * Activity tab; on a phone it is a sheet.
 */

const STEPS_IO = "steps-io please";

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

/** A finished `steps-io` thread with the Activity tab showing its turn. */
async function stepsIo(page: Page): Promise<Locator> {
  await startThread(page, STEPS_IO);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await showActivity(page);
  const turn = turnSections(page).first();
  await expect(
    turn.getByRole("button", { name: /^Web search from search Stephane Segning/ }),
  ).toBeVisible();
  return turn;
}

const search = (turn: Locator) =>
  turn.getByRole("button", { name: /^Web search from search Stephane Segning$/ });
/** The block a toggle opened: the element its aria-controls names. */
async function ioOf(turn: Locator, toggle: Locator): Promise<Locator> {
  const id = await toggle.getAttribute("aria-controls");
  expect(id).toBeTruthy();
  return turn.locator(`[id="${id}"]`);
}

test("a tool step is called by its tool and says what it was asked; a click opens Input, then Output", async ({
  page,
}) => {
  const turn = await stepsIo(page);

  // not "search__web_search": the tool in words, from its server, with the query
  await expect(turn.getByText("search__web_search")).toHaveCount(0);
  const toggle = search(turn);
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(toggle.locator('[data-slot="step-server"]')).toHaveText("from search");
  await expect(toggle.locator('[data-slot="step-preview"]')).toHaveText("Stephane Segning");
  await expect(turn.locator('[data-slot="step-io"]')).toHaveCount(0);

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  const io = await ioOf(turn, toggle);
  await expect(io.getByRole("heading", { level: 4 })).toHaveText(["Input", "Output"]);
  const input = io.locator('[data-slot="step-input"]');
  await expect(input).toContainText("query");
  await expect(input).toContainText("Stephane Segning");
  await expect(input).toContainText("api_key");
  await expect(input).toContainText("[redacted]");
  const output = io.locator('[data-slot="step-output"]');
  await expect(output).toContainText("example.org/mock-search/1");
  // text in a monospace face, not markup
  expect(await output.evaluate((el) => getComputedStyle(el).fontFamily)).toMatch(/mono/i);
  await expectNoHorizontalScroll(page);

  // the same button closes it
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(io).toHaveCount(0);

  // and it is a replay: a reload brings the same block back, from the log
  await hideActivity(page);
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await showActivity(page);
  const again = search(turnSections(page).first());
  await again.click();
  await expect(
    (await ioOf(turnSections(page).first(), again)).locator('[data-slot="step-output"]'),
  ).toContainText("example.org/mock-search/1");
});

test("the button works from the keyboard: Enter and Space open and close it", async ({ page }) => {
  const turn = await stepsIo(page);
  const toggle = search(turn);
  await toggle.focus();
  await expect(toggle).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(turn.locator('[data-slot="step-io"]')).toHaveCount(1);
  await expect(toggle).toBeFocused();
  await page.keyboard.press("Space");
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Space");
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
});

test("a cut output says how much is not there; an input too big to keep says so", async ({
  page,
}) => {
  const turn = await stepsIo(page);

  const page_ = turn.getByRole("button", {
    name: /^Fetch page from fetch https:\/\/example\.org\/long-page$/,
  });
  await page_.click();
  const out = await ioOf(turn, page_);
  await expect(out.getByRole("heading", { level: 4 })).toHaveText(["Input", "Output"]);
  await expect(out.locator('[data-slot="step-output-cut"]')).toHaveText(/^\d+ KiB more not kept$/);
  await expect(out.locator('[data-slot="step-output"]')).toContainText("bytes not kept");

  const write = turn.getByRole("button", { name: /^Write many from files$/ });
  await write.click();
  const cut = await ioOf(turn, write);
  await expect(cut).toContainText("Input not kept (18 KiB)");
  await expect(cut.locator('[data-slot="step-input"]')).toHaveCount(0);
  await expectNoHorizontalScroll(page);
});

test("a failed step shows Error, and the step with nothing to show is not a button", async ({
  page,
}) => {
  const turn = await stepsIo(page);

  const failed = turn.getByRole("button", { name: /^Failed: Command failed/ });
  await expect(failed).toHaveAttribute("aria-expanded", "false");
  await failed.click();
  const io = await ioOf(turn, failed);
  await expect(io.getByRole("heading", { level: 4 })).toHaveText(["Input", "Error"]);
  await expect(io.locator('[data-slot="step-output"]')).toContainText(
    "Type error: Property 'title'",
  );
  await expect(io.getByRole("heading", { name: "Output" })).toHaveCount(0);

  // the job's budget had no room for this one's: it says so, with nothing to read
  const dropped = turn.getByRole("button", { name: /^Web search from search$/ });
  await dropped.click();
  await expect((await ioOf(turn, dropped)).locator('[data-slot="step-io-dropped"]')).toContainText(
    "not kept",
  );

  // a step that sent neither is a plain row
  const none = turn.locator('[data-slot="step"]', { hasText: "no_details" });
  await expect(none).toHaveCount(1);
  await expect(none.getByRole("button")).toHaveCount(0);
});

test("the failed chip of the chat's line takes the person to the failed step, open", async ({
  page,
}) => {
  await startThread(page, STEPS_IO);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(turnSummaries(page)).toHaveCount(1);
  const chip = turnFailedChip(page);
  await expect(chip).toHaveText("1 failed");
  // a control of its own, beside the line's button
  await expect(chip).toHaveRole("button");
  await expect(turnSummaries(page).locator('[data-slot="failed-chip"]')).toHaveCount(0);

  await chip.click();
  await expect(activityTab(page)).toBeVisible();
  const turn = turnSections(page).first();
  const failed = turn.getByRole("button", { name: /^Failed: Command failed/ });
  await expect(failed).toHaveAttribute("aria-expanded", "true");
  await expect(failed).toBeFocused();
  await expect(await ioOf(turn, failed)).toContainText("Type error: Property 'title'");
  await expectNoHorizontalScroll(page);
});

test("the chip of a failure inside a sub-agent opens the sub-agent and the failed step", async ({
  page,
}) => {
  await startThread(page, "Delegate the login fix to the coding sub-agent");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await turnFailedChip(page).click();
  const turn = turnSections(page).first();
  await expect(turn.getByRole("button", { name: /^OpenCode/ })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  const failed = turn.getByRole("button", { name: /^Failed: Command failed/ });
  await expect(failed).toBeFocused();
  const io = await ioOf(turn, failed);
  await expect(io.getByRole("heading", { level: 4 })).toHaveText(["Input", "Error"]);
  await expect(io).toContainText("login::keeps_the_next_url");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("axe: the Activity tab with every step opened", async ({ page }) => {
      const turn = await stepsIo(page);
      const toggles = turn.locator('[data-slot="step-toggle"]');
      const n = await toggles.count();
      expect(n).toBeGreaterThanOrEqual(5);
      for (let i = 0; i < n; i++) {
        const toggle = toggles.nth(i);
        if ((await toggle.getAttribute("aria-expanded")) === "false") await toggle.click();
      }
      await expect(turn.locator('[data-slot="step-io"]')).toHaveCount(n);
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
