import AxeBuilder from "@axe-core/playwright";
import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  badge,
  conversation,
  expectNoHorizontalScroll,
  openThreadList,
  startThread,
  threadList,
} from "./helpers";

/*
 * Editing a message into a branch (ADR 0029, web/DESIGN.md "Fork and branch"): the pencil under a
 * message of the person opens an editor in its place; sending it makes a new chat that holds the
 * conversation up to that message, the new words and the agent's answer to them. The old words and
 * their answers stay in the other version, and `‹ n/m ›` under the message goes between the two.
 * The mock server plays the contract of `forkThread` and `listBranches`; nothing here waits for a
 * timer: every step waits for what the page shows.
 */

const userMessages = (page: Page) => conversation(page).locator('[data-slot="user-message"]');
const message = (page: Page, text: string) => userMessages(page).filter({ hasText: text });
const picker = (bubble: Locator) => bubble.locator('[data-slot="branch-picker"]');
const editButton = (bubble: Locator) => bubble.getByRole("button", { name: "Edit what you said" });
const sendEdit = (page: Page) =>
  page.locator('[data-slot="message-editor"]').getByRole("button", { name: "Send" });
const editor = (page: Page) => page.getByRole("textbox", { name: "What you said" });
const threadIdOf = (url: string) => new URL(url).pathname.split("/").pop() as string;

/** Says `text` in the open thread and waits for the agent to be done with it. */
async function say(page: Page, text: string) {
  await page.getByLabel("Message").fill(text);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText(`echo: ${text}`)).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
}

/** A finished thread of two jobs: "echo first" and "echo second". Returns its address. */
async function twoJobs(page: Page): Promise<string> {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await say(page, "echo second");
  // each message has been read from the log: it has its edit button
  await expect(userMessages(page)).toHaveCount(2);
  await expect(editButton(message(page, "echo second"))).toBeVisible();
  return page.url();
}

/** Opens the editor on a message, types `text` and sends it with the keyboard. */
async function editTo(page: Page, bubble: Locator, text: string) {
  await bubble.hover();
  await editButton(bubble).click();
  await expect(editor(page)).toBeFocused();
  await editor(page).fill(text);
  await editor(page).press("Control+Enter");
}

test("edit the second message: a new chat with the new answer, at 2/2, and ‹ goes back to 1/2 and the original", async ({
  page,
}) => {
  const original = await twoJobs(page);
  await expect(picker(userMessages(page).first())).toHaveCount(0);

  await editTo(page, message(page, "echo second"), "echo other");

  // the new chat: its own address, the first message and its answer as they were, then the new words
  await expect(page).not.toHaveURL(original);
  await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}#m-\d+$/);
  await expect(conversation(page).getByText("echo: echo other")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(userMessages(page)).toHaveText([/^echo first/, /^echo other/]);
  await expect(conversation(page).getByText("echo: echo first")).toBeVisible();
  await expect(conversation(page).getByText("echo second")).toHaveCount(0);
  // the message the page arrived at is where the focus is
  const edited = message(page, "echo other");
  await expect(edited).toBeFocused();
  await expect(edited).toHaveAttribute("data-seq", /^\d+$/);
  await expect(edited).toHaveAttribute("id", /^m-\d+$/);
  await expectNoHorizontalScroll(page);

  // 2/2: no way on, and the live region says it in words; the first message has no other version
  await expect(picker(edited)).toContainText("2/2");
  await expect(picker(edited).getByRole("status")).toHaveText("Version 2 of 2");
  await expect(picker(edited).getByRole("button", { name: "Next version" })).toBeDisabled();
  await expect(picker(userMessages(page).first())).toHaveCount(0);

  // the thread list shows the conversation once, and its first thread is the open chat
  await openThreadList(page);
  const root = threadList(page).locator(`a[href="/threads/${threadIdOf(original)}"]`);
  await expect(root).toHaveAttribute("aria-current", "true");
  await expect(
    threadList(page).locator(`a[href="/threads/${threadIdOf(page.url())}"]`),
  ).toHaveCount(0);
  // on a phone the list is a sheet over the page: closed, the page behind it can be used again
  const sheet = page.getByRole("dialog", { name: "Threads" });
  if (await sheet.isVisible()) {
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
  }

  // ‹ goes to the original, at its message, and shows 1/2
  await picker(edited).getByRole("button", { name: "Previous version" }).click();
  await expect(page).toHaveURL(new RegExp(`${threadIdOf(original)}#m-\\d+$`));
  const first = message(page, "echo second");
  await expect(first).toBeVisible();
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  await expect(conversation(page).getByText("echo other")).toHaveCount(0);
  await expect(picker(first)).toContainText("1/2");
  await expect(picker(first).getByRole("status")).toHaveText("Version 1 of 2");
  await expect(picker(first).getByRole("button", { name: "Previous version" })).toBeDisabled();
  await expect(first).toBeFocused();

  // and › goes on again
  await picker(first).getByRole("button", { name: "Next version" }).click();
  await expect(conversation(page).getByText("echo: echo other")).toBeVisible();
  await expect(picker(message(page, "echo other"))).toContainText("2/2");
});

