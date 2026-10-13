import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import {
  agentPicker,
  animationsDone,
  aPerson,
  badge,
  chooseRowAction,
  expectNoHorizontalScroll,
  forkThread,
  MOCK_URL,
  makeThread,
  openRowMenu,
  openThreadList,
  rowMenuButton,
  rowTitles,
  startThread,
  threadList,
} from "./helpers";

/*
 * Deleting a chat erases it (ADR 0043, web/DESIGN.md "Layout"): the row's menu has Delete, a dialog says what
 * is erased, the list drops the chat without a reload, and the open chat's page becomes the new chat. A chat
 * that works is stopped first: "Stop and delete" cancels, waits for the agent to report the stop, and deletes.
 * Each test is a person of its own in the mock, which answers as the orchestrator does (409 `thread_active`,
 * a second delete 404, 403 without `thread.delete`).
 */

const dialog = (page: Page) => page.getByRole("alertdialog");

async function listOpen(page: Page) {
  await page.goto("/");
  await expect(agentPicker(page)).toBeVisible();
  await openThreadList(page);
  await expect(threadList(page)).toBeVisible();
}

/** Opens a row's menu and chooses Delete: the dialog is up. */
async function askToDelete(page: Page, title: string) {
  await chooseRowAction(page, title, "Delete");
  await expect(dialog(page)).toBeVisible();
}

test("Delete asks first, says what is erased, and Cancel keeps the chat and the focus on the row's button", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const title = "echo delete cancel me";
  const id = await makeThread(person, title);
  await listOpen(page);
  await askToDelete(page, title);
  const box = dialog(page);
  await expect(box).toHaveAccessibleName(`Delete “${title}”?`);
  await expect(box).toContainText("cannot be undone");
  await expect(box).toContainText("chats made from it by editing a message");
  await expect(box).toContainText("files");
  await expect(box).toContainText("Forks of it are kept");
  await expect(box.getByRole("button", { name: "Delete", exact: true })).toBeVisible();
  await box.getByRole("button", { name: "Cancel" }).click();
  await expect(box).toBeHidden();
  await expect(rowMenuButton(page, title)).toBeFocused();
  expect((await page.request.get(`/api/threads/${id}`)).status()).toBe(200);
  await expectNoHorizontalScroll(page);
});

test("a finished chat is deleted from the list: the row goes without a reload, and the chat is erased", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const keep = "echo delete keep this";
  const gone = "echo delete drop this";
  await makeThread(person, keep);
  const id = await makeThread(person, gone);
  await listOpen(page);
  await expect.poll(() => rowTitles(page)).toEqual([gone, keep]);
  await askToDelete(page, gone);
  const request = page.waitForRequest((r) => r.method() === "DELETE" && r.url().endsWith(id));
  await dialog(page).getByRole("button", { name: "Delete", exact: true }).click();
  await request;
  await expect(dialog(page)).toBeHidden();
  await expect.poll(() => rowTitles(page)).toEqual([keep]);
  // erased: the chat is a 404 now, and the page was never reloaded
  expect((await page.request.get(`/api/threads/${id}`)).status()).toBe(404);
  await expect(page).toHaveURL(/\/$/);
  // the focus is somewhere the person can go on from
  await expect(page.locator("body")).not.toBeFocused();
});

test("deleting the chat that is open lands on the new chat, and its address is a chat that is not found", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await startThread(page, "echo delete the open one");
  await expect(badge(page)).toHaveText("Done");
  const address = page.url();
  await openThreadList(page);
  await askToDelete(page, "echo delete the open one");
  await dialog(page).getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(agentPicker(page)).toBeVisible();
  await expect(page.getByLabel("Message")).toBeVisible();
  await openThreadList(page);
  await expect(
    threadList(page).getByRole("link", { name: "echo delete the open one" }),
  ).toHaveCount(0);
  // the way back is not a page of a chat: Back does not return to it, and its address is not found
  await page.goto(address);
  await expect(page.getByText(/Thread not found/)).toBeVisible();
});

test("a chat that works is stopped first: Stop and delete cancels, waits for the stop, and erases it", async ({
  page,
}, info) => {
  await aPerson(page, info);
  await startThread(page, "slow delete while working");
  await expect(badge(page)).toHaveText("Working…");
  const address = page.url();
  const id = new URL(address).pathname.split("/").pop() as string;
  await openThreadList(page);
  await askToDelete(page, "slow delete while working");
  const box = dialog(page);
  // it says why, and offers the one thing that can be done
  await expect(box).toContainText("still working");
  await expect(box.getByRole("button", { name: "Delete", exact: true })).toHaveCount(0);
  const cancel = page.waitForRequest(
    (r) => r.method() === "POST" && r.url().endsWith(`/api/threads/${id}/cancel`),
  );
  const erase = page.waitForRequest((r) => r.method() === "DELETE" && r.url().endsWith(id));
  await box.getByRole("button", { name: "Stop and delete" }).click();
  await cancel;
  await erase;
  await expect(page).toHaveURL(/\/$/);
  await expect(dialog(page)).toBeHidden();
  expect((await page.request.get(`/api/threads/${id}`)).status()).toBe(404);
});

