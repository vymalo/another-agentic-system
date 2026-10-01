import { expect, type Page, test } from "@playwright/test";
import {
  agentMenuItem,
  agentPicker,
  badge,
  conversation,
  expectNoHorizontalScroll,
  openAgentMenu,
  openThreadList,
  startThread,
  THREAD_URL,
  threadList,
} from "./helpers";

/*
 * Forking a chat (ADR 0029, web/DESIGN.md "Fork and branch"): "Fork from here" under a finished
 * agent turn makes a new chat that holds the conversation up to the end of that turn; another agent
 * in the agent menu continues the conversation in a fork. The mock server plays the contract of
 * `forkThread`; nothing here waits for a timer: a turn that must still be going on is one the mock
 * holds until the test stops it (`Refactor`), and the rest waits for what the page shows.
 */

const forkButton = (page: Page) =>
  conversation(page).getByRole("button", { name: "Fork from here" });

/** What the person said, bubble by bubble. */
const userMessages = (page: Page) => conversation(page).locator('[data-slot="user-message"]');

/** The divider a fork opens with: "Forked from <title>". */
const divider = (page: Page) => conversation(page).locator('[data-slot="fork-divider"]');

const threadIdOf = (url: string) => new URL(url).pathname.split("/").pop() as string;

/** A thread's resource, as the mock says it. */
async function resource(page: Page, id: string) {
  const res = await page.request.get(`/api/threads/${id}`);
  expect(res.status()).toBe(200);
  return (await res.json()) as {
    lastSeq: number;
    state: string;
    target: { agentId: string };
    forkedFrom?: { threadId?: string; seq: number; kind: string };
  };
}

test("fork from here: a new chat with the conversation to the end of that turn, and the parent untouched", async ({
  page,
}) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  const parentUrl = page.url();
  const parent = threadIdOf(parentUrl);
  const before = await resource(page, parent);

  await expect(forkButton(page)).toBeEnabled();
  await forkButton(page).click();

  // the new chat: its own address, the conversation as it was, then the divider, and it is finished
  await expect(page).not.toHaveURL(parentUrl);
  await expect(page).toHaveURL(THREAD_URL);
  await expect(divider(page)).toContainText("Forked from echo first");
  await expect(userMessages(page)).toHaveText([/^echo first/]);
  await expect(conversation(page).getByText("echo: echo first")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(divider(page).getByRole("link", { name: "echo first" })).toHaveAttribute(
    "href",
    `/threads/${parent}`,
  );
  await expectNoHorizontalScroll(page);

  const fork = await resource(page, threadIdOf(page.url()));
  expect(fork.forkedFrom).toEqual({ threadId: parent, seq: before.lastSeq, kind: "fork" });
  expect(fork.target).toEqual(before.target);

  // the conversation goes on in the fork, as the next job
  await page.getByLabel("Message").fill("echo and now more");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("echo: echo and now more")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  // the parent is as it was
  expect((await resource(page, parent)).lastSeq).toBe(before.lastSeq);

  // the fork is a row of the thread list, marked, and the parent's row is there too (the mock is
  // shared by the tests of a run: the rows are found by where they lead)
  await openThreadList(page);
  const forkRow = threadList(page).locator(`a[href="/threads/${threadIdOf(page.url())}"]`);
  await expect(forkRow).toHaveAttribute("aria-current", "page");
  await expect(forkRow).toContainText("(fork)");
  const parentRow = threadList(page).locator(`a[href="/threads/${parent}"]`);
  await expect(parentRow).toBeVisible();
  await expect(parentRow).not.toContainText("(fork)");

  // the divider leads back to the parent
  await page.reload();
  await divider(page).getByRole("link", { name: "echo first" }).click();
  await expect(page).toHaveURL(parentUrl);
  await expect(conversation(page).getByText("echo: echo first")).toBeVisible();
  await expect(divider(page)).toHaveCount(0);
});

test("an earlier turn forks to its own end, the later turns are left out", async ({ page }) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await page.getByLabel("Message").fill("echo second");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("echo: echo second")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
  await expect(forkButton(page)).toHaveCount(2);

  await forkButton(page).first().click();
  await expect(divider(page)).toContainText("Forked from");
  await expect(conversation(page).getByText("echo: echo first")).toBeVisible();
  await expect(userMessages(page)).toHaveText([/^echo first/]);
  await expect(badge(page)).toHaveText("Done");
});

