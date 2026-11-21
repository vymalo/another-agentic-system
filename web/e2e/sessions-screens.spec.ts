import { expect, type Page, test } from "@playwright/test";
import { uuidv7 } from "../src/lib/uuid";
import {
  animationsDone,
  aPerson,
  badge,
  conversation,
  hideActivity,
  MOCK_URL,
  openThreadList,
  panel,
  panelTab,
  panelToggle,
  startThread,
  THREAD_URL,
} from "./helpers";

/*
 * `pnpm screens e2e/sessions-screens.spec.ts`: the screens of subagents (child-sessions/v1, web/README.md
 * "Subagents"), for both color schemes on a desktop and a phone, in e2e/__screens__/. Only this file is
 * run for them, so the other screens are left as they are. Each state is a person of its own in the mock.
 */

const DIR = "e2e/__screens__";

async function resetMock() {
  await fetch(`${MOCK_URL}/__mock/reset`, { method: "POST" });
}

async function shot(page: Page, name: string) {
  const device = test.info().project.name;
  const dark = await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches);
  await page.mouse.move(0, 0);
  await animationsDone(page);
  await page.waitForTimeout(250);
  expect(await page.evaluate(() => document.scrollingElement?.scrollTop ?? 0)).toBe(0);
  await page.screenshot({
    path: `${DIR}/${device}-${dark ? "dark" : "light"}-${name}.png`,
    animations: "disabled",
    caret: "hide",
    scale: "css",
  });
}

const dialog = (page: Page, name: string) => page.getByRole("dialog", { name });
const bannerOf = (page: Page) => page.locator('[data-slot="child-banner"]');
const childState = (page: Page) => bannerOf(page).locator('[data-slot="child-state"]');

async function showSubagents(page: Page) {
  if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
    await panelToggle(page).click();
  }
  await expect(panel(page)).toBeVisible();
  await panelTab(page, "Subagents").click();
  return panel(page).getByRole("tabpanel", { name: /^Subagents/ });
}

/** Starts a subagent of the open thread through the API; its start line comes to the chat. */
async function start(page: Page, description: string, text: string, keepOpen: boolean) {
  const parent = page.url().split("/").pop() ?? "";
  const res = await page.request.post(`/api/threads/${parent}/sessions`, {
    data: { id: uuidv7(), agent: "planner", description, text, keepOpen },
  });
  expect(res.status()).toBe(202);
}

/** A parent that has a subagent of its own and the page of that subagent; `hold` keeps its round working. */
async function inChild(page: Page, keepOpen: boolean, text: string) {
  await startThread(page, "hello");
  await expect(badge(page)).toContainText("Done");
  await start(page, "Plan the docs", text, keepOpen);
  await hideActivity(page);
  await conversation(page)
    .locator('[data-slot="start-line"]')
    .getByRole("link", { name: /^Open/ })
    .click();
  await expect(bannerOf(page)).toBeVisible();
  return page.url().split("/").pop() ?? "";
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(scheme, () => {
    test.use({ colorScheme: scheme });
    test.beforeEach(resetMock);

    test("the Subagents tab and the cards", async ({ page }, info) => {
      await aPerson(page, info, { user: "ada@example.com" });
      await startThread(page, "helpers");
      await expect(conversation(page).locator('[data-slot="session-card"]')).toHaveCount(2);
      await start(page, "Check the schema", "hold panel", true);
      await expect(conversation(page).locator('[data-slot="start-line"]')).toHaveCount(1);
      const tab = await showSubagents(page);
      await expect(tab.locator('[data-slot="subagent-state"]').first()).toBeVisible();
      await expect(tab).toContainText("Check the schema");
      // the script's own late events have come: its two helpers are closed
      await expect(tab.getByText("2 closed")).toBeVisible();
      await expect(tab.locator('[data-slot="subagent-state"]', { hasText: "Working" })).toHaveCount(
        1,
      );
      await shot(page, "sessions-panel");
    });

    test("the New subagent dialog", async ({ page }, info) => {
      await aPerson(page, info, { user: "ada@example.com" });
      await startThread(page, "hello");
      await expect(badge(page)).toContainText("Done");
      const tab = await showSubagents(page);
      await tab.getByRole("button", { name: "New subagent" }).click();
      const form = dialog(page, "New subagent");
      await form.getByLabel("What it is for").fill("Plan the docs");
      await form.getByLabel("What to ask it").fill("Read the README and list what is out of date.");
      await expect(form.getByRole("checkbox", { name: /Close when done/ })).toBeChecked();
      await shot(page, "sessions-new");
    });

    test("a start that did not start", async ({ page }, info) => {
      const person = await aPerson(page, info, { user: "ada@example.com" });
      const session = person.cookie.split("=")[1] ?? "";
      const set = await page.request.post(
        `${MOCK_URL}/__mock/config?sessionStarts=refuse&session=${session}`,
      );
      expect(set.ok()).toBe(true);
      await startThread(page, "hello");
      await expect(badge(page)).toContainText("Done");
      const tab = await showSubagents(page);
      await tab.getByRole("button", { name: "New subagent" }).click();
      const form = dialog(page, "New subagent");
      await form.getByLabel("What it is for").fill("Plan the docs");
      await form.getByLabel("What to ask it").fill("Read the README.");
      await form.getByRole("button", { name: "Start subagent" }).click();
      await expect(form).toBeHidden();
      await hideActivity(page);
      const line = conversation(page).locator('[data-slot="start-line"]');
      await expect(line).toContainText("Did not start");
      await expect(line.getByRole("button", { name: "Try again" })).toBeVisible();
      await shot(page, "sessions-start-failed");
    });

    test("a subagent's own thread, working and closed, and the Close dialog", async ({
      page,
    }, info) => {
      await aPerson(page, info, { user: "ada@example.com" });
      const child = await inChild(page, true, "hold screens");
      await expect(childState(page)).toHaveText("Working");
      await expect(bannerOf(page)).toContainText("Stays open");
      await shot(page, "sessions-child");

      await bannerOf(page).getByRole("button", { name: "Close planner" }).click();
      await expect(dialog(page, "Close subagent")).toBeVisible();
      await shot(page, "sessions-close");
      await page.keyboard.press("Escape");
      await expect(dialog(page, "Close subagent")).toBeHidden();

      await fetch(`${MOCK_URL}/__mock/release?thread=${child}`, { method: "POST" });
      await expect(childState(page)).toHaveText("Idle");
      await bannerOf(page).getByRole("button", { name: "Close planner" }).click();
      await dialog(page, "Close subagent").getByRole("button", { name: "Close subagent" }).click();
      await expect(page.locator('[data-slot="session-closed-notice"]')).toBeVisible();
      await expect(childState(page)).toHaveText("Closed");
      await shot(page, "sessions-child-closed");
    });

    test("the rail with a subagent under its parent", async ({ page }, info) => {
      await aPerson(page, info, { user: "ada@example.com" });
      await startThread(page, "helpers");
      await expect(conversation(page).locator('[data-slot="session-card"]')).toHaveCount(2);
      await start(page, "Check the schema", "hold rail", true);
      await expect(page).toHaveURL(THREAD_URL);
      await hideActivity(page);
      await openThreadList(page);
      await expect(
        page.getByText("Check the schema").filter({ visible: true }).first(),
      ).toBeVisible();
      await shot(page, "sessions-rail");
    });
  });
}
