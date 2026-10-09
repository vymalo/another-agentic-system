import { type BrowserContext, expect, type Page, test } from "@playwright/test";
import { BASE_URL, badge, conversation, MOCK_URL } from "./helpers";
import { installProbe, measureOpen, seedLongThread } from "./open-probe";

/*
 * A thread opens at its end (ADR 0059). Its log may be replayed a run at a time (an orchestrator that does not serve the
 * history, or does not ask the web to open from it: `history=on`): the page used to show the turns as they came and chase
 * its own bottom with a smooth scroll, run after run, and now the transcript is held back, the skeleton stands in, and the
 * whole conversation is shown at once, already at the bottom (slice 1). Or the thread is opened from its newest turns (the
 * mock's default, `windowed`): the first page is the conversation, at the bottom. Either way nothing scrolls after the first
 * paint. The numbers come from the same probe as `open-long-thread.spec.ts`: it samples the viewport on every frame.
 */

const ORIGIN = "http://127.0.0.1:3000";
const TURNS = 30;
/** The newest turns a thread opened from its history shows (`ui.history.initialTurns`). */
const NEWEST = 12;

const VIEWPORT = '[data-slot="aui_thread-viewport"]';
const USER = '[data-slot="user-message"]:visible';
/** How far the transcript's end is below what the viewport shows, in px. */
const fromBottom = (page: Page) =>
  page.locator(VIEWPORT).evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop);

type Mode = "on" | "windowed";
const MODES: { mode: Mode; shown: number; how: string }[] = [
  { mode: "on", shown: TURNS, how: "replayed" },
  { mode: "windowed", shown: NEWEST, how: "opened from its history" },
];

let sessions = 0;
/** Makes the context's browser a session of its own, with the mock's `ui.history` as given. */
async function history(context: BrowserContext, mode: Mode) {
  const session = `e2e-opens-${process.pid}-${Date.now()}-${++sessions}`;
  const res = await fetch(`${MOCK_URL}/__mock/config?history=${mode}&session=${session}`, {
    method: "POST",
  });
  expect(res.status).toBe(204);
  await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
}

for (const { mode, shown, how } of MODES) {
  test.describe(`a long thread ${how}`, () => {
    test("is first painted whole and already at the bottom, and nothing scrolls after that", async ({
      browser,
    }) => {
      test.setTimeout(180_000);
      const id = await seedLongThread(TURNS);
      const context = await browser.newContext();
      await history(context, mode);
      const page = await context.newPage();
      await installProbe(page);
      const m = await measureOpen(page, id, TURNS, { shown });
      await context.close();

      // the first thing painted is the conversation, not the start of it
      expect(m.firstPaint, "a turn was painted").not.toBeNull();
      expect(m.turnsAtFirstPaint, "turns in the page at the first paint").toBe(shown);
      expect(m.fromBottomAtFirstPaint, "px from the end at the first paint").toBeLessThanOrEqual(1);
      // and it stays: no scroll event, no frame in which the viewport was somewhere else than in the one before
      expect(m.scrollEvents, "scroll events after the first paint").toBe(0);
      expect(m.scrollJumps, "frames in which the viewport moved after the first paint").toBe(0);
      expect(m.travelled, "px the viewport travelled after the first paint").toBe(0);
      expect(m.finalScroll).toBe(m.firstPaint);
    });

    test("the skeleton stands in while it loads, and is gone when the conversation is shown", async ({
      page,
      context,
      request,
    }) => {
      await history(context, mode);
      const id = await seedLongThread(TURNS);
      // the page of history is held back a moment, so that the skeleton can be looked at
      if (mode === "windowed") {
        const held = await request.post(`${MOCK_URL}/__mock/history-delay?thread=${id}&ms=800`);
        expect(held.status()).toBe(204);
      }
      const skeleton = page.locator('[data-slot="aui_thread-history-skeleton"]');
      await page.goto(`${BASE_URL}/threads/${id}`);
      await expect(skeleton).toBeVisible();
      await expect(conversation(page)).toHaveAttribute("aria-busy", "true");
      // none of the turns is in front of the person while it loads
      await expect(page.locator(USER)).toHaveCount(0);

      await expect(page.locator(USER)).toHaveCount(shown, { timeout: 120_000 });
      await expect(skeleton).toHaveCount(0);
      await expect(conversation(page)).toHaveAttribute("aria-busy", "false");
      await expect(badge(page)).toHaveText("Done");
    });

    test("a run that starts after the thread is shown still scrolls to the end, from where the person was", async ({
      page,
      context,
    }) => {
      test.setTimeout(180_000);
      await history(context, mode);
      const id = await seedLongThread(TURNS);
      await page.goto(`${BASE_URL}/threads/${id}`);
      await expect(page.locator(USER)).toHaveCount(shown, { timeout: 120_000 });
      await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(1);

      // the person reads something far above, and then writes
      await page.locator(VIEWPORT).evaluate((el) => el.scrollTo({ top: 0, behavior: "instant" }));
      await expect.poll(() => fromBottom(page)).toBeGreaterThan(1000);
      await page.getByLabel("Message").fill("Fix the next thing");
      await page.getByRole("button", { name: "Send" }).click();

      // the run starts, and the page follows it to the end
      const next = conversation(page).getByText("Fix the next thing", { exact: true });
      await expect(next).toBeVisible();
      await expect.poll(() => fromBottom(page), { timeout: 15_000 }).toBeLessThanOrEqual(1);
      await expect(next).toBeInViewport();
      await expect(badge(page)).toHaveText("Done");
      await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(1);
    });
  });
}
