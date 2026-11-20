import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, startThread } from "./helpers";

/**
 * A plan (docs/api/ui-catalog-v1.md, "Version 8", "A surface sent again" and "The plan") against the
 * mock orchestrator's `plan` story: an agent whose plan is ONE surface that it sends again, whole and
 * under the same id, each time the plan changes: five copies over two runs. The page holds one drawing per
 * stretch between two messages of the person, the earlier ones as the person saw them; a dropped step is said
 * in words, struck through and not counted; its note is shown; the newest plan is pinned in the panel; and the
 * change is said politely, once, never for the words the agent streams.
 */

declare global {
  interface Window {
    __planSaid?: string[];
  }
}

/** Every announcement of the plan's status region: each is a node added to it. Installed before the page runs. */
async function listenToTheStatusRegion(page: Page) {
  await page.addInitScript(() => {
    window.__planSaid = [];
    const watch = () =>
      new MutationObserver((records) => {
        for (const r of records) {
          for (const node of r.addedNodes) {
            const el = node instanceof Element ? node : null;
            const parent = el?.parentElement;
            if (el && parent?.getAttribute("data-slot") === "plan-status") {
              window.__planSaid?.push(el.textContent ?? "");
            }
          }
        }
      }).observe(document, { childList: true, subtree: true });
    watch();
  });
}

/** A run of the plan is eleven steps, 400 ms apart: longer than an expectation waits by default. */
const DONE = 20_000;

const said = (page: Page) => page.evaluate(() => window.__planSaid ?? []);
const checklists = (page: Page) => conversation(page).locator('[data-slot="checklist"]');
/** Every drawing of the plan in the conversation, and the newest one, which is the live one. */
const plans = (page: Page) =>
  conversation(page).getByRole("region", { name: "Plan from reviewer" });
const plan = (page: Page) => plans(page).last();

async function say(page: Page, text: string) {
  await page.getByLabel("Message", { exact: true }).fill(text);
  await page.getByRole("button", { name: "Send" }).click();
}

