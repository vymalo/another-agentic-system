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
  await expect(agentPicker(page)).toBeVisible();
  if (agent) await chooseAgent(page, agent);
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
 * file only. They rely on roles and accessible names: the button `Agent: <name>` (the picker in the
 * top bar, a menu), the label `Message`; buttons `Send`, `Stop`, `Threads` (phone), `Thread
 * options`, `Load older`, `Retry`; the link `New chat`; the `log` "Conversation"; the `status`
 * "Thread state: ..."; the `navigation` "Threads"; a check or CI report is a step, a `listitem`
 * named "Check: …" or "CI: …".
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

/** The agent picker of the top bar: a button, "Agent: Coder", that opens a menu. */
export const agentPicker = (page: Page) => page.getByRole("button", { name: /^Agent:/ });

/** The open agent menu. */
export const agentMenu = (page: Page) => page.getByRole("menu");

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** An item of the agent menu (an agent, a channel or a revision) by the start of its name. */
export const agentMenuItem = (page: Page, name: string) =>
  agentMenu(page).getByRole("menuitemradio", { name: new RegExp(`^${escapeRegExp(name)}\\b`) });

/**
 * Opens the agent menu (when it is not open) and returns it. Whether it is open is the button's
 * `aria-expanded`: a menu that was just closed stays on the page for its exit animation. (While it
 * is open the rest of the page is `aria-hidden`, the button included, hence `includeHidden`.)
 */
export async function openAgentMenu(page: Page) {
  const button = page.getByRole("button", { name: /^Agent:/, includeHidden: true });
  if ((await button.getAttribute("aria-expanded")) !== "true") await button.click();
  const menu = agentMenu(page);
  await expect(menu).toBeVisible();
  return menu;
}

/** Closes the agent menu with Escape and waits for it to be gone; the focus is back on its button. */
export async function closeAgentMenu(page: Page) {
  await page.keyboard.press("Escape");
  await expect(agentMenu(page)).toBeHidden();
  await expect(agentPicker(page)).toBeFocused();
}

/** Chooses an agent of a new chat by its name: opens the menu, clicks the agent; the menu closes. */
export async function chooseAgent(page: Page, name: string) {
  await openAgentMenu(page);
  await agentMenuItem(page, name).click();
  await expect(agentMenu(page)).toBeHidden();
}

/** Chooses a release (a channel or a revision) of the agent of a new chat. */
export async function chooseRelease(page: Page, value: string) {
  await openAgentMenu(page);
  await agentMenuItem(page, value).click();
  await expect(agentMenu(page)).toBeHidden();
}

/**
 * The right-hand panel of a thread: the header's toggle ("Thread details", `aria-expanded`), the panel
 * itself (a complementary landmark when docked, a dialog when it is a sheet; both are named "Thread
 * details"), its tabs and the separator that resizes it.
 */
export const panelToggle = (page: Page) => page.getByRole("button", { name: "Thread details" });
export const panel = (page: Page) =>
  page
    .getByRole("complementary", { name: "Thread details" })
    .or(page.getByRole("dialog", { name: "Thread details" }));
export const panelTab = (page: Page, name: "Activity" | "Sources") =>
  panel(page).getByRole("tab", { name: new RegExp(`^${name}`) });
export const panelResizer = (page: Page) =>
  page.getByRole("separator", { name: "Resize the details panel" });

/**
 * The Activity tab of the panel: what the agents did, turn by turn, as a tree. The steps of a
 * turn are not in the conversation (it keeps one line per turn), so a spec that looks at a step
 * looks here: `showActivity` first, because on a phone the panel is a sheet that has to be opened.
 */
export const activityTab = (page: Page) => panel(page).getByRole("tabpanel", { name: "Activity" });

/** The one line an agent turn keeps in the conversation: the button that opens its steps. */
export const turnSummaries = (page: Page) =>
  // not through the log's role: behind a phone's sheet the page is hidden from the accessibility tree
  page.locator('[role="log"][aria-label="Conversation"] [data-slot="turn-summary"]');

/** The sections of the Activity tab, one per agent turn that did something. */
export const turnSections = (page: Page) => activityTab(page).locator('[data-slot="turn-section"]');

/**
 * Makes the Activity tab show and returns it: the docked panel is open already on a wide window
 * (and is opened when the person closed it), a phone's sheet is opened by the header's toggle.
 */
export async function showActivity(page: Page): Promise<Locator> {
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await expect(panel(page)).toBeVisible();
  const tab = panelTab(page, "Activity");
  if ((await tab.getAttribute("aria-selected")) !== "true") await tab.click();
  return activityTab(page);
}

/** Closes the sheet of a phone, so the page behind it can be read and used again; a docked panel stays. */
export async function hideActivity(page: Page) {
  const sheet = page.getByRole("dialog", { name: "Thread details" });
  if (await sheet.isVisible()) {
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
  }
}
