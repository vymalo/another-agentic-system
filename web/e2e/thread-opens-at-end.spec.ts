import { expect, type Page, test } from "@playwright/test";
import { BASE_URL, badge, conversation } from "./helpers";
import { installProbe, measureOpen, seedLongThread } from "./open-probe";

/*
 * A thread opens at its end (ADR 0059, slice 1). Its log is replayed a run at a time; the page used to show the turns as
 * they came and chase its own bottom with a smooth scroll, run after run. Now the transcript is held back, the skeleton
 * stands in, and the whole conversation is shown at once, already at the bottom. The numbers come from the same probe
 * as `open-long-thread.spec.ts`: it samples the viewport on every frame.
 */

const TURNS = 30;

const VIEWPORT = '[data-slot="aui_thread-viewport"]';
/** How far the transcript's end is below what the viewport shows, in px. */
const fromBottom = (page: Page) =>
  page.locator(VIEWPORT).evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop);

test("a long thread is first painted whole and already at the bottom, and nothing scrolls after that", async ({
  browser,
}) => {
  test.setTimeout(180_000);
  const id = await seedLongThread(TURNS);
  const context = await browser.newContext();
  const page = await context.newPage();
  await installProbe(page);
  const m = await measureOpen(page, id, TURNS);
  await context.close();

  // the first thing painted is the conversation, not the start of it
  expect(m.firstPaint, "a turn was painted").not.toBeNull();
  expect(m.turnsAtFirstPaint, "turns in the page at the first paint").toBe(TURNS);
  expect(m.fromBottomAtFirstPaint, "px from the end at the first paint").toBeLessThanOrEqual(1);
  // and it stays: no scroll event, no frame in which the viewport was somewhere else than in the one before
  expect(m.scrollEvents, "scroll events after the first paint").toBe(0);
  expect(m.scrollJumps, "frames in which the viewport moved after the first paint").toBe(0);
  expect(m.travelled, "px the viewport travelled after the first paint").toBe(0);
  expect(m.finalScroll).toBe(m.firstPaint);
});

test("the skeleton stands in while the log replays, and is gone when the conversation is shown", async ({
  page,
}) => {
  const id = await seedLongThread(TURNS);
  const skeleton = page.locator('[data-slot="aui_thread-history-skeleton"]');
  await page.goto(`${BASE_URL}/threads/${id}`);
  await expect(skeleton).toBeVisible();
  await expect(conversation(page)).toHaveAttribute("aria-busy", "true");
  // none of the turns is in front of the person while it replays
  await expect(page.locator('[data-slot="user-message"]:visible')).toHaveCount(0);

  await expect(page.locator('[data-slot="user-message"]:visible')).toHaveCount(TURNS, {
    timeout: 120_000,
  });
  await expect(skeleton).toHaveCount(0);
  await expect(conversation(page)).toHaveAttribute("aria-busy", "false");
  await expect(badge(page)).toHaveText("Done");
});

test("a run that starts after the thread is shown still scrolls to the end, from where the person was", async ({
  page,
}) => {
  test.setTimeout(180_000);
  const id = await seedLongThread(TURNS);
  await page.goto(`${BASE_URL}/threads/${id}`);
  await expect(page.locator('[data-slot="user-message"]:visible')).toHaveCount(TURNS, {
    timeout: 120_000,
  });
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
