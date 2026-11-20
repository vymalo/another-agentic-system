import { expect, type Page, test } from "@playwright/test";
import {
  agentPicker,
  aPerson,
  expectNoHorizontalScroll,
  MOCK_URL,
  makeThread,
  openThreadList,
  type Person,
  threadList,
} from "./helpers";

/*
 * Searching a person's own threads (ADR 0068, web/DESIGN.md "Search"), on the mock, which is the second implementation of
 * docs/api/thread-search-v1.md: the box under "New chat", the results in the rail's place, the keys, the shortcut, the states and what
 * never leaves the body of the POST. Each test is a person of its own (`aPerson`), so the threads found are theirs, and a word made
 * up for the test is only in their threads.
 *
 * The mock's `talk …` script is the one whose agent says something of its own ("Plan: add a test", a final message), so a thread
 * made with it has a person's message with the made-up word and an agent's answer with the word "plan".
 */

let counter = 0;
/** A word no other test has: letters and digits only, so it is one word. */
const word = (info: { workerIndex: number }) =>
  `zq${info.workerIndex}x${++counter}${Date.now() % 1000}`;

const box = (page: Page) => page.getByRole("combobox", { name: "Search chats" });
const options = (page: Page) => page.getByRole("option");
/** The first hit that is a message, chosen by what it is (not by its place in the list). */
const messageHit = (page: Page) =>
  page.locator('[data-slot="search-hit"][data-field="message"]:visible').first();
const live = (page: Page) => page.locator('[role="status"][aria-live="polite"]:visible');

/** The home page with the list open (a sheet on a phone) and the box found. */
async function open(page: Page) {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect(box(page)).toBeVisible();
}

async function twoThreads(person: Person, w: string) {
  const a = await makeThread(person, `talk ${w} alpha`);
  const b = await makeThread(person, `talk ${w} bravo`);
  return { a, b };
}

test("typing finds the person's threads and marks the words that matched, in the rail's place", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await twoThreads(person, w);
  await open(page);
  await expect(threadList(page).getByRole("heading", { name: "Chats", exact: true })).toBeVisible();
  await box(page).fill(w);
  const list = page.getByRole("listbox", { name: "Search results" });
  await expect(list).toBeVisible();
  // the list is not drawn while the results are
  await expect(threadList(page).getByRole("heading", { name: "Chats", exact: true })).toHaveCount(
    0,
  );
  // two threads, each with its title (a hit) and the person's message (a hit)
  await expect(options(page)).toHaveCount(4);
  const marks = list.locator("mark");
  await expect(marks).toHaveCount(4);
  for (const text of await marks.allTextContents()) expect(text).toBe(w);
  // the agent's answer does not have the word: no hit there
  await expect(list).not.toContainText("Plan: add a test");
  await expectNoHorizontalScroll(page);
});

test("a word of the agent's answer is found, and the hit says who said it", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await makeThread(person, `talk ${w}`);
  await open(page);
  await box(page).fill("plan");
  await expect(options(page)).toHaveCount(1);
  const hit = options(page).first();
  await expect(hit).toContainText("Agent");
  await expect(hit.locator("mark")).toHaveText("Plan");
});

test("the arrow keys walk the hits and Enter opens a message with its turn focused", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  const { a } = await twoThreads(person, w);
  await open(page);
  await box(page).fill(`${w} alpha`);
  await expect(options(page)).toHaveCount(2);
  // title first, then the message
  await box(page).press("ArrowDown");
  await expect(options(page).nth(0)).toHaveAttribute("aria-selected", "true");
  await box(page).press("ArrowDown");
  const second = options(page).nth(1);
  await expect(second).toHaveAttribute("aria-selected", "true");
  await expect(box(page)).toHaveAttribute(
    "aria-activedescendant",
    (await second.getAttribute("id")) ?? "",
  );
  await expect(second).toContainText("You");
  await box(page).press("Enter");
  await expect(page).toHaveURL(new RegExp(`/threads/${a}#m-\\d+$`));
  // the message the hit names is the one that has the focus
  const seq = new URL(page.url()).hash.replace("#m-", "");
  const message = page.locator(`#m-${seq}`);
  await expect(message).toBeFocused();
  await expect(message).toContainText(`talk ${w} alpha`);
});

test("an agent's answer opens at the turn that has it: the person's message that started it has the focus", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  const a = await makeThread(person, `talk ${w}`);
  await open(page);
  await box(page).fill("plan");
  await expect(options(page)).toHaveCount(1);
  await options(page).first().click();
  await expect(page).toHaveURL(new RegExp(`/threads/${a}#m-\\d+$`));
  // the answer has no element of its own: the page settles for the message before it
  const message = page.locator('[data-slot="user-message"]');
  await expect(message).toContainText(`talk ${w}`);
  await expect(message).toBeFocused({ timeout: 8_000 });
});