test("while the turn is going on it cannot be forked, and the button says why; once it is over it can", async ({
  page,
}) => {
  await startThread(page, "Refactor the module");
  await expect(badge(page)).toHaveText("Working…");
  await expect(forkButton(page)).toBeDisabled();
  await forkButton(page).hover();
  await expect(page.getByRole("tooltip")).toContainText("once the agent has finished this turn");
  // a click does nothing: no request is made
  let requested = false;
  page.on("request", (r) => {
    if (r.url().endsWith("/fork")) requested = true;
  });
  await forkButton(page).click({ force: true });
  expect(requested).toBe(false);
  await expect(page).toHaveURL(THREAD_URL);

  // the other agents are not offered either, and the menu says why
  const menu = await openAgentMenu(page);
  await expect(agentMenuItem(page, "Reviewer")).toBeDisabled();
  await expect(menu).toContainText("The agent is working");
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(forkButton(page)).toBeEnabled();
});

test("the keyboard reaches Fork from here, and the new chat is where the focus is not lost", async ({
  page,
}) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await forkButton(page).focus();
  await expect(forkButton(page)).toBeFocused();
  // the actions show with the focus in them, not only on hover
  await expect(forkButton(page).locator("xpath=..")).toHaveCSS("opacity", "1");
  await page.keyboard.press("Enter");
  await expect(divider(page)).toContainText("Forked from echo first");
});

test("another agent in the menu continues the conversation in a new chat, after a question", async ({
  page,
}) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  const parentUrl = page.url();
  await expect(agentPicker(page)).toContainText("Coder");

  // Cancel: nothing is made, the chat is as it was, the focus is back on the picker
  await openAgentMenu(page);
  await agentMenuItem(page, "Reviewer").click();
  const dialog = page.getByRole("alertdialog", { name: "Continue with Reviewer in a new chat?" });
  await expect(dialog).toContainText(
    "The conversation so far is copied; this chat stays as it is.",
  );
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  await expect(agentPicker(page)).toBeFocused();
  expect(page.url()).toBe(parentUrl);

  // Continue: a new chat with the reviewer, the conversation copied
  await openAgentMenu(page);
  await agentMenuItem(page, "Reviewer").click();
  await dialog.getByRole("button", { name: "Continue in a new chat" }).click();
  await expect(page).not.toHaveURL(parentUrl);
  await expect(page).toHaveURL(THREAD_URL);
  await expect(agentPicker(page)).toHaveText("Agent: Reviewer");
  await expect(divider(page)).toContainText("Forked from echo first");
  await expect(divider(page)).toContainText("continued with reviewer");
  await expect(conversation(page).getByText("echo: echo first")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");

  // the reviewer takes the next message
  const fork = await resource(page, threadIdOf(page.url()));
  expect(fork.target.agentId).toBe("reviewer");
  await page.getByLabel("Message").fill("echo and over to you");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("echo: echo and over to you")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");

  // the first chat still talks to the coder
  await page.goto(parentUrl);
  await expect(agentPicker(page)).toContainText("Coder");
  await expect(divider(page)).toHaveCount(0);
});

test("another release of the thread's agent is a fork with that release", async ({ page }) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await openAgentMenu(page);
  await agentMenuItem(page, "staging").click();
  const dialog = page.getByRole("alertdialog", { name: "Continue with Coder in a new chat?" });
  await dialog.getByRole("button", { name: "Continue in a new chat" }).click();
  await expect(divider(page)).toContainText("Forked from echo first");
  await expect(agentPicker(page)).toHaveText("Agent: Coder · staging");
  const fork = await resource(page, threadIdOf(page.url()));
  expect(fork.target).toEqual({ agentId: "coder", release: "staging" });
});

test("a thread that waits for an answer can be forked, and the fork takes an ordinary message", async ({
  page,
}) => {
  await startThread(page, "ask which branch");
  await expect(badge(page)).toHaveText("Your turn");
  await forkButton(page).click();
  await expect(divider(page)).toContainText("Forked from");
  // the question belongs to the copy; the fork is a finished job
  await expect(badge(page)).toHaveText("Done");
  await page.getByLabel("Message").fill("echo carry on without it");
  await page.getByRole("button", { name: "Send" }).click();
  await expect(conversation(page).getByText("echo: echo carry on without it")).toBeVisible();
  await expect(badge(page)).toHaveText("Done");
});

test("a refusal because the turn is not over is shown as a message, and the chat stays", async ({
  page,
}) => {
  await startThread(page, "echo first");
  await expect(badge(page)).toHaveText("Done");
  await page.route("**/api/threads/*/fork", (route) =>
    route.fulfill({
      status: 409,
      contentType: "application/problem+json",
      body: JSON.stringify({
        title: "Conflict",
        status: 409,
        detail: "the turn is still going on",
        code: "turn_open",
      }),
    }),
  );
  const url = page.url();
  await forkButton(page).click();
  await expect(
    page.getByRole("alert").filter({ hasText: "still working on this turn" }),
  ).toBeVisible();
  expect(page.url()).toBe(url);
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(page.getByText("still working on this turn")).toHaveCount(0);
});