test("a chat that started working since the list was read is told so by the server, and is stopped then", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const title = "echo delete late work";
  const quiet = await makeThread(person, title);
  await listOpen(page);
  // the list says done; then the chat is asked something that takes a while
  const run = await fetch(`${MOCK_URL}/agui/agents/adam`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "text/event-stream",
      Cookie: person.cookie,
    },
    body: JSON.stringify({
      threadId: quiet,
      runId: "run-2",
      messages: [
        { id: "m-1", role: "user", content: title },
        { id: "m-2", role: "user", content: "slow now" },
      ],
    }),
  });
  const reader = run.body?.getReader();
  let seen = "";
  const decoder = new TextDecoder();
  while (reader && !seen.includes("RUN_STARTED")) {
    const { value, done } = await reader.read();
    if (done) break;
    seen += decoder.decode(value);
  }
  await reader?.cancel();
  await askToDelete(page, title);
  const box = dialog(page);
  await box.getByRole("button", { name: "Delete", exact: true }).click();
  // the server said 409 thread_active: no error, the dialog now offers to stop it
  await expect(box.getByRole("button", { name: "Stop and delete" })).toBeVisible();
  await expect(box).toContainText("still working");
  await expect(page.getByRole("alert").filter({ hasText: /Could not/ })).toHaveCount(0);
  await box.getByRole("button", { name: "Stop and delete" }).click();
  await expect(box).toBeHidden();
  await expect.poll(() => rowTitles(page)).toEqual([]);
  expect((await page.request.get(`/api/threads/${quiet}`)).status()).toBe(404);
});

test("a fork is kept when its parent is deleted: it takes the parent's place in the list", async ({
  page,
}, info) => {
  const person = await aPerson(page, info);
  const parentTitle = "echo delete parent of a fork";
  const forkTitle = "echo delete the fork that stays";
  const parent = await makeThread(person, parentTitle);
  const fork = await forkThread(person, parent, forkTitle);
  await listOpen(page);
  await expect.poll(() => rowTitles(page)).toEqual([parentTitle, forkTitle]);
  await askToDelete(page, parentTitle);
  await dialog(page).getByRole("button", { name: "Delete", exact: true }).click();
  await expect.poll(() => rowTitles(page)).toEqual([forkTitle]);
  // no longer nested: a row of the list's own level, with the whole menu of one
  await expect(threadList(page).locator('[data-slot="thread-children"]')).toHaveCount(0);
  const menu = await openRowMenu(page, forkTitle);
  await expect(menu.getByRole("menuitem", { name: "Pin", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  expect((await page.request.get(`/api/threads/${fork}`)).status()).toBe(200);
  expect((await page.request.get(`/api/threads/${parent}`)).status()).toBe(404);
});

test("a person whose roles do not hold thread.delete is not offered it", async ({ page }, info) => {
  // a role that reads and does not write; the chat is handed to them, as the mock allows
  const maker = await aPerson(page, info);
  const title = "echo delete not for viewers";
  const id = await makeThread(maker, title);
  const viewer = await aPerson(page, info, { me: "read-only", label: "viewer" });
  const handed = await fetch(`${MOCK_URL}/__mock/owner?thread=${id}&owner=${viewer.user}`, {
    method: "POST",
  });
  expect(handed.status).toBe(204);
  // (a role that cannot invoke an agent has no agent to wait for: the list is the page's)
  await page.goto("/");
  await openThreadList(page);
  await expect(rowMenuButton(page, title)).toBeVisible();
  const menu = await openRowMenu(page, title);
  await expect(menu.getByRole("menuitem")).toHaveText(["Pin", "Archive"]);
  await page.keyboard.press("Escape");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: Page) {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: the delete dialog, for a chat that is idle and for one that works, has no serious violations", async ({
      page,
    }, info) => {
      const person = await aPerson(page, info);
      await makeThread(person, "echo delete axe idle");
      await makeThread(person, "slow delete axe working");
      await listOpen(page);
      await askToDelete(page, "echo delete axe idle");
      expect(await serious(page)).toEqual([]);
      await dialog(page).getByRole("button", { name: "Cancel" }).click();
      await expect(dialog(page)).toBeHidden();
      await askToDelete(page, "slow delete axe working");
      await expect(dialog(page)).toContainText("still working");
      expect(await serious(page)).toEqual([]);
    });
  });
}
