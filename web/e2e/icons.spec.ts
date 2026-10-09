import { expect, type Locator, type Page, test } from "@playwright/test";
import { badge, panelToggle, startThread } from "./helpers";

/**
 * Icons where an icon does the job (the owner, 2026-10-09: "avoid too much texts when a logo/icon can do
 * the job"): a state is a shape of its own, a control that is an icon keeps its name and says its words in
 * a tooltip, for a pointer and for the keyboard, and what a person has to read stays text. The pills are
 * measured, not read: the words of an icon are visually hidden text, which a screen reader reads and a
 * test of `innerText` cannot tell from words on the screen.
 */

/** The tooltip that is open: one that is on its way out (its exit animation) is still in the page, closed. */
const tooltip = (page: Page) => page.locator('[role="tooltip"]:not([data-state="closed"])');

/** The width of an element in CSS pixels. */
async function widthOf(el: Locator): Promise<number> {
  const box = await el.boundingBox();
  if (!box) throw new Error("the element is not drawn");
  return box.width;
}

test.describe("the state of a thread", () => {
  test("is an icon with its words as its name and its tooltip", async ({ page }) => {
    await startThread(page, "Fix the redirect loop after signing in");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    // a pill of an icon, not of words: the same width as the other chips of the top bar
    expect(await widthOf(badge(page))).toBeLessThan(36);
    await expect(badge(page)).toHaveAttribute("aria-label", "Thread state: Done");
    await expect(badge(page).locator("svg")).toHaveCount(1);
    // a pointer resting on it says the word
    await badge(page).hover();
    await expect(tooltip(page)).toHaveText("Done");
  });

  test("keeps its words for the one state that asks the person to act", async ({ page }) => {
    await startThread(page, "Deploy the new login page");
    await expect(badge(page)).toHaveText("Your turn");
    expect(await widthOf(badge(page))).toBeGreaterThan(60);
    await expect(badge(page)).toHaveAttribute("aria-label", "Thread state: Your turn");
  });

  test("while the agent works is a spinner, and a stopped thread another shape", async ({
    page,
  }) => {
    await startThread(page, "Refactor the session store behind a trait");
    await expect(badge(page)).toHaveAttribute("aria-label", "Thread state: Working…");
    expect(await widthOf(badge(page))).toBeLessThan(36);
    await expect(
      badge(page).locator("svg.animate-spin, svg.motion-safe\\:animate-spin"),
    ).toHaveCount(1);
    const spinner = await badge(page).locator("svg").getAttribute("class");
    await page.getByRole("button", { name: "Stop" }).click();
    await expect(badge(page)).toHaveAttribute("aria-label", "Thread state: Stopped");
    const stopped = await badge(page).locator("svg").getAttribute("class");
    expect(stopped).not.toBe(spinner);
  });
});

test.describe("controls that are icons", () => {
  test("say their words in a tooltip for a pointer, and keep their names", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "a phone has no pointer resting on a control");
    await startThread(page, "Fix the redirect loop after signing in");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });

    // Send is off while the box is empty, and an element that is off is not hovered
    await page.getByLabel("Message").fill("one more thing");
    await page.getByRole("button", { name: "Send" }).hover();
    await expect(tooltip(page)).toHaveText("Send");
    await page.getByRole("button", { name: "Thread options" }).hover();
    await expect(tooltip(page)).toHaveText("Thread options");
    await panelToggle(page).hover();
    await expect(tooltip(page)).toContainText("Thread details");
    // the words are the control's own name: a tooltip that has not opened is not needed to read it
    for (const name of ["Send", "Thread options", "Thread details"]) {
      await expect(page.getByRole("button", { name }).first()).toHaveAccessibleName(name);
    }
  });

  test("say their words when the keyboard reaches them, with a ring that shows where it is", async ({
    page,
  }) => {
    await startThread(page, "Fix the redirect loop after signing in");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    const options = page.getByRole("button", { name: "Thread options" });
    // Tab until the button has the focus: the header's controls come in a fixed order
    await page.getByRole("button", { name: /^Agent:/ }).focus();
    for (
      let i = 0;
      i < 12 && !(await options.evaluate((el) => el === document.activeElement));
      i++
    ) {
      await page.keyboard.press("Tab");
    }
    await expect(options).toBeFocused();
    await expect(tooltip(page)).toHaveText("Thread options");
    // the focus is visible: a ring (the button's own `focus-visible` style) is drawn
    expect(await options.evaluate((el) => el.matches(":focus-visible"))).toBe(true);
    expect(
      await options.evaluate(
        (el) =>
          getComputedStyle(el).boxShadow !== "none" || getComputedStyle(el).outlineStyle !== "none",
      ),
    ).toBe(true);
  });

  test("a control that the page focuses after a click does not explain itself", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "the sidebar of a phone is a sheet");
    await startThread(page, "Fix the redirect loop after signing in");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    // closing the sidebar hands the focus to the button that opens it again
    await page.getByRole("button", { name: "Close sidebar" }).click();
    await expect(page.getByRole("button", { name: "Open sidebar" })).toBeFocused();
    await page.mouse.move(700, 450);
    await page.waitForTimeout(400);
    await expect(tooltip(page)).toHaveCount(0);
  });

  test("a menu's trigger leaves its menu alone: no tooltip over an open menu", async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, "keyboard order");
    await startThread(page, "Fix the redirect loop after signing in");
    await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
    const options = page.getByRole("button", { name: "Thread options" });
    await options.focus();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Shift+Tab");
    await expect(tooltip(page)).toHaveText("Thread options");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("menu")).toBeVisible();
    await expect(tooltip(page)).toHaveCount(0);
  });
});

test.describe("the file the agent made", () => {
  test("is downloaded by an icon that is named after the file", async ({ page }) => {
    await startThread(page, "files make some", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const download = page.getByRole("link", { name: "Download results.png" });
    expect(await widthOf(download)).toBeLessThan(44);
    await expect(download.locator("svg")).toHaveCount(1);
  });
});
