import AxeBuilder from "@axe-core/playwright";
import { type APIRequestContext, expect, type Locator, type Page, test } from "@playwright/test";
import {
  animationsDone,
  badge,
  conversation,
  expectNoHorizontalScroll,
  MOCK_URL,
  startThread,
} from "./helpers";

/*
 * The token ring (ADR 0056, docs/api/usage-v1.md), against the mock's `usage*` scripts (mock/scripts.ts),
 * whose `model_usage` and `model_usage_total` events the mock projects as the orchestrator does
 * (`vymalo.usage`, `vymalo.usage_total`). The ring is a button beside Send: it fills with how full the
 * context of the agent's latest call is, amber from 80 %, red from 95 %, and opens the thread's totals
 * per model and what the agent and each sub-agent spent, apart. A thread with no usage has no ring.
 */

const ring = (page: Page) => page.getByRole("button", { name: /^Token usage:/ });
const details = (page: Page) => page.getByRole("dialog", { name: "Token usage" });

/** The row of a details table whose name is exactly `name`. */
const row = (page: Page, table: Locator, name: string) =>
  table.getByRole("row").filter({ has: page.getByText(name, { exact: true }) });

const threadOf = (page: Page) => new URL(page.url()).pathname.split("/").pop() ?? "";

async function release(page: Page, request: APIRequestContext) {
  const res = await request.post(`${MOCK_URL}/__mock/release?thread=${threadOf(page)}`);
  expect(res.status(), "the release of a held run").toBe(204);
}

test("the ring fills with the agent's last call and opens the totals per model and per agent", async ({
  page,
}) => {
  await startThread(page, "usage count the tokens");
  await expect(badge(page)).toHaveText("Done");
  await expect(conversation(page).getByText("Done.")).toBeVisible();

  // the agent's last call: 2,400 of 131,072 tokens, under 80 %
  await expect(ring(page)).toHaveAccessibleName(
    "Token usage: context 2 % full, 2,400 of 131,072 tokens",
  );
  await expect(ring(page)).toHaveAttribute("data-level", "normal");
  await expectNoHorizontalScroll(page);

  // the details, by the keyboard: the task's totals per model, and the agent and its sub-agent apart
  await ring(page).focus();
  await page.keyboard.press("Enter");
  await expect(details(page)).toBeVisible();
  const byModel = details(page).getByRole("table", { name: "This thread, by model" });
  const big = row(page, byModel, "glm-5.3");
  await expect(big).toContainText("openai");
  await expect(big).toContainText("3,600 in");
  await expect(big).toContainText("1,000 cached");
  await expect(big).toContainText("200 out");
  await expect(big).toContainText("20 reasoning");
  await expect(row(page, byModel, "glm-5.3-mini")).toContainText("600 in");
  const byWho = details(page).getByRole("table", { name: "By who spent it" });
  await expect(row(page, byWho, "Adam")).toContainText("Agent · 2 calls");
  await expect(row(page, byWho, "Adam")).toContainText("3,600 in");
  await expect(row(page, byWho, "Researcher")).toContainText("Sub-agent · 1 call");
  await expect(row(page, byWho, "Researcher")).toContainText("600 in");
  await expectNoHorizontalScroll(page);
  await page.keyboard.press("Escape");
  await expect(details(page)).toHaveCount(0);
  await expect(ring(page)).toBeFocused();

  // a reload replays the log: the same ring
  await page.reload();
  await expect(ring(page)).toHaveAccessibleName(
    "Token usage: context 2 % full, 2,400 of 131,072 tokens",
  );
});

test("the ring turns red from 95 % and has no fill for a call with no context window", async ({
  page,
}) => {
  await startThread(page, "usage-full fill it");
  await expect(badge(page)).toHaveText("Done");
  await expect(ring(page)).toHaveAttribute("data-level", "danger");
  await expect(ring(page)).toHaveAccessibleName(
    "Token usage: context 96 % full, 126,000 of 131,072 tokens",
  );

  await startThread(page, "usage-nowindow count it");
  await expect(badge(page)).toHaveText("Done");
  await expect(ring(page)).toHaveAttribute("data-level", "none");
  await expect(ring(page)).toHaveAccessibleName(
    "Token usage: last call 5,000 input tokens, no context window known",
  );
  await expect(ring(page).locator('[data-slot="usage-fill"]')).toHaveCount(0);
  // a ring with no fill still opens the totals
  await ring(page).click();
  await expect(details(page)).toContainText("its context window is not known");
  await expect(details(page).getByRole("table", { name: "This thread, by model" })).toContainText(
    "5,000 in",
  );
});

test("the ring moves while the agent works, and a thread with no usage has none", async ({
  page,
  request,
}) => {
  await startThread(page, "talk to me");
  await expect(badge(page)).toHaveText("Done");
  await expect(ring(page)).toHaveCount(0);

  await startThread(page, "usage-hold hold on");
  await expect(ring(page)).toHaveAttribute("data-level", "normal");
  await release(page, request);
  await expect(ring(page)).toHaveAttribute("data-level", "warn");
  await expect(ring(page)).toHaveAccessibleName(
    "Token usage: context 84 % full, 110,000 of 131,072 tokens",
  );
  await expect(badge(page)).toHaveText("Done");
});

test("the ring and its details have no accessibility violations", async ({ page }) => {
  await startThread(page, "usage count the tokens");
  await expect(badge(page)).toHaveText("Done");
  await ring(page).click();
  await expect(details(page)).toBeVisible();
  await animationsDone(page);
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  const serious = results.violations.filter(
    (v) => v.impact === "serious" || v.impact === "critical",
  );
  expect(serious).toEqual([]);
});
