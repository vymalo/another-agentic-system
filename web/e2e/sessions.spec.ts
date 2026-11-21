import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  animationsDone,
  aPerson,
  badge,
  conversation,
  expectNoHorizontalScroll,
  hideActivity,
  MOCK_URL,
  panel,
  panelTab,
  panelToggle,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * Subagents in the parent's chat (child-sessions/v1, ADR 0072, web/README.md "Subagents"), against the
 * mock: the script `helpers` opens two helper sessions, which the agent's run draws as one card each
 * (a card is said again in the runs of its later events and is drawn once, in the run that opened it,
 * with the state said last), and "Open" goes to the helper's own thread.
 */

const cards = (page: import("@playwright/test").Page) =>
  conversation(page).locator('[data-slot="session-card"]');

test("each subagent is one card, in its state, and Open goes to its thread", async ({ page }) => {
  await startThread(page, "helpers");
  const parentUrl = page.url();
  await expect(cards(page)).toHaveCount(2);

  const planner = cards(page).filter({ hasText: "planner" });
  await expect(planner).toContainText("Plan the migration");
  // the late reports (a round, a block, a done, a close) were said as runs of their own: no turn for them
  await expect(planner.locator('[data-slot="session-state"]')).toHaveText("Closed");
  const researcher = cards(page).filter({ hasText: "researcher" });
  await expect(researcher).toContainText("Check the sources");
  await expect(researcher.locator('[data-slot="session-state"]')).toHaveText("Closed");
  await expect(conversation(page).locator('[data-slot="agent-turn"]')).toHaveCount(1);

  const open = planner.getByRole("link", { name: /^Open/ });
  // the same tab: the arrow is a mark of where it goes, not of a new window
  await expect(open).not.toHaveAttribute("target", /./);
  await open.click();
  // the parent's address matches THREAD_URL too: wait for the child's
  await expect(page).not.toHaveURL(parentUrl);
  await expect(page).toHaveURL(THREAD_URL);
  await expect(page.getByRole("log", { name: "Conversation" })).toBeVisible();
});

test("the cards are keyboard-reachable and pass axe in the page's scheme, without sideways scroll", async ({
  page,
}) => {
  await startThread(page, "helpers");
  await expect(cards(page)).toHaveCount(2);
  await animationsDone(page);
  const link = cards(page).first().getByRole("link", { name: /^Open/ });
  await link.focus();
  await expect(link).toBeFocused();
  await expectNoHorizontalScroll(page);
  const { violations } = await new AxeBuilder({ page })
    .include('[data-slot="session-card"]')
    .analyze();
  expect(violations).toEqual([]);
});

test("a start a person asked for is one muted line, which turns into a link to the subagent", async ({
  page,
}) => {
  await startThread(page, "hello");
  await expect(page.getByRole("status", { name: /^Thread state:/ })).toContainText("Done");
  const parent = THREAD_URL.exec(page.url()) ? (page.url().split("/").pop() ?? "") : "";
  const answered = await page.request.post(`/api/threads/${parent}/sessions`, {
    data: { id: uuidv7(), agent: "planner", description: "Plan the docs", text: "Go." },
  });
  expect(answered.status()).toBe(202);

  const line = conversation(page).locator('[data-slot="start-line"]');
  await expect(line).toHaveCount(1);
  await expect(line).toContainText("You started planner: Plan the docs");
  const open = line.getByRole("link", { name: /^Open/ });
  await expect(open).toBeVisible();
  // the session it opened is a card of its own, once, with the words "Started by you"
  await expect(cards(page)).toHaveCount(1);
  await expect(cards(page)).toContainText("Started by you");
  await open.click();
  await expect(page).not.toHaveURL(new RegExp(`${parent}$`));
});

/*
 * The Subagents tab of the right-hand panel and its two dialogs (S4e): a person starts a subagent from
 * the tab, sees the start line and the card in the chat and the row in the tab, closes it from the row;
 * a start the agent refuses is one line with "Try again", which brings the draft back; an agent whose
 * card does not list the extension has no tab and no button.
 */

/** Opens the panel (a docked one is open already; a phone's sheet is opened by its toggle) on the Subagents tab. */
async function showSubagents(page: import("@playwright/test").Page) {
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await expect(panel(page)).toBeVisible();
  await panelTab(page, "Subagents").click();
  return panel(page).getByRole("tabpanel", { name: /^Subagents/ });
}