test.describe("a plan: one drawing per stretch, updated where it stands", () => {
  test("is two drawings after five updates over two runs and a message of the person, the dropped step in words and not counted", async ({
    page,
  }) => {
    await startThread(page, "plan now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    // run 1 sent the plan three times: one checklist, the last copy, with its note
    await expect(checklists(page)).toHaveCount(1);
    await expect(plan(page)).toContainText("1 of 4 done");
    await expect(plan(page).getByText("The second branch of parse_group")).toBeVisible();
    await expect(plan(page).getByRole("listitem")).toHaveCount(4);

    await say(page, "carry on");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    // the person wrote in between: run 2's copies are a new drawing, and the first stays as the person saw it
    await expect(plans(page)).toHaveCount(2);
    await expect(checklists(page)).toHaveCount(2);
    const frozen = plans(page).first();
    await expect(frozen).toContainText("1 of 4 done");
    await expect(frozen).toContainText("This plan was updated further down.");
    await expect(frozen.getByRole("listitem")).toHaveCount(4);
    await expect(plan(page)).not.toContainText("updated further down");
    await expect(plan(page).getByRole("listitem")).toHaveCount(6);
    await expect(
      conversation(page).getByText("This interface was updated further down."),
    ).toHaveCount(0);
    // five of six steps are in the plan: the dropped one is in neither number
    await expect(plan(page)).toContainText("2 of 5 done");
    await expect(plan(page).getByRole("progressbar", { name: "Progress" })).toHaveAttribute(
      "aria-valuetext",
      "2 of 5 done",
    );
    // the dropped step is said in words, struck through, and its note is shown
    const dropped = plan(page).locator('li[data-state="dropped"]');
    await expect(dropped).toHaveCount(1);
    await expect(dropped).toContainText("Port it to the CLI");
    await expect(dropped).toContainText("Dropped");
    await expect(dropped).toContainText("Out of scope: the CLI does not parse");
    await expect(dropped.getByText("Port it to the CLI")).toHaveCSS(
      "text-decoration-line",
      "line-through",
    );
    // the steps that were added are at the end, to do
    await expect(plan(page).locator('li[data-state="todo"]')).toHaveCount(2);
    await expectNoHorizontalScroll(page);
  });

  test("is said politely, once for a burst, not for a plan just met, and not for the words the agent streams", async ({
    page,
  }) => {
    await listenToTheStatusRegion(page);
    await startThread(page, "plan now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const region = plan(page).locator('[data-slot="plan-status"]');
    await expect(region).toHaveAttribute("role", "status");
    await expect(region).toHaveAttribute("aria-live", "polite");
    // the page drew the plan of run 1 as it stood when the thread opened: nothing to announce about a plan the person has just met
    await expect(plan(page)).toContainText("1 of 4 done");
    await page.waitForTimeout(1500);
    expect(await said(page)).toEqual([]);

    await say(page, "carry on");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    // run 2 sent the plan twice, 400 ms apart, and then a reply that streamed in pieces: one announcement, of the last state
    await expect.poll(() => said(page)).toEqual(["Plan updated: 2 of 5 done"]);
    await expect(conversation(page)).toContainText("The plan is now five steps, one dropped.");
    // and it stays one: what streamed did not touch the region
    await page.waitForTimeout(1500);
    expect(await said(page)).toEqual(["Plan updated: 2 of 5 done"]);
  });

  test("is pinned in the panel: the Plan tab shows the latest copy and its progress, and follows the updates", async ({
    page,
  }) => {
    await startThread(page, "plan now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(plan(page)).toContainText("1 of 4 done");

    const toggle = page.getByRole("button", { name: "Thread details" });
    const opened = async () => {
      if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
    };
    await opened();
    const tab = page.getByRole("tab", { name: /^Plan/ });
    await expect(tab).toBeVisible();
    await expect(tab).toContainText("1/4");
    await tab.click();
    const pinned = page.getByRole("tabpanel");
    await expect(pinned.locator('[data-slot="checklist"]')).toHaveCount(1);
    await expect(pinned).toContainText("Fix the parser");
    await expect(pinned).toContainText("1 of 4 done");

    // the person talks on: a sheet (a phone) is closed to write, a docked panel stays
    const sheet = page.getByRole("dialog");
    if (await sheet.count()) {
      await page.keyboard.press("Escape");
      await expect(sheet).toHaveCount(0);
    }
    await say(page, "carry on");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    await opened();
    await expect(page.getByRole("tab", { name: /^Plan/ })).toContainText("2/5");
    const after = page.getByRole("tabpanel");
    await expect(after).toContainText("2 of 5 done");
    await expect(after.locator('li[data-state="dropped"]')).toContainText("Dropped");
    await expect(after.locator('li[data-state="dropped"]')).toContainText(
      "Out of scope: the CLI does not parse",
    );
    await expectNoHorizontalScroll(page);
  });

  test("a plan sent again in a new job is a new drawing; the earlier ones stay as they were, only the newest is live", async ({
    page,
  }) => {
    await startThread(page, "plan now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await say(page, "carry on");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    await expect(plans(page)).toHaveCount(2);
    // a third job: the person asks for the plan again, and the orchestrator starts the activity again under a new key
    await say(page, "plan again");
    await expect(badge(page)).toHaveText("Your turn", { timeout: DONE });
    await expect(plans(page)).toHaveCount(3);
    await expect(plans(page).nth(0)).toContainText("1 of 4 done");
    await expect(plans(page).nth(0)).toContainText("This plan was updated further down.");
    await expect(plans(page).nth(1)).toContainText("2 of 5 done");
    await expect(plans(page).nth(1)).toContainText("This plan was updated further down.");
    await expect(plans(page).nth(2)).toContainText("1 of 4 done");
    await expect(plans(page).nth(2)).not.toContainText("updated further down");
    // only the live drawing holds the announcer
    await expect(conversation(page).locator('[data-slot="plan-status"]')).toHaveCount(1);
    await expect(plans(page).nth(2).locator('[data-slot="plan-status"]')).toHaveCount(1);
    await expectNoHorizontalScroll(page);
  });

  test("the words the person answered stay above their message, also after a reload; the rewritten plan is a drawing of its own", async ({
    page,
  }) => {
    await startThread(page, "plan-rewrite now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    await expect(plan(page)).toContainText("ORIGINAL: delete the cache");
    await say(page, "ok go ahead");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    const check = async () => {
      await expect(plans(page)).toHaveCount(2);
      await expect(plans(page).first()).toContainText("ORIGINAL: delete the cache");
      await expect(plans(page).first()).not.toContainText("REWRITTEN");
      await expect(plans(page).last()).toContainText("REWRITTEN: delete the database");
      // the first drawing is above the person's message: they approved those words
      const above = await page.evaluate(() => {
        const log = document.querySelector('[role="log"]') as HTMLElement;
        const first = log.querySelector('section[data-slot="plan"]') as HTMLElement;
        const words = [...log.querySelectorAll("*")].find(
          (el) => el.children.length === 0 && el.textContent === "ok go ahead",
        ) as HTMLElement;
        return Boolean(first.compareDocumentPosition(words) & Node.DOCUMENT_POSITION_FOLLOWING);
      });
      expect(above).toBe(true);
    };
    await check();
    await page.reload();
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    await check();
  });

  test("a hundred copies in one job are one drawing and at most one announcement", async ({
    page,
  }) => {
    await listenToTheStatusRegion(page);
    await startThread(page, "plan-burst now", "Reviewer");
    await expect(badge(page)).toHaveText("Done", { timeout: DONE });
    await expect(plans(page)).toHaveCount(1);
    await expect(checklists(page)).toHaveCount(1);
    await expect(plan(page)).toContainText("19 of 20 done");
    await expect(plan(page)).toContainText("copy 100");
    await page.waitForTimeout(1500);
    expect((await said(page)).length).toBeLessThanOrEqual(1);
  });

  test("a thread that has no plan has no Plan tab", async ({ page }) => {
    await startThread(page, "choices-v8 now", "Reviewer");
    await expect(badge(page)).toHaveText("Your turn");
    const toggle = page.getByRole("button", { name: "Thread details" });
    if ((await toggle.getAttribute("aria-expanded")) !== "true") await toggle.click();
    await expect(page.getByRole("tab", { name: "Activity" })).toBeVisible();
    await expect(page.getByRole("tab", { name: /^Plan/ })).toHaveCount(0);
  });
});
