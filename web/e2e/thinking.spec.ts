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
 * What the agent's model thought (ADR 0044, docs/api/agui.md "Reasoning"), against the mock's `think`,
 * `think-gate` and `think-cut` scenarios (mock/scripts.ts): a "Thinking" block above the answer, closed
 * until the person opens it, growing while it is open and the model is still writing it, and whole after a
 * reload from the log. The reasoning is not the reply: it is never in the answer's words.
 *
 * `think-gate` holds the model while it is still thinking until the test releases it
 * (`POST /__mock/release`), so the state the test looks at stays as long as it needs.
 */

const thinking = (page: Page) => conversation(page).locator('[data-slot="thinking"]');
const thinkingText = (page: Page) => conversation(page).locator('[data-slot="thinking-text"]');
const reply = (page: Page) => conversation(page).locator('[data-slot="agent-message"]');
const toggle = (page: Page) => thinking(page).getByRole("button", { name: "Thinking" });

const threadOf = (page: Page) => new URL(page.url()).pathname.split("/").pop() ?? "";

async function release(page: Page, request: APIRequestContext) {
  const res = await request.post(`${MOCK_URL}/__mock/release?thread=${threadOf(page)}`);
  expect(res.status(), "the release of a held run").toBe(204);
}

async function axeViolations(page: Page) {
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
}

test("the block is closed, grows while it is open, and stays open when the log's text comes", async ({
  page,
  request,
}) => {
  await startThread(page, "think-gate write fibonacci");

  // the model is still thinking: a block, closed, marked as being written; no text on the screen yet
  await expect(thinking(page)).toHaveCount(1);
  await expect(thinking(page)).toHaveAttribute("data-state", "closed");
  await expect(thinking(page)).toHaveAttribute("data-streaming", "true");
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "false");
  await expect(thinkingText(page)).toHaveCount(0);
  await expect(conversation(page).getByText("The user wants Fibonacci")).toHaveCount(0);

  // open it: what was written so far, and the rest when the model goes on
  await toggle(page).click();
  await expect(toggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(thinkingText(page)).toContainText("I should write the iterative version,");
  await expect(thinkingText(page)).not.toContainText("and then say how it works.");
  await expectNoHorizontalScroll(page);

  await release(page, request);
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(thinking(page)).toHaveCount(1);
  await expect(thinking(page)).toHaveAttribute("data-streaming", "false");
  // the log's reasoning took the draft's place and the block did not close under the person's hand
  await expect(thinking(page)).toHaveAttribute("data-state", "open");
  await expect(thinkingText(page)).toContainText("and then say how it works.");
  await expect(conversation(page).getByText("The user wants Fibonacci")).toHaveCount(1);

  // the reply is below the block and has none of the reasoning
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toHaveText("Fibonacci in Rust.");
  const block = await thinking(page).boundingBox();
  const answer = await reply(page).boundingBox();
  expect(block && answer && block.y < answer.y, "the block is above the reply").toBe(true);
});

test("after a reload the reasoning is whole from the log, closed, above the one reply", async ({
  page,
}) => {
  await startThread(page, "think write fibonacci");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await page.reload();
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(thinking(page)).toHaveCount(1);
  await expect(thinking(page)).toHaveAttribute("data-state", "closed");
  await expect(thinking(page)).toHaveAttribute("data-streaming", "false");
  await expect(thinkingText(page)).toHaveCount(0);
  await expect(reply(page)).toHaveCount(1);
  await expect(reply(page)).toHaveText("Fibonacci in Rust.");

  await toggle(page).click();
  await expect(thinkingText(page)).toHaveText(
    "The user wants Fibonacci in Rust. I should write the iterative version, since it needs no recursion, and then say how it works.",
  );
  await toggle(page).click();
  await expect(thinkingText(page)).toHaveCount(0);
});

test("a reasoning the log cut at its bound is the part it kept", async ({ page }) => {
  await startThread(page, "think-cut write fibonacci");
  await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
  await expect(thinking(page)).toHaveCount(1);
  await toggle(page).click();
  await expect(thinkingText(page)).toContainText("The user wants Fibonacci in Rust.");
  await expect(reply(page)).toHaveText("Done.");
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility of the Thinking block (${scheme})`, () => {
    test.use({ colorScheme: scheme });

    test("axe: a block, open and being written, has no serious violations", async ({
      page,
      request,
    }) => {
      await startThread(page, "think-gate write fibonacci");
      await toggle(page).click();
      await expect(thinkingText(page)).toContainText("I should write the iterative version,");
      expect(await axeViolations(page)).toEqual([]);
      await release(page, request);
      await expect(badge(page)).toHaveText("Done", { timeout: 30_000 });
    });
  });
}