const dialog = (page: import("@playwright/test").Page, name: string) =>
  page.getByRole("dialog", { name });

async function fillStart(page: import("@playwright/test").Page, description: string, text: string) {
  const form = dialog(page, "New subagent");
  await form.getByLabel("What it is for").fill(description);
  await form.getByLabel("What to ask it").fill(text);
}

test("start a subagent from the tab: a line and a card in the chat, a row in the tab, and Close from the row", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await startThread(page, "hello");
  await expect(badge(page)).toContainText("Done");
  const tab = await showSubagents(page);
  await expect(tab).toContainText("No subagents yet");

  const opener = tab.getByRole("button", { name: "New subagent" });
  await opener.click();
  const form = dialog(page, "New subagent");
  await expect(form).toBeVisible();
  // the two helpers of the card, each with its own words; "Close when done" is on
  await expect(form.getByRole("radio")).toHaveCount(2);
  await expect(form).toContainText("Breaks a request into steps.");
  await expect(form.getByRole("checkbox", { name: /Close when done/ })).toBeChecked();
  await fillStart(page, "Plan the docs", "Go.");
  await expect(form.getByTestId("description-count")).toHaveText("13/80");
  // keep it open, so that the row has a Close to press
  await form.getByRole("checkbox", { name: /Close when done/ }).uncheck();
  await form.getByRole("button", { name: "Start subagent" }).click();
  await expect(form).toBeHidden();
  await expect(opener).toBeFocused();

  // the tab: the row, in words, that stays open
  const row = tab.locator('[data-slot="subagent-row"]');
  await expect(row).toHaveCount(1);
  await expect(row).toContainText("Plan the docs");
  await expect(row).toContainText("Stays open");
  await expect(row.locator('[data-slot="subagent-state"]')).toHaveText("Idle");
  await expect(panelTab(page, "Subagents")).toContainText("1");

  // the chat: one muted line, which turned into a link, and the card of the session
  await hideActivity(page);
  const line = conversation(page).locator('[data-slot="start-line"]');
  await expect(line).toContainText("You started planner: Plan the docs");
  await expect(line.getByRole("link", { name: /^Open/ })).toBeVisible();
  await expect(cards(page)).toHaveCount(1);

  // Close: asked first, idle closes at once, and the row folds into "1 closed"
  const again = await showSubagents(page);
  await again.getByRole("button", { name: "Close planner" }).click();
  const closing = dialog(page, "Close subagent");
  await expect(closing).toContainText("Close planner? It takes no more messages.");
  await closing.getByRole("button", { name: "Close subagent" }).click();
  await expect(closing).toBeHidden();
  await expect(again.getByText("1 closed")).toBeVisible();
  await expect(again.getByRole("button", { name: "Close planner" })).toHaveCount(0);
});

test("a start the agent refuses is one line with Try again, which brings the draft back", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const session = person.cookie.split("=")[1] ?? "";
  const set = (query: string) =>
    page.request.post(`${MOCK_URL}/__mock/config?${query}&session=${session}`);
  expect((await set("sessionStarts=refuse")).ok()).toBe(true);
  await startThread(page, "hello");
  await expect(badge(page)).toContainText("Done");
  const tab = await showSubagents(page);
  await tab.getByRole("button", { name: "New subagent" }).click();
  await fillStart(page, "Plan the docs", "A prompt that must not be lost.");
  await dialog(page, "New subagent").getByRole("button", { name: "Start subagent" }).click();
  await expect(dialog(page, "New subagent")).toBeHidden();

  await hideActivity(page);
  const line = conversation(page).locator('[data-slot="start-line"]');
  await expect(line).toContainText("Did not start: max_working");
  await line.getByRole("button", { name: "Try again" }).click();
  const form = dialog(page, "New subagent");
  await expect(form.getByLabel("What it is for")).toHaveValue("Plan the docs");
  await expect(form.getByLabel("What to ask it")).toHaveValue("A prompt that must not be lost.");

  // the agent can take it now
  expect((await set("sessionStarts=open")).ok()).toBe(true);
  await form.getByRole("button", { name: "Start subagent" }).click();
  await expect(form).toBeHidden();
  await expect(conversation(page).locator('[data-slot="start-line"]')).toHaveCount(2);
  await expect(
    conversation(page).locator('[data-slot="start-line"][data-state="opened"]'),
  ).toHaveCount(1);
});

