import { readFile } from "node:fs/promises";
import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import {
  badge,
  expectNoHorizontalScroll,
  exportMenuItem,
  MOCK_URL,
  openThreadList,
  startThread,
  threadList,
} from "./helpers";

/*
 * A thread's description (ADR 0035): the model writes it when a job ends and it arrives on the open
 * thread by itself (the mock's `Plan …` script, three sentences), it is a muted line under the
 * header that opens to the whole text, a hover card on the thread in the sidebar, in the export, a
 * person edits it from the thread's menu (an empty one clears it), and `ui.showDescriptions` hides
 * it everywhere. The text is plain: a model wrote it.
 *
 * The mock keeps `ui` per session, named by a cookie, so the test that hides descriptions does not
 * hide them from the tests running beside it.
 */

const ORIGIN = "http://127.0.0.1:3000";
const TITLE = "Plan the session expiry test";
const SENTENCE = "The person wants a plan for adding a test to the session expiry check";
const WHOLE = /Nothing was changed yet\.$/;
/** The mock plays a step every 400 ms and describes the thread after the last: a few seconds. */
const ARRIVES = { timeout: 20_000 };

const test = base.extend<{ config: { showDescriptions: (on: boolean) => Promise<void> } }>({
  config: async ({ context, request }, use, info) => {
    const session = `e2e-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    await use({
      showDescriptions: async (on) => {
        const res = await request.post(
          `${MOCK_URL}/__mock/config?showDescriptions=${on}&session=${session}`,
        );
        expect(res.ok()).toBe(true);
      },
    });
  },
});

/** The description under the header (the line and its control). */
const description = (page: Page) => page.locator('[data-slot="thread-description"]');
const field = (page: Page) => page.getByRole("textbox", { name: "Thread description" });

/**
 * This page's own thread in the sidebar. The mock keeps every test's threads, and the tests of this
 * file all start one with the same title, so the first row of that title can be another test's
 * thread (one whose description has not come, or that a test cleared): no card is drawn for it.
 */
function ownRow(page: Page) {
  const id = new URL(page.url()).pathname.split("/").pop();
  return threadList(page).locator(`a[href$="/threads/${id}"]`);
}

/** An item of the thread's overflow menu, opened and chosen with the keyboard. */
async function chooseFromMenu(page: Page, name: string | RegExp) {
  await page.getByRole("button", { name: "Thread options" }).focus();
  await page.keyboard.press("Enter");
  await page.getByRole("menuitem", { name }).click();
}

async function describedThread(page: Page) {
  await startThread(page, TITLE);
  // the model describes the thread after its job ends: it arrives on the open page, no reload
  await expect(description(page)).toContainText(SENTENCE, ARRIVES);
  await expect(badge(page)).toHaveText("Done");
}

test("the description arrives on the open thread, one muted line that opens to the whole text", async ({
  page,
}) => {
  await startThread(page, TITLE);
  await expect(description(page)).toContainText(SENTENCE, ARRIVES);
  await expectNoHorizontalScroll(page);

  const more = description(page).getByRole("button", { name: "Show more" });
  await expect(more).toHaveAttribute("aria-expanded", "false");
  // one line: it is cut, not wrapped
  const text = description(page).locator("p");
  const oneLine = (await text.boundingBox())?.height ?? 0;
  expect(oneLine).toBeLessThan(30);

  // the keyboard opens it and closes it again
  await more.focus();
  await page.keyboard.press("Enter");
  const less = description(page).getByRole("button", { name: "Show less" });
  await expect(less).toHaveAttribute("aria-expanded", "true");
  await expect(text).toHaveText(new RegExp(`^${SENTENCE}.*Nothing was changed yet\\.$`));
  expect((await text.boundingBox())?.height ?? 0).toBeGreaterThan(oneLine);
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Space");
  await expect(description(page).getByRole("button", { name: "Show more" })).toBeVisible();
});

test("the description is in the thread's export", async ({ page }) => {
  await describedThread(page);
  const item = await exportMenuItem(page);
  const [download] = await Promise.all([page.waitForEvent("download"), item.click()]);
  const doc = JSON.parse(await readFile(await download.path(), "utf8"));
  expect(doc.thread.description).toMatch(WHOLE);
  expect(doc.events.at(-1)).toMatchObject({
    kind: "thread_described",
    data: { source: "model" },
  });
});

test("the sidebar row shows it in a hover card, and says it as the link's description", async ({
  page,
  isMobile,
}) => {
  await describedThread(page);
  await openThreadList(page);
  const row = ownRow(page);
  await expect(row).toHaveAccessibleDescription(WHOLE);
  test.skip(isMobile, "a phone has no hover; the line under the header has the text");
  await row.hover();
  const card = page.locator('[data-slot="thread-description-card"]');
  await expect(card).toContainText(SENTENCE);
  await expect(card).toContainText(TITLE);
  await page.mouse.move(700, 500);
  await expect(card).toBeHidden();
  // the keyboard: focusing the row opens it, Escape closes it
  await row.focus();
  await expect(card).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(card).toBeHidden();
});

test("a person writes the description with the keyboard, and an empty one clears it", async ({
  page,
}) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  await expect(description(page)).toHaveCount(0);

  await chooseFromMenu(page, "Add description");
  await expect(field(page)).toBeFocused();
  await page.keyboard.type("  Say hello to the world.  ");
  await page.keyboard.press("Enter");
  await expect(description(page)).toHaveText("Say hello to the world.");
  await expect(field(page)).toHaveCount(0);
  // it is the server's: it is there after a reload, and the model's never replaces it
  await page.reload();
  await expect(description(page)).toHaveText("Say hello to the world.");

  await chooseFromMenu(page, "Edit description");
  await expect(field(page)).toHaveValue("Say hello to the world.");
  await page.keyboard.type(" Twice.");
  await page.keyboard.press("Escape");
  await expect(description(page)).toHaveText("Say hello to the world.");

  await chooseFromMenu(page, "Edit description");
  await field(page).fill("");
  await page.keyboard.press("Enter");
  await expect(description(page)).toHaveCount(0);
  await page.reload();
  await expect(badge(page)).toHaveText("Done");
  await expect(description(page)).toHaveCount(0);
  await page.getByRole("button", { name: "Thread options" }).click();
  await expect(page.getByRole("menuitem", { name: "Add description" })).toBeVisible();
});

test("a refused description says why, keeps the field and what was typed", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  await page.route("**/api/threads/*", async (route) => {
    if (route.request().method() !== "PATCH") return route.fallback();
    return route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Service Unavailable",
        status: 503,
        detail: "storage is unavailable",
      }),
    });
  });
  await chooseFromMenu(page, "Add description");
  await field(page).fill("Mine.");
  await page.keyboard.press("Enter");
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not save the description" }),
  ).toContainText("storage is unavailable");
  await expect(field(page)).toHaveValue("Mine.");
});

test("with ui.showDescriptions off the description is nowhere: no line, no card, no menu item", async ({
  page,
  config,
  isMobile,
}) => {
  await config.showDescriptions(false);
  await startThread(page, TITLE);
  await expect(badge(page)).toHaveText("Done");
  // the model's description has had its time to arrive
  await expect
    .poll(async () => {
      const id = /\/threads\/([0-9a-f-]{36})$/.exec(page.url())?.[1];
      const got = (await (await fetch(`${MOCK_URL}/api/threads/${id}`)).json()) as {
        description?: string;
      };
      return got.description ?? "";
    })
    .toMatch(WHOLE);
  await page.waitForTimeout(300);
  await expect(description(page)).toHaveCount(0);
  await expect(page.getByText(SENTENCE)).toHaveCount(0);
  await openThreadList(page);
  const row = ownRow(page);
  expect(await row.getAttribute("aria-describedby")).toBeNull();
  if (!isMobile) {
    await row.hover();
    await page.waitForTimeout(700);
    await expect(page.locator('[data-slot="thread-description-card"]')).toHaveCount(0);
  }
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Thread options" }).click();
  await expect(page.getByRole("menuitem", { name: "Rename" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: /description/i })).toHaveCount(0);
});

test("the description is plain text, whatever it holds", async ({ page }) => {
  await startThread(page, "echo hello");
  await expect(badge(page)).toHaveText("Done");
  const text = "**bold** <img src=x onerror=window.__pwned=1> [link](https://evil.example)";
  await chooseFromMenu(page, "Add description");
  await field(page).fill(text);
  await page.keyboard.press("Enter");
  await expect(description(page)).toContainText("**bold**");
  await expect(description(page).locator("strong, img, a")).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBe(
    undefined,
  );
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility of the description (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    const serious = async (page: Page) => {
      // axe reads the colours as they are drawn: a card that is fading in is not at its colours yet
      // (the fade runs for 150 ms and a loaded machine starts axe inside it)
      await page.evaluate(() =>
        Promise.all(
          document
            .getAnimations()
            .filter((a) => a.effect?.getComputedTiming().iterations !== Number.POSITIVE_INFINITY)
            .map((a) => a.finished.catch(() => undefined)),
        ),
      );
      return (
        await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()
      ).violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    };

    test("axe: the line closed and open, the field, the hover card", async ({ page, isMobile }) => {
      await describedThread(page);
      expect(await serious(page)).toEqual([]);

      await description(page).getByRole("button", { name: "Show more" }).click();
      await expect(description(page).getByRole("button", { name: "Show less" })).toBeVisible();
      expect(await serious(page)).toEqual([]);

      await chooseFromMenu(page, "Edit description");
      await expect(field(page)).toBeFocused();
      expect(await serious(page)).toEqual([]);
      await page.keyboard.press("Escape");

      await page.getByRole("button", { name: "Thread options" }).click();
      // (no axe pass with it open: the thread menu is a modal Radix menu, which hides the page behind
      // it from assistive technology while the page's buttons stay focusable under it, and axe
      // reads that as `aria-hidden-focus`; the report names the page's own buttons, not the items.)
      await expect(page.getByRole("menuitem", { name: "Edit description" })).toBeVisible();
      await page.keyboard.press("Escape");

      if (!isMobile) {
        await ownRow(page).hover();
        const card = page.locator('[data-slot="thread-description-card"]');
        await expect(card).toBeVisible();
        // open, and then at rest: `serious` waits for the end of its animation
        await expect(card).toHaveAttribute("data-state", "open");
        expect(await serious(page)).toEqual([]);
      }
    });
  });
}
