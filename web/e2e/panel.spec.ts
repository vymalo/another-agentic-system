import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import {
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  panel,
  panelResizer,
  panelTab,
  panelToggle,
  startThread,
} from "./helpers";

/*
 * The right-hand panel (web/DESIGN.md, "Panel"): beside the chat on a wide window, a sheet on a
 * narrower one, with the tabs Activity and Sources. The browser of the suite is 1280 px wide: the
 * panel opens by itself there when the person never chose.
 */

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

const CODER_PR = "https://github.com/acme/demo/pull/12";

/** A finished thread of the `sources` scenario (mock/scripts.ts). */
async function sourcesThread(page: Page) {
  await startThread(page, "sources the login redirect");
  await expect(badge(page)).toHaveText("Done", { timeout: 20_000 });
}

/** Makes sure the panel is showing: docked and open already, or its toggle opens the sheet. */
async function openPanel(page: Page) {
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await expect(panel(page)).toBeVisible();
}

test("the panel opens by itself on a wide window and remembers the person's choice", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "a wide window");
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  await expect(panel(page)).toBeVisible();
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(panelToggle(page)).toHaveAttribute("aria-controls", "thread-panel");
  await expect(panelTab(page, "Activity")).toHaveAttribute("aria-selected", "true");
  await expectNoHorizontalScroll(page);

  // the toggle closes it; nothing of it is reachable, and the choice survives a reload
  await panelToggle(page).click();
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(panel(page)).toBeHidden();
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(panel(page)).toBeHidden();
  await expect(page.locator("html")).not.toHaveAttribute("data-panel", "open");

  await panelToggle(page).click();
  await expect(panel(page)).toBeVisible();
  await page.reload();
  await expect(panel(page)).toBeVisible();
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator("html")).toHaveAttribute("data-panel", "open");
});

test("Ctrl+Shift+. toggles it from anywhere, the box included", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "a keyboard shortcut");
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  await page.getByLabel("Message").focus();
  await page.keyboard.press("Control+Shift+Period");
  await expect(panel(page)).toBeHidden();
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "false");
  // the box keeps the focus, and the shortcut typed nothing
  await expect(page.getByLabel("Message")).toBeFocused();
  await expect(page.getByLabel("Message")).toHaveValue("");
  await page.keyboard.press("Control+Shift+Period");
  await expect(panel(page)).toBeVisible();
});

test("the new-chat page has no panel", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "What should we get done?" })).toBeVisible();
  await expect(panelToggle(page)).toHaveCount(0);
  await expect(page.locator("#thread-panel")).toHaveCount(0);
});

test("the tabs are Activity and Sources, and the arrows move between them", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "the phone's sheet is its own test");
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  const tabs = panel(page).getByRole("tablist", { name: "Sections" }).getByRole("tab");
  await expect(tabs).toHaveCount(2);
  // the Activity tab holds the agent's steps (one turn here)
  await expect(panel(page).getByRole("tabpanel")).toContainText("Turn 1");

  await panelTab(page, "Activity").focus();
  await page.keyboard.press("ArrowRight");
  await expect(panelTab(page, "Sources")).toHaveAttribute("aria-selected", "true");
  await expect(panelTab(page, "Sources")).toBeFocused();
  // the echo scenario shares one thing: its pull request
  await expect(panelTab(page, "Sources")).toHaveText("Sources1");
  await expect(panel(page).getByRole("link", { name: /^acme\/demo#1/ })).toBeVisible();

  // the tab is remembered
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(panelTab(page, "Sources")).toHaveAttribute("aria-selected", "true");
});

test("Sources lists what the agent shared, each once: the pull request, the branch, the run, the links in its words", async ({
  page,
}) => {
  await sourcesThread(page);
  await openPanel(page);
  await panelTab(page, "Sources").click();
  // a pull request, a branch, a CI report and one link: the pull request and the run the words link to
  // are the same items as the typed ones, so they are not listed twice
  await expect(panelTab(page, "Sources")).toHaveText("Sources4");

  const code = panel(page).getByRole("region", { name: "Pull requests & branches" });
  const pr = code.getByRole("link", {
    name: /^acme\/demo#12 — Fix the redirect loop after signing in/,
  });
  await expect(pr).toHaveAttribute("href", CODER_PR);
  await expect(pr).toHaveAttribute("target", "_blank");
  await expect(pr).toHaveAttribute("rel", "noopener noreferrer");
  await expect(pr).toContainText("(opens in a new tab)");
  // a branch is text: the projection's repository is host/owner/name, not a URL
  await expect(code.getByText("agent/fix-login-redirect · acme/demo")).toBeVisible();
  await expect(code.getByRole("link", { name: /agent\/fix-login-redirect/ })).toHaveCount(0);

  const checks = panel(page).getByRole("region", { name: "Checks" });
  await expect(checks.getByRole("link", { name: /^CI: ci\/build — passed/ })).toHaveAttribute(
    "href",
    "https://ci.example.com/runs/12",
  );

  const links = panel(page).getByRole("region", { name: "Links" });
  await expect(links.getByRole("link")).toHaveCount(1);
  await expect(links.getByRole("link", { name: /^the axum redirect docs/ })).toHaveAttribute(
    "href",
    "https://docs.rs/axum/latest/axum/response/struct.Redirect.html",
  );
  // not a source: a URL inside code, a javascript: link
  await expect(panel(page).getByText("only-in-code")).toHaveCount(0);
  await expect(panel(page).locator('a[href^="javascript:"]')).toHaveCount(0);
  await expectNoHorizontalScroll(page);
});

test("a Turn button scrolls the chat to that turn and focuses its header", async ({
  page,
}, testInfo) => {
  await sourcesThread(page);
  await openPanel(page);
  await panelTab(page, "Sources").click();
  await panel(page)
    .getByRole("button", { name: "Show turn 1 in the conversation" })
    .first()
    .click();
  // a sheet covers the chat, so it closes first; a docked panel stays
  if (testInfo.project.name === "mobile") await expect(panel(page)).toBeHidden();
  else await expect(panel(page)).toBeVisible();
  const header = conversation(page).locator('[data-slot="turn-header"]').first();
  await expect(header).toBeFocused();
  await expect(header).toBeInViewport();
});

test("the panel resizes from the keyboard, within its limits, and remembers the width", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "a docked panel");
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  const handle = panelResizer(page);
  await expect(handle).toHaveAttribute("aria-valuemin", "300");
  await expect(handle).toHaveAttribute("aria-valuenow", "360");
  // the width animates for a moment when it changes
  const width = async () => Math.round((await panel(page).boundingBox())?.width ?? 0);
  await expect.poll(width).toBe(360);

  await handle.focus();
  await page.keyboard.press("ArrowLeft");
  await expect(handle).toHaveAttribute("aria-valuenow", "376");
  await expect.poll(width).toBe(376);
  await page.keyboard.press("Home");
  await expect(handle).toHaveAttribute("aria-valuenow", "300");
  await page.keyboard.press("End");
  // 1280 px: the chat keeps 560 beside the 272 px sidebar, so the panel stops at 448
  await expect(handle).toHaveAttribute("aria-valuenow", "448");
  await expectNoHorizontalScroll(page);

  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(panelResizer(page)).toHaveAttribute("aria-valuenow", "448");
});