test("the dialog and the tab pass axe, and the dialog is used from the keyboard", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await startThread(page, "hello");
  await expect(badge(page)).toContainText("Done");
  const tab = await showSubagents(page);
  await animationsDone(page);
  const { violations } = await new AxeBuilder({ page })
    .include('[data-slot="sessions-panel"]')
    .analyze();
  expect(violations).toEqual([]);

  await tab.getByRole("button", { name: "New subagent" }).focus();
  await page.keyboard.press("Enter");
  const form = dialog(page, "New subagent");
  await expect(form).toBeVisible();
  await animationsDone(page);
  expect(
    (await new AxeBuilder({ page }).include('[role="dialog"][aria-labelledby]').analyze())
      .violations,
  ).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(form).toBeHidden();
  await expect(tab.getByRole("button", { name: "New subagent" })).toBeFocused();
});

test("an agent whose card does not list the extension has no Subagents tab and no button", async ({
  page,
}) => {
  await startThread(page, "hello", "Reviewer");
  await expect(badge(page)).toContainText("Done");
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await expect(panelTab(page, "Activity")).toBeVisible();
  await expect(panelTab(page, "Subagents")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "New subagent" })).toHaveCount(0);
});

/*
 * A subagent's own thread (S4f): the banner under the top bar, a message box that follows the session
 * (idle starts the next round, a working one is steered, Stop, and Stop and send only when it is kept
 * open), Close, and the notice that replaces the box once the session is closed. A first word of `hold`
 * is the mock's round that waits for a release.
 */

type Page = import("@playwright/test").Page;

/** Starts a subagent from the parent's chat through the API and goes to its own thread; its id. */
async function inChild(page: Page, keepOpen: boolean, text: string): Promise<string> {
  await startThread(page, "hello");
  await expect(badge(page)).toContainText("Done");
  const parent = page.url().split("/").pop() ?? "";
  const started = await page.request.post(`/api/threads/${parent}/sessions`, {
    data: { id: uuidv7(), agent: "planner", description: "Plan the docs", text, keepOpen },
  });
  expect(started.status()).toBe(202);
  await hideActivity(page);
  await conversation(page)
    .locator('[data-slot="start-line"]')
    .getByRole("link", { name: /^Open/ })
    .click();
  await expect(page).not.toHaveURL(new RegExp(`${parent}$`));
  await expect(page.locator('[data-slot="child-banner"]')).toBeVisible();
  return page.url().split("/").pop() ?? "";
}

const bannerOf = (page: Page) => page.locator('[data-slot="child-banner"]');
const childState = (page: Page) => bannerOf(page).locator('[data-slot="child-state"]');
const messageBox = (page: Page) => page.getByLabel("Message", { exact: true });
/**
 * Lets the held round of `child` go on. The mock answers 409 until the round is at its hold (the step
 * before it is what the page showed), so this asks again until it is let go, and never releases nothing.
 */
const release = async (child: string) => {
  await expect
    .poll(
      async () =>
        (await fetch(`${MOCK_URL}/__mock/release?thread=${child}`, { method: "POST" })).status,
    )
    .toBe(204);
};

async function say(page: Page, text: string) {
  await messageBox(page).fill(text);
  await page.getByRole("button", { name: "Send", exact: true }).click();
}