test("a hit of the open thread goes to its message again", async ({ page }, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  const a = await makeThread(person, `talk ${w}`);
  await page.goto(`/threads/${a}`);
  await expect(page.locator('[data-slot="user-message"]')).toBeVisible();
  await openThreadList(page);
  await box(page).fill(w);
  await messageHit(page).click();
  await expect(page).toHaveURL(new RegExp(`/threads/${a}#m-\\d+$`));
  await expect(page.locator('[data-slot="user-message"]')).toBeFocused({ timeout: 8_000 });
});

test("Escape clears the box and brings the rail back; the second gives the focus back to where it was", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await twoThreads(person, w);
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  const composer = page.getByLabel("Message", { exact: true });
  await composer.focus();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(box(page)).toBeFocused();
  await page.keyboard.type(w);
  await expect(options(page).first()).toBeVisible();
  await box(page).press("Escape");
  await expect(box(page)).toHaveValue("");
  await expect(page.getByRole("listbox")).toHaveCount(0);
  await expect(threadList(page).getByRole("heading", { name: "Chats", exact: true })).toBeVisible();
  await expect(box(page)).toBeFocused();
  await box(page).press("Escape");
  await expect(composer).toBeFocused();
});

test("with no `search` in the configuration there is no box and the shortcut does nothing", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await page.route("**/api/config", async (route) => {
    const res = await route.fetch();
    const body = (await res.json()) as Record<string, unknown>;
    delete body.search;
    await route.fulfill({ response: res, json: body });
  });
  // the page records whether anything took the key's default, once every listener has run
  await page.addInitScript(() => {
    const w = window as unknown as { keyTaken: boolean[] };
    w.keyTaken = [];
    window.addEventListener("keydown", (e) => {
      if (e.key.toLowerCase() === "k") setTimeout(() => w.keyTaken.push(e.defaultPrevented), 0);
    });
  });
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect(threadList(page).getByRole("link", { name: "New chat" })).toBeVisible();
  await expect(page.getByRole("combobox", { name: "Search chats" })).toHaveCount(0);
  await page.keyboard.press("ControlOrMeta+k");
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { keyTaken: boolean[] }).keyTaken))
    .toEqual([false]);
});

test("a query with no match says so, with the words searched for", async ({ page }, info) => {
  const person = await aPerson(page, info);
  await makeThread(person, "talk something else");
  await open(page);
  await box(page).fill("qqqzzzvvv");
  await expect(page.getByText("No chats match “qqqzzzvvv”.").first()).toBeVisible();
  await expect(options(page)).toHaveCount(0);
  await expect(live(page)).toHaveText("No chats match “qqqzzzvvv”.");
});

test("a query with no word of two letters says what search needs, and finds nothing", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await open(page);
  await box(page).fill("a");
  await expect(page.getByText(/at least two letters or digits/).first()).toBeVisible();
  await expect(options(page)).toHaveCount(0);
});

test("the live region says how many chats were found", async ({ page }, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await twoThreads(person, w);
  await open(page);
  await box(page).fill(w);
  await expect(live(page)).toHaveText("2 chats found");
  await box(page).fill(`${w} alpha`);
  await expect(live(page)).toHaveText("1 chat found");
});

test("an archived thread is found and marked as archived", async ({ page }, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  const { a } = await twoThreads(person, w);
  const res = await fetch(`${MOCK_URL}/api/threads/${a}/rail`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json", Cookie: person.cookie },
    body: JSON.stringify({ archived: true }),
  });
  expect(res.ok).toBe(true);
  await open(page);
  await box(page).fill(`${w} alpha`);
  await expect(options(page)).toHaveCount(2);
  await expect(options(page).first()).toContainText("Archived");
});

test("an error says so with Retry, and Retry searches again", async ({ page }, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await makeThread(person, `talk ${w}`);
  let failing = true;
  await page.route("**/api/search/threads", async (route) => {
    if (!failing) return route.continue();
    await route.fulfill({
      status: 500,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: "Internal Server Error", status: 500 }),
    });
  });
  await open(page);
  await box(page).fill(w);
  const alert = page.getByRole("alert").filter({ hasText: "Could not search your chats." });
  await expect(alert).toBeVisible();
  failing = false;
  await alert.getByRole("button", { name: "Retry" }).click();
  await expect(options(page).first()).toBeVisible();
  await expect(alert).toHaveCount(0);
});

