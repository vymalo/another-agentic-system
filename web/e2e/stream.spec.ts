import AxeBuilder from "@axe-core/playwright";
import { type APIRequestContext, expect, type Page, test } from "@playwright/test";
import {
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  startThread,
} from "./helpers";

/*
 * Live text (ADR 0027, web/DESIGN.md "A turn"), against the mock's `stream-gate`, `stream-hold` and
 * `stream-abandon` scenarios (mock/scripts.ts): the agent's words are drawn while they are written,
 * as a draft after the turn's parts, and the log's message, when it comes, is the one reply. A
 * draft is not in the runtime's transcript, so none of this is a message until the log says it.
 *
 * A draft is on the screen only as long as the script has left, so a test that has to look at one
 * after a page load, a reload or a cut must not race a script that plays on a timer (that is what
 * made these tests fail on the phone project: the reply was finished by the time the page was
 * back). Those tests run a scenario that holds the reply until the test releases
 * it (`stream-gate`, `stream-abandon`: `POST /__mock/release`), so the state they look at stays as
 * long as they need, on any machine. The words growing piece by piece is covered by the app's
 * DOM test (`chat-shell-live.dom.test.tsx`, `stream-long`).
 */

const draft = (page: Page) => conversation(page).locator('[data-slot="agent-draft"]');
const reply = (page: Page) => conversation(page).locator('[data-slot="agent-message"]');

/** The thread the page is on (its id is the last part of the URL). */
const threadOf = (page: Page) => new URL(page.url()).pathname.split("/").pop() ?? "";

/**
 * Lets a held run go on. Call it once the draft shows the last piece before the hold: the mock holds
 * with that piece, so a release after it is never early (a 409 says the run does not wait).
 */
async function release(page: Page, request: APIRequestContext) {
  const res = await request.post(`${MOCK_URL}/__mock/release?thread=${threadOf(page)}`);
  expect(res.status(), "the release of a held run").toBe(204);
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test("a draft is Markdown as it is written; the log's reply replaces it, once", async ({
  page,
  request,
}) => {
  await startThread(page, "stream-gate write the plan");

  // five pieces in, held: a draft in the agent's turn, busy and silent for a screen reader; not a reply yet
  await expect(draft(page)).toContainText("fix the off-by-one in the loop");
  await expect(draft(page)).toHaveAttribute("aria-busy", "true");
  await expect(draft(page)).toHaveAttribute("aria-live", "off");
  await expect(reply(page)).toHaveCount(0);
  await expect(conversation(page).getByText(/is starting/)).toHaveCount(0);
  await expect(draft(page)).toContainText("that fixes it.");
  await expect(draft(page).getByRole("listitem").first()).toBeVisible();
  await expect(draft(page).getByRole("listitem")).toHaveCount(2);
  await expectNoHorizontalScroll(page);

  // the rest comes, and the log says the reply: one message, no draft, the words once
  await release(page, request);
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

test("a stream the model gave up goes, and the words said next are said once", async ({
  page,
  request,
}) => {
  await startThread(page, "stream-abandon what is the answer");
  await expect(draft(page)).toContainText("The answer is forty-");
  // the model stalls here until the test lets it fail
  await release(page, request);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toHaveText("Sorry, let me say that again: it is forty-two.");
  await expect(conversation(page).getByText("The answer is")).toHaveCount(0);
});

test("a connection cut mid-reply: the reply arrives once, whole", async ({ page, request }) => {
  await startThread(page, "stream-gate write the plan");
  await expect(draft(page)).toContainText("fix the off-by-one in the loop");
  // the network goes away in the middle of the reply (this thread's streams only: the tests of this
  // file run side by side against one mock): the reconnect heard none of the pieces, and is told the
  // text so far again by the sender's refresh
  const cut = await request.post(`${MOCK_URL}/__mock/drop-streams?thread=${threadOf(page)}`);
  expect(cut.status()).toBe(204);
  await expect(draft(page)).toHaveCount(1);
  await expect(draft(page)).toContainText("fix the off-by-one in the loop");
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
  await release(page, request);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(draft(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toContainText("when it is green.");
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
});

test("a page reloaded mid-reply shows the text so far, once, and the reply when it is done", async ({
  page,
  request,
}) => {
  await startThread(page, "stream-gate write the plan");
  await expect(draft(page)).toContainText("fix the off-by-one in the loop");
  await page.reload();
  // the new connection heard none of the pieces: the sender says the text again, from the start
  await expect(draft(page)).toHaveCount(1);
  await expect(draft(page)).toContainText("fix the off-by-one in the loop");
  await expect(draft(page)).toContainText("I'll start with the failing test");
  await expect(conversation(page).getByText("I'll start with the failing test")).toHaveCount(1);
  await release(page, request);
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