test("a kept-open subagent: banner, a message starts the next round, steer, Stop, Stop and send, Close", async ({
  page,
}, info) => {
  await aPerson(page, info);
  const child = await inChild(page, true, "hold first");
  await expect(bannerOf(page)).toContainText("Stays open");
  await expect(bannerOf(page).getByRole("link", { name: /^Back to/ })).toHaveAttribute(
    "href",
    /\/threads\/[0-9a-f-]{36}$/,
  );
  await expect(childState(page)).toHaveText("Working");
  await expectNoHorizontalScroll(page);

  // no @, no tools picker in a subagent
  await expect(page.getByRole("button", { name: "Tools" })).toHaveCount(0);

  // steer: a message while the round works is read by it, in one run
  await expect(conversation(page)).toContainText("hold first");
  await say(page, "a note for the round");
  await expect(conversation(page)).toContainText("a note for the round");
  await release(child);
  await expect(childState(page)).toHaveText("Idle");

  // idle: the message starts the next round
  await say(page, "hold second");
  await expect(childState(page)).toHaveText("Working");
  // Stop ends the round; the subagent stays open
  await page.getByRole("button", { name: "Stop", exact: true }).hover();
  await expect(page.getByRole("tooltip")).toContainText(
    "Stops this round, the subagent stays open",
  );
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(childState(page)).toHaveText("Idle");

  // Stop and send: the round stops and the message starts the next one
  await say(page, "hold third");
  await expect(childState(page)).toHaveText("Working");
  await messageBox(page).fill("instead, this");
  await page.getByRole("button", { name: "Delivery options" }).click();
  await page.getByRole("menuitem", { name: /Stop and send/ }).click();
  await expect(conversation(page)).toContainText("instead, this");
  // "instead, this" is not a hold: its round ends by itself, and there is nothing to release
  await expect(childState(page)).toHaveText("Idle");

  // Close: an idle subagent closes at once; the box becomes the notice, with the way back
  await bannerOf(page).getByRole("button", { name: "Close planner" }).click();
  await dialog(page, "Close subagent").getByRole("button", { name: "Close subagent" }).click();
  const notice = page.locator('[data-slot="session-closed-notice"]');
  await expect(notice).toContainText("This subagent is closed and takes no more messages");
  await expect(messageBox(page)).toHaveCount(0);
  await expect(childState(page)).toHaveText("Closed");
  await expect(bannerOf(page).getByRole("button", { name: /^Close/ })).toHaveCount(0);
  await notice.getByRole("link", { name: /^Back to/ }).click();
  await expect(page).not.toHaveURL(new RegExp(child));
  await expect(page).toHaveURL(THREAD_URL);
});

test("a subagent that closes when done has no Stop and send, says what Stop does, and closes with its round", async ({
  page,
}, info) => {
  await aPerson(page, info);
  const child = await inChild(page, false, "hold only");
  await expect(bannerOf(page)).toContainText("Closes when done");
  await expect(childState(page)).toHaveText("Working");
  await messageBox(page).fill("a note");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Delivery options" })).toHaveCount(0);
  await page.getByRole("button", { name: "Stop", exact: true }).hover();
  await expect(page.getByRole("tooltip")).toContainText("Stops this round and closes the subagent");
  // Stop ends the round, and a subagent that is not kept open closes with it
  await page.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(page.locator('[data-slot="session-closed-notice"]')).toBeVisible();
  await expect(childState(page)).toHaveText("Closed");
  await fetch(`${MOCK_URL}/__mock/release?thread=${child}`, { method: "POST" }).catch(() => {});
});

test("a close that waits for the round can be made stronger from the banner", async ({
  page,
}, info) => {
  await aPerson(page, info);
  const child = await inChild(page, true, "hold slowly");
  await bannerOf(page).getByRole("button", { name: "Close planner" }).click();
  await dialog(page, "Close subagent")
    .getByRole("button", { name: "Close when this round ends" })
    .click();
  await expect(childState(page)).toHaveText("Closing…");
  await expect(page.getByText(/Closing… this subagent takes no more messages/)).toBeVisible();
  await bannerOf(page).getByRole("button", { name: "Close planner" }).click();
  const stronger = dialog(page, "Close subagent");
  await expect(stronger.getByRole("button", { name: "Close when this round ends" })).toHaveCount(0);
  await stronger.getByRole("button", { name: "Stop and close" }).click();
  await expect(page.locator('[data-slot="session-closed-notice"]')).toBeVisible();
  await fetch(`${MOCK_URL}/__mock/release?thread=${child}`, { method: "POST" }).catch(() => {});
});

test("the banner and the closed notice pass axe, in the page's scheme", async ({ page }, info) => {
  await aPerson(page, info);
  await inChild(page, true, "hold axe");
  await animationsDone(page);
  expect(
    (await new AxeBuilder({ page }).include('[data-slot="child-banner"]').analyze()).violations,
  ).toEqual([]);
});
