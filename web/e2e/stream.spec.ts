import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, expectNoHorizontalScroll, MOCK_URL, startThread } from "./helpers";

/*
 * Live text (ADR 0027, web/DESIGN.md "A turn"), against the mock's `stream-long`, `stream-hold` and
 * `stream-abandon` scenarios (mock/scripts.ts): the agent's words are drawn while they are written,
 * as a draft after the turn's parts, and the log's message, when it comes, is the one reply. A
 * draft is not in the runtime's transcript, so none of this is a message until the log says it.
 */

const draft = (page: Page) => conversation(page).locator('[data-slot="agent-draft"]');
const reply = (page: Page) => conversation(page).locator('[data-slot="agent-message"]');

async function axeViolations(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test("the words grow while the agent writes them, then the reply stays, once", async ({ page }) => {
  await startThread(page, "stream-long write the plan");

  // a draft in the agent's turn, busy and silent for a screen reader; not a reply yet
  await expect(draft(page)).toBeVisible();
  await expect(draft(page)).toHaveAttribute("aria-busy", "true");
  await expect(draft(page)).toHaveAttribute("aria-live", "off");
  await expect(reply(page)).toHaveCount(0);
  await expect(conversation(page).getByText(/is starting/)).toHaveCount(0);
  const first = (await draft(page).innerText()).length;
  await expectNoHorizontalScroll(page);

  // it grows, and it is Markdown as it is written
  await expect
    .poll(async () => (await draft(page).innerText()).length, { timeout: 10_000 })
    .toBeGreaterThan(first);
  await expect(draft(page)).toContainText("that fixes it.");
  await expect(draft(page).getByRole("listitem").first()).toBeVisible();
  await expectNoHorizontalScroll(page);

  // the log says the reply: one message, no draft, the words once
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toContainText("when it is green.");
  await expect(reply(page).getByRole("listitem")).toHaveCount(3);
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
  await expect(conversation(page).getByText("when it is green.")).toHaveCount(1);
});

test("a reply that is still being written ends with Stop: nothing is left that looks alive", async ({
  page,
}) => {
  await startThread(page, "stream-hold write the plan");
  await expect(draft(page)).toContainText("then make the smallest change");
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
  await expect(draft(page)).toHaveCount(0);
  await expect(conversation(page).getByText("then make the smallest change")).toHaveCount(0);
});

test("a stream the model gave up goes, and the words said next are said once", async ({ page }) => {
  await startThread(page, "stream-abandon what is the answer");
  await expect(draft(page)).toContainText("The answer is forty-");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toHaveText("Sorry, let me say that again: it is forty-two.");
  await expect(conversation(page).getByText("The answer is")).toHaveCount(0);
});

test("a connection cut mid-reply: the reply arrives once, whole", async ({ page, request }) => {
  await startThread(page, "stream-long write the plan");
  await expect(draft(page)).toBeVisible();
  // the network goes away in the middle of the reply: the draft goes with the connection, and the
  // reconnect is told the text so far again (the sender's refresh) or the final message
  expect((await request.post(`${MOCK_URL}/__mock/drop-streams`)).ok()).toBe(true);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toContainText("when it is green.");
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
});

test("a page reloaded mid-reply shows the text so far, once, and the reply when it is done", async ({
  page,
}) => {
  await startThread(page, "stream-long write the plan");
  await expect(draft(page)).toContainText("that fixes it.");
  await page.reload();
  // the new connection heard none of the pieces: the sender says the text again, from the start
  await expect(draft(page)).toHaveCount(1);
  await expect(draft(page)).toContainText("I'll start with the failing test");
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
});

test("the caret blinks, unless the person asked for less motion: then it stays, still", async ({
  page,
}) => {
  await startThread(page, "stream-hold write the plan");
  await expect(draft(page)).toContainText("then make the smallest change");
  // where the caret is: the end of the last paragraph, heading or list item of the draft
  const caret = () =>
    draft(page).evaluate((root) => {
      const last = root.querySelector(".aui-md")?.lastElementChild;
      const host = last?.matches("ul, ol") ? last.lastElementChild : last;
      if (!host) return null;
      const after = getComputedStyle(host, "::after");
      return { animation: after.animationName, content: after.content, width: after.width };
    });
  expect(await caret()).toEqual({ animation: "caret-blink", content: '""', width: "2px" });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect.poll(caret).toEqual({ animation: "none", content: '""', width: "2px" });
  await page.getByRole("button", { name: "Stop" }).click();
  await expect(badge(page)).toHaveText("Stopped");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility with a reply being written (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    test("axe: a draft on the screen has no serious violations", async ({ page }) => {
      await startThread(page, "stream-hold write the plan");
      await expect(draft(page)).toContainText("then make the smallest change");
      await expect(page.getByRole("button", { name: "Stop" })).toBeVisible();
      expect(await axeViolations(page)).toEqual([]);
      await page.getByRole("button", { name: "Stop" }).click();
      await expect(badge(page)).toHaveText("Stopped");
    });
  });
}
