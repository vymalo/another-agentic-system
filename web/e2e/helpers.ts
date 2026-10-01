import { expect, type Locator, type Page } from "@playwright/test";

/** The app under test: the production build `pnpm test:e2e` starts. */
// nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
export const BASE_URL = "http://127.0.0.1:3000";

/** ADR 0008: the release channel travels in `forwardedProps` under this URI. */
export const RELEASE_CHANNELS_URI = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

export const THREAD_URL = /\/threads\/[0-9a-f-]{36}$/;

/** Start a thread from the home page with the given agent (default: the first, Coder). */
export async function startThread(page: Page, text: string, agent?: string) {
  await page.goto("/");
  const select = page.getByLabel("Agent");
  await expect(select).toBeVisible();
  if (agent) await select.selectOption({ label: agent });
  await page.getByLabel("Message").fill(text);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page).toHaveURL(THREAD_URL);
  await expectNoHorizontalScroll(page);
}

/** The page never scrolls sideways, on a phone or anywhere else. */
export async function expectNoHorizontalScroll(page: Page) {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  );
  expect(overflow, "horizontal overflow in px").toBeLessThanOrEqual(0);
}

/**
 * Shared locators. Specs go through these (never a CSS selector), so a markup change touches this
 * file only. They rely on roles and accessible names: labels `Agent`, `Release`, `Message`; buttons
 * `Send`, `Stop`, `Threads` (phone), `Thread options`, `Load older`, `Retry`; the link `New chat`;
 * the `log` "Conversation"; the `status` "Thread state: ..."; the `navigation` "Threads"; a check
 * or CI report is a step, a `listitem` named "Check: …" or "CI: …".
 */
export const badge = (page: Page) => page.getByRole("status", { name: /^Thread state:/ });

export const conversation = (page: Page) => page.getByRole("log", { name: "Conversation" });

/** The page's own error alerts (Next's route announcer is a `role="alert"` too: excluded). */
export const errorLine = (page: Page) =>
  page.getByRole("alert").and(page.locator(":not(#__next-route-announcer__)"));

/** The thread list. On a phone it is collapsed until `openThreadList` runs. */
export const threadList = (page: Page) => page.getByRole("navigation", { name: "Threads" });

/** One entry per thread in the list, in order (the "New chat" link and "Load older" excluded). */
export const threadRows = (page: Page) => threadList(page).getByRole("listitem");

/** Opens the thread list where it is a sheet (a phone); a no-op where it is always visible. */
export async function openThreadList(page: Page) {
  const toggle = page.getByRole("button", { name: "Threads" });
  if (await toggle.isVisible()) await toggle.click();
}

/** The agent turn of the log whose text contains `text` (its words, its steps and its cards). */
export const agentMessage = (page: Page, text: string) =>
  conversation(page).locator('[data-slot="agent-turn"]', { hasText: text });

/** `coder · coder-r47`: the agent's name and revision, once, at the top of its turn. */
export const actorLabel = (turn: Locator) => turn.locator('[data-slot="actor-label"]').first();

/** "Export JSON", an item of the thread's overflow menu: opens the menu and returns the item. */
export async function exportMenuItem(page: Page): Promise<Locator> {
  const item = page.getByRole("menuitem", { name: /Export JSON|Exporting…/ });
  if (!(await item.isVisible())) await page.getByRole("button", { name: "Thread options" }).click();
  await expect(item).toBeVisible();
  return item;
}

/** The option a native `<select>` shows. */
export const selectedOption = (select: Locator) => select.getByRole("option", { selected: true });

/**
 * The options of the release picker's "Revisions" group. Playwright's role engine does not expose
 * options inside an `<optgroup>`, so this is the one CSS selector.
 */
export const revisionOptions = (select: Locator) =>
  select.locator('optgroup[label="Revisions"] option');