test("edit the first message: a chat that holds only the new words, and the original is its other version", async ({
  page,
}) => {
  const original = await twoJobs(page);
  await editTo(page, message(page, "echo first"), "echo changed");

  await expect(page).not.toHaveURL(original);
  await expect(conversation(page).getByText("echo: echo changed")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(userMessages(page)).toHaveText([/^echo changed/]);
  await expect(conversation(page).getByText("echo second")).toHaveCount(0);
  const edited = message(page, "echo changed");
  await expect(picker(edited)).toContainText("2/2");
  await expect(edited).toBeFocused();

  // back to the original: both its messages, and the first one says 1/2
  await picker(edited).getByRole("button", { name: "Previous version" }).click();
  await expect(page).toHaveURL(new RegExp(`${threadIdOf(original)}#m-\\d+$`));
  await expect(userMessages(page)).toHaveText([/^echo first/, /^echo second/]);
  // the link named the first message, and that is where the page is
  await expect(userMessages(page).first()).toBeFocused();
  await expect(userMessages(page).first()).toHaveAttribute("id", new URL(page.url()).hash.slice(1));
  await expect(picker(userMessages(page).first())).toContainText("1/2");
  await expect(picker(userMessages(page).nth(1))).toHaveCount(0);

  // the edit goes on as a conversation of its own: the next message is its second
  await picker(userMessages(page).first()).getByRole("button", { name: "Next version" }).click();
  await expect(conversation(page).getByText("echo: echo changed")).toBeVisible();
  await say(page, "echo more");
  await expect(userMessages(page)).toHaveText([/^echo changed/, /^echo more/]);
  await expect(picker(userMessages(page).first())).toContainText("2/2");
});

test("the keyboard alone: Edit, type, Escape gives the message back, Ctrl+Enter sends, the arrows choose", async ({
  page,
}) => {
  const original = await twoJobs(page);
  const second = message(page, "echo second");

  // the button is reached by the keyboard, and shows with the focus in it
  await editButton(second).focus();
  await expect(editButton(second)).toBeFocused();
  await expect(editButton(second)).toHaveCSS("opacity", "1");
  await page.keyboard.press("Enter");
  await expect(editor(page)).toBeFocused();
  await expect(editor(page)).toHaveValue("echo second");
  await expect(page.getByText("Esc cancels · Ctrl/⌘ Enter sends")).toBeAttached();

  // Escape: nothing is sent, the words are as they were and the focus is on the button again
  await page.keyboard.type(" and more");
  await page.keyboard.press("Escape");
  await expect(editor(page)).toHaveCount(0);
  await expect(second).toContainText("echo second");
  await expect(second).not.toContainText("and more");
  await expect(editButton(second)).toBeFocused();
  expect(page.url()).toBe(original);

  // again, and a plain Enter is a new line, not a send
  await page.keyboard.press("Enter");
  await expect(editor(page)).toBeFocused();
  await editor(page).fill("echo kbd");
  await page.keyboard.press("Enter");
  await expect(editor(page)).toHaveValue("echo kbd\n");
  expect(page.url()).toBe(original);
  await editor(page).fill("echo kbd");
  await page.keyboard.press("Control+Enter");

  await expect(conversation(page).getByText("echo: echo kbd")).toBeVisible();
  const edited = message(page, "echo kbd");
  await expect(picker(edited)).toContainText("2/2");
  // the arrows are buttons: Enter on ‹ goes to the original
  await picker(edited).getByRole("button", { name: "Previous version" }).focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`${threadIdOf(original)}#m-\\d+$`));
  await expect(picker(message(page, "echo second"))).toContainText("1/2");
});

test("a message is not sent empty, and an edit that is refused says so and keeps the words", async ({
  page,
}) => {
  await twoJobs(page);
  const second = message(page, "echo second");
  await editButton(second).click();
  await editor(page).fill("   ");
  await expect(sendEdit(page)).toBeDisabled();
  await editor(page).fill("echo kept");

  await page.route("**/api/threads/*/fork", (route) =>
    route.fulfill({
      status: 503,
      contentType: "application/problem+json",
      body: JSON.stringify({ title: "Unavailable", status: 503, detail: "storage is unavailable" }),
    }),
  );
  const url = page.url();
  await editor(page).press("Control+Enter");
  await expect(
    page.getByRole("alert").filter({ hasText: "Could not edit the message" }),
  ).toContainText("storage is unavailable");
  expect(page.url()).toBe(url);
  await expect(editor(page)).toHaveValue("echo kept");
  await page.getByRole("button", { name: "Dismiss" }).click();
  await editor(page).press("Escape");
  await expect(second).toContainText("echo second");
});

for (const colorScheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${colorScheme})`, () => {
    test.use({ colorScheme });
    test(`axe (${colorScheme}): the editor, and a message with versions, have no serious violations`, async ({
      page,
    }) => {
      await twoJobs(page);
      const violations = async () => {
        // axe reads the colours as they are drawn: a turn that is fading in is not at its colours yet
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
      // the edit button, shown by the focus in it
      await editButton(message(page, "echo second")).focus();
      expect(await violations()).toEqual([]);
      // the editor
      await page.keyboard.press("Enter");
      await expect(editor(page)).toBeFocused();
      expect(await violations()).toEqual([]);
      await editor(page).fill("echo axe");
      await page.keyboard.press("Control+Enter");
      // the version picker of the new chat, and of the original
      const edited = message(page, "echo axe");
      await expect(picker(edited)).toContainText("2/2");
      await expect(edited).toBeFocused();
      expect(await violations()).toEqual([]);
      await picker(edited).getByRole("button", { name: "Previous version" }).click();
      await expect(picker(message(page, "echo second"))).toContainText("1/2");
      expect(await violations()).toEqual([]);
    });
  });
}
