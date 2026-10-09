import { test as base, expect, type Page } from "@playwright/test";
import { activityTab, BASE_URL, badge, conversation, MOCK_URL } from "./helpers";
import { seedLongThread } from "./open-probe";

/*
 * A thread opened from its history (ADR 0059, docs/api/history.md): the page reads the newest turns as one page, shows
 * them at once and at the bottom, and reads older ones as the person scrolls up, keeping the place of what they were
 * reading. The orchestrator is the mock, whose fold is pinned to the real one by the page-boundary goldens
 * (`mock/history.test.ts`).
 */

const ORIGIN = "http://127.0.0.1:3000";
const TURNS = 60;
const VIEWPORT = '[data-slot="aui_thread-viewport"]';
const USER = '[data-slot="user-message"]:visible';

type Mode = "off" | "on" | "windowed";
type Fixtures = {
  /** Sets `ui.history` for this test's session: `windowed` is a deployment that has the web open threads from their history. */
  history: (mode: Mode, params?: Record<string, number>) => Promise<void>;
  /** What the page asked the history route for, per thread, in order. */
  calls: (threadId: string) => Promise<string[]>;
};

const test = base.extend<Fixtures>({
  history: async ({ context, request }, use, info) => {
    const session = `hist-${info.workerIndex}-${info.testId}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    await use(async (mode, params = {}) => {
      const query = new URLSearchParams({
        history: mode,
        session,
        ...Object.fromEntries(Object.entries(params).map(([k, v]) => [k, String(v)])),
      });
      const res = await request.post(`${MOCK_URL}/__mock/config?${query}`);
      expect(res.ok()).toBe(true);
    });
  },
  calls: async ({ request }, use) => {
    await use(async (threadId) => {
      const res = await request.get(`${MOCK_URL}/__mock/history-calls?thread=${threadId}`);
      return (await res.json()) as string[];
    });
  },
});

test.afterEach(async ({ request }) => {
  await request.post(`${MOCK_URL}/__mock/history-fail?times=0`);
  await request.post(`${MOCK_URL}/__mock/history-delay?ms=0`);
});

/** How far the transcript's end is below what the viewport shows, in px. */
const fromBottom = (page: Page) =>
  page.locator(VIEWPORT).evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop);

const toTop = (page: Page) =>
  page.locator(VIEWPORT).evaluate((el) => el.scrollTo({ top: 0, behavior: "instant" }));

/** The first message of the person that is in the page, and how far from the top of the viewport it is. */
const firstMessage = (page: Page) =>
  page.locator(VIEWPORT).evaluate((root) => {
    const el = root.querySelector<HTMLElement>('[data-slot="user-message"]');
    if (!el) return null;
    return {
      id: el.dataset.messageId ?? "",
      text: el.textContent ?? "",
      top: el.getBoundingClientRect().top - root.getBoundingClientRect().top,
    };
  });

const topOf = (page: Page, id: string) =>
  page.locator(VIEWPORT).evaluate((root, wanted) => {
    const el = [...root.querySelectorAll<HTMLElement>("[data-message-id]")].find(
      (e) => e.dataset.messageId === wanted,
    );
    return el ? el.getBoundingClientRect().top - root.getBoundingClientRect().top : null;
  }, id);

const open = async (page: Page, threadId: string) => {
  await page.goto(`${BASE_URL}/threads/${threadId}`);
  await expect(badge(page)).toHaveText("Done", { timeout: 60_000 });
};

test("a long thread opens from its newest turns, at the bottom, and reads nothing older until the person scrolls up", async ({
  page,
  history,
  calls,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await open(page, id);
  // the newest 12 turns (ui.history.initialTurns), not the sixty: the page never replayed the log
  await expect(page.locator(USER)).toHaveCount(12);
  await expect(conversation(page).getByText("Turn 60:").first()).toBeVisible();
  await expect.poll(() => fromBottom(page)).toBeLessThanOrEqual(1);
  expect(await calls(id)).toEqual(["limit=12"]);
});

test("scrolling to the top reads the next older page and the turn being read stays where it was", async ({
  page,
  history,
  request,
  calls,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await request.post(`${MOCK_URL}/__mock/history-delay?ms=1500`);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(12);

  await toTop(page);
  const status = page.getByRole("status").filter({ hasText: "Loading earlier messages" });
  await expect(status).toBeVisible();
  const before = await firstMessage(page);
  expect(before).not.toBeNull();

  // the page of 20 comes in front, and the turn that was first is where it was
  await expect(page.locator(USER)).toHaveCount(32);
  const after = await topOf(page, before?.id ?? "");
  expect(after).not.toBeNull();
  expect(Math.abs((after ?? 0) - (before?.top ?? 0))).toBeLessThanOrEqual(2);
  expect(await calls(id)).toHaveLength(2);
  expect((await calls(id))[1]).toMatch(/^limit=20&before=\d+$/);
});

test("the pages grow, the last one ends the log, and the row that asks for more is gone", async ({
  page,
  history,
  calls,
}) => {
  await history("windowed", { initialTurns: 10, pageTurns: 10 });
  const id = await seedLongThread(TURNS);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(10);
  const row = page.locator('[data-slot="aui_earlier"]');
  await expect(row).toHaveCount(1);

  // 10, then 10 more, then 20, then the remaining 20
  for (const total of [20, 40, 60]) {
    await toTop(page);
    await expect(page.locator(USER)).toHaveCount(total);
  }
  await expect(row).toHaveCount(0);
  const asked = await calls(id);
  expect(asked.map((q) => q.split("&")[0])).toEqual([
    "limit=10",
    "limit=10",
    "limit=20",
    "limit=40",
  ]);
  // every turn is there, once, in order
  const texts = await page.locator(USER).allTextContents();
  expect(new Set(texts).size).toBe(TURNS);
});

test("the turns of the activity panel are numbered by the whole thread, and keep their numbers when older turns load", async ({
  page,
  history,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(12);
  const turns = activityTab(page).getByRole("region", { name: /^Turn \d+ · / });
  // 48 turns came before the 12 held: the first of them is the 49th, not the first
  await expect(turns).toHaveCount(12);
  await expect(turns.first()).toHaveAccessibleName(/^Turn 49 · /);
  await expect(turns.last()).toHaveAccessibleName(/^Turn 60 · /);

  await toTop(page);
  await expect(page.locator(USER)).toHaveCount(32);
  await expect(turns).toHaveCount(32);
  await expect(turns.first()).toHaveAccessibleName(/^Turn 29 · /);
  // the turns that were held keep their numbers
  await expect(activityTab(page).getByRole("region", { name: /^Turn 49 · / })).toHaveCount(1);
  await expect(turns.last()).toHaveAccessibleName(/^Turn 60 · /);
});

test("a page that cannot be read is said, and Retry reads it", async ({
  page,
  history,
  request,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(12);

  await request.post(`${MOCK_URL}/__mock/history-fail?times=1&status=503`);
  await toTop(page);
  const alert = page.getByRole("alert").filter({ hasText: "Could not load earlier messages" });
  await expect(alert).toBeVisible();
  // what was shown is still there
  await expect(page.locator(USER)).toHaveCount(12);
  await alert.getByRole("button", { name: "Retry" }).click();
  await expect(page.locator(USER)).toHaveCount(32);
  await expect(alert).toHaveCount(0);
});

test("a message sent after the thread was opened from its history is answered, and the older turns still come", async ({
  page,
  history,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(12);

  await page.getByLabel("Message").fill("Fix the next thing");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("Fix the next thing", { exact: true })).toBeVisible();
  await expect(page.locator(USER)).toHaveCount(13);
  await expect(badge(page)).toHaveText("Done", { timeout: 60_000 });

  await toTop(page);
  await expect(page.locator(USER)).toHaveCount(33);
  // the new turn is still the last, once
  const texts = await page.locator(USER).allTextContents();
  expect(texts.at(-1)).toContain("Fix the next thing");
  expect(texts.filter((t) => t.includes("Fix the next thing"))).toHaveLength(1);
});

test("older turns asked for while the agent works wait for it, and come when it is done", async ({
  page,
  history,
}) => {
  await history("windowed");
  const id = await seedLongThread(TURNS);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(12);

  await page.getByLabel("Message").fill("Fix the next thing");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("Fix the next thing", { exact: true })).toBeVisible();
  await toTop(page);
  await expect(
    page.getByRole("status").filter({ hasText: "will load when the agent is done" }),
  ).toBeVisible();
  // the person's message of the open run is not lost to the import
  await expect(badge(page)).toHaveText("Done", { timeout: 60_000 });
  await expect(page.locator(USER)).toHaveCount(33);
  const texts = await page.locator(USER).allTextContents();
  expect(texts.filter((t) => t.includes("Fix the next thing"))).toHaveLength(1);
});

test("an orchestrator that serves the history but is not asked to open from it replays the log", async ({
  page,
  history,
  calls,
}) => {
  await history("on");
  const id = await seedLongThread(30);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(30, { timeout: 120_000 });
  expect(await calls(id)).toEqual([]);
});

test("an orchestrator without the history route replays the log, as before", async ({
  page,
  history,
  calls,
}) => {
  await history("off");
  const id = await seedLongThread(30);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(30, { timeout: 120_000 });
  expect(await calls(id)).toEqual([]);
});

test("a page that cannot be read at the start opens the thread the old way", async ({
  page,
  history,
  request,
}) => {
  await history("windowed");
  const id = await seedLongThread(30);
  await request.post(`${MOCK_URL}/__mock/history-fail?times=1&status=500`);
  await open(page, id);
  await expect(page.locator(USER)).toHaveCount(30, { timeout: 120_000 });
});
