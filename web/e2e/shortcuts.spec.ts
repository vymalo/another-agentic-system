import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { animationsDone, badge, conversation, startThread } from "./helpers";
import { stubVoice } from "./voice";

/*
 * The keyboard help sheet (ADR 0067 S1): the keys this page listens to, opened by `?` and by Ctrl/⌘+`/` and by nothing
 * else. It is a modal dialog: Esc closes it, and `?` in the message box is a character and not a request for help. The
 * rows are the page's real bindings (`src/features/shortcuts/lib/shortcuts.ts`, held against the handlers by
 * `shortcuts.test.ts`), the sheet's own two openers included. Esc also stops a read-aloud, which is held here in a real
 * browser.
 */

const sheet = (page: Page) => page.getByRole("dialog", { name: "Keyboard shortcuts" });

/** Every shortcut the page has, as the sheet says it. */
const ROWS = [
  "Search threads",
  "Thread details (the panel)",
  "Send",
  "New line",
  "Stop and send",
  "Close a dialog, or stop reading aloud",
  "Show these keys",
];

test("opens with ? and with Ctrl+/, lists the page's keys, and Esc closes it", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("button", { name: /^Agent:/ })).toBeVisible();
  await expect(sheet(page)).toHaveCount(0);

  await page.keyboard.press("?");
  await expect(sheet(page)).toBeVisible();
  await expect(sheet(page).getByRole("heading", { name: "Keyboard shortcuts" })).toBeVisible();
  for (const row of ROWS) await expect(sheet(page).getByText(row, { exact: true })).toBeVisible();
  // the platform's own modifier: Chromium here is not an Apple platform, so Ctrl and not ⌘
  await expect(sheet(page).getByText("Ctrl").first()).toBeVisible();
  await expect(sheet(page).getByText("⌘")).toHaveCount(0);
  // the keys are keys, not prose: the chord the page reads ("Escape") drawn as the keyboard shows it ("Esc")
  await expect(sheet(page).locator("kbd", { hasText: "K" })).toBeVisible();
  await expect(sheet(page).locator("kbd", { hasText: "Esc" })).toBeVisible();
  // and the sheet lists its own two openers, as one row joined by "or"
  const help = sheet(page).getByText("Show these keys", { exact: true }).locator("..");
  await expect(help).toContainText("or");

  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);

  await page.keyboard.press("Control+/");
  await expect(sheet(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);

  // on AZERTY `/` is the shifted character of its key: the chord arrives with Shift held
  await page.keyboard.press("Control+Shift+/");
  await expect(sheet(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);
});

test("? in the message box is a character, and Ctrl+/ still opens the sheet", async ({ page }) => {
  await startThread(page, "echo help");
  await expect(badge(page)).toHaveText("Done");
  const box = page.getByLabel("Message", { exact: true });
  await box.click();
  await page.keyboard.type("?");
  await expect(box).toHaveValue("?");
  await expect(sheet(page)).toHaveCount(0);
  // Ctrl+/ is no browser binding, so it is read from the box too
  await page.keyboard.press("Control+/");
  await expect(sheet(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);
  await expect(box).toHaveValue("?");
});

test("Esc stops a read-aloud", async ({ page }) => {
  await stubVoice(page);
  // the Reviewer answers in words: a card alone has no turn actions, and so no Read aloud
  await startThread(page, "talk", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  const turn = conversation(page).locator('[data-slot="agent-turn"]').last();
  const readAloud = turn.getByRole("button", { name: "Read aloud" });
  await readAloud.click();
  await expect(readAloud).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Escape");
  await expect(readAloud).toHaveAttribute("aria-pressed", "false");
});

test("the help sheet passes axe, in both schemes", async ({ page }) => {
  await page.goto("/");
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.keyboard.press("?");
    await expect(sheet(page)).toBeVisible();
    await animationsDone(page);
    const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(
      results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
      scheme,
    ).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(sheet(page)).toHaveCount(0);
  }
});