test("a busy server (503) is told, and a rate limit (429) waits its Retry-After and tries again", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await makeThread(person, `talk ${w}`);
  let answer: "busy" | "limited" | "pass" = "busy";
  await page.route("**/api/search/threads", async (route) => {
    if (answer === "pass") return route.continue();
    if (answer === "limited") answer = "pass";
    await route.fulfill({
      status: answer === "busy" ? 503 : 429,
      headers: { "Retry-After": "1" },
      contentType: "application/problem+json",
      body: JSON.stringify({ title: "no", status: answer === "busy" ? 503 : 429 }),
    });
  });
  await open(page);
  await box(page).fill(w);
  await expect(
    page.getByRole("alert").filter({ hasText: "Search is busy right now" }),
  ).toBeVisible();
  answer = "limited";
  await box(page).fill(`${w} `);
  await expect(page.getByText(/You are searching too fast/).first()).toBeVisible();
  // after the second it asked for, the same search is sent again and answered
  await expect(options(page).first()).toBeVisible({ timeout: 8_000 });
});

test("the words are in the body of a POST and nowhere else: not the URL, not the storage", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const w = word(info);
  await makeThread(person, `talk ${w}`);
  const seen: { url: string; method: string; body: unknown }[] = [];
  page.on("request", (request) => {
    if (request.url().includes("/api/search/")) {
      seen.push({ url: request.url(), method: request.method(), body: request.postDataJSON() });
    }
  });
  await open(page);
  await box(page).fill(w);
  await expect(options(page).first()).toBeVisible();
  expect(seen.length).toBeGreaterThan(0);
  for (const request of seen) {
    expect(request.method).toBe("POST");
    expect(request.url).not.toContain(w);
    expect(new URL(request.url).search).toBe("");
  }
  expect(seen.at(-1)?.body).toMatchObject({ q: w });
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }));
  expect(stored).not.toContain(w);
  // the box is not a form that a browser keeps or submits
  await expect(box(page)).toHaveAttribute("autocomplete", "off");
});

test("Ctrl or ⌘ with K focuses the box from the composer, and Escape twice gives the focus back to it", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  const composer = page.getByLabel("Message", { exact: true });
  await composer.focus();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(box(page)).toBeFocused();
  await expect(box(page)).toHaveAttribute("aria-keyshortcuts", "Control+K Meta+K");
  await page.keyboard.type("hello");
  await page.keyboard.press("Escape");
  await expect(box(page)).toHaveValue("");
  await page.keyboard.press("Escape");
  await expect(composer).toBeFocused();
});

test("the shortcut opens a sidebar that was collapsed", async ({ page, isMobile }, info) => {
  test.skip(isMobile, "a phone has a sheet, not a sidebar to collapse");
  await aPerson(page, info);
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await page.getByRole("button", { name: "Close sidebar" }).click();
  await expect(page.getByRole("button", { name: "Open sidebar" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(box(page)).toBeFocused();
  await expect(page.getByRole("button", { name: "Close sidebar" })).toBeVisible();
});

test.describe("a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test("the shortcut opens the sheet and the box has the focus; a hit closes it and opens the thread", async ({
    page,
  }, info) => {
    const person = await aPerson(page, info);
    const w = word(info);
    const a = await makeThread(person, `talk ${w}`);
    await page.goto("/");
    await expect(agentPicker(page)).toBeVisible();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(box(page)).toBeVisible();
    await expect(box(page)).toBeFocused();
    await page.keyboard.type(w);
    await expect(options(page).first()).toBeVisible();
    await expectNoHorizontalScroll(page);
    await options(page).first().click();
    await expect(page).toHaveURL(new RegExp(`/threads/${a}$`));
    await expect(box(page)).toBeHidden();
  });

  test("a hit of the open thread closes the sheet", async ({ page }, info) => {
    const person = await aPerson(page, info);
    const w = word(info);
    const a = await makeThread(person, `talk ${w}`);
    await page.goto(`/threads/${a}`);
    await expect(page.locator('[data-slot="user-message"]')).toBeVisible();
    await openThreadList(page);
    await box(page).fill(w);
    await messageHit(page).click();
    await expect(page).toHaveURL(new RegExp(`/threads/${a}#m-\\d+$`));
    await expect(box(page)).toBeHidden();
  });

  test("the first Escape clears a query without closing the sheet; the next closes it", async ({
    page,
  }, info) => {
    await aPerson(page, info);
    await page.goto("/");
    await openThreadList(page);
    await box(page).fill("hello");
    await box(page).press("Escape");
    await expect(box(page)).toHaveValue("");
    await expect(box(page)).toBeVisible();
    await box(page).press("Escape");
    await expect(box(page)).toBeHidden();
  });
});