test("the panel's edge is dragged with the pointer", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "a docked panel");
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  const box = await panelResizer(page).boundingBox();
  if (!box) throw new Error("no resize handle");
  const x = box.x + box.width / 2;
  const y = box.y + box.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x - 60, y, { steps: 5 });
  await page.mouse.up();
  await expect(panelResizer(page)).toHaveAttribute("aria-valuenow", "420");
});

test("on a narrower window the panel is a sheet from the right: the toggle opens it, Escape closes it and the focus goes back", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name === "mobile", "a tablet");
  await page.setViewportSize({ width: 1000, height: 800 });
  await startThread(page, "Implement the thing");
  await expect(badge(page)).toHaveText("Done");
  // never opens by itself, whatever the browser remembered on a wide window
  await expect(panel(page)).toHaveCount(0);
  await expect(panelToggle(page)).toHaveAttribute("aria-expanded", "false");
  await panelToggle(page).click();
  const sheet = page.getByRole("dialog", { name: "Thread details" });
  await expect(sheet).toBeVisible();
  await expect(sheet).toHaveAttribute("data-side", "right");
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();
  await expect(panelToggle(page)).toBeFocused();
});

test("phone: the panel is a sheet from the bottom with both tabs, and a Turn button closes it onto the turn", async ({
  page,
}, testInfo) => {
  test.skip(testInfo.project.name !== "mobile", "a phone");
  await sourcesThread(page);
  await expect(panel(page)).toHaveCount(0);
  await panelToggle(page).click();
  const sheet = page.getByRole("dialog", { name: "Thread details" });
  await expect(sheet).toBeVisible();
  await expect(sheet).toHaveAttribute("data-side", "bottom");
  await expectNoHorizontalScroll(page);
  await expect(panelTab(page, "Activity")).toHaveAttribute("aria-selected", "true");
  await panelTab(page, "Sources").click();
  await expect(sheet.getByRole("region", { name: "Links" })).toBeVisible();
  await expectNoHorizontalScroll(page);

  // Escape closes it and the focus is back on the toggle
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();
  await expect(panelToggle(page)).toBeFocused();

  // a Turn button closes it and goes to the turn
  await panelToggle(page).click();
  await sheet.getByRole("button", { name: "Show turn 1 in the conversation" }).first().click();
  await expect(sheet).toBeHidden();
  await expect(conversation(page).locator('[data-slot="turn-header"]').first()).toBeFocused();
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility with the panel open (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    test("axe: the Activity tab and the Sources tab with sources have no serious violations", async ({
      page,
    }) => {
      await sourcesThread(page);
      await openPanel(page);
      expect(await axeViolations(page)).toEqual([]);
      await panelTab(page, "Sources").click();
      await expect(panel(page).getByRole("region", { name: "Links" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });

    test("axe: the empty Sources tab has no serious violations", async ({ page }) => {
      // a failed run shares nothing
      await startThread(page, "fail now");
      await expect(badge(page)).toHaveText("Failed");
      await openPanel(page);
      await panelTab(page, "Sources").click();
      await expect(panel(page).getByText("Nothing shared yet")).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
