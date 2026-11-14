import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Page } from "@playwright/test";
import { currentMonth, monthLabel, previousMonth } from "../src/features/me/lib/month";
import { uuidv7 } from "../src/lib/uuid";
import { animationsDone, expectNoHorizontalScroll, MOCK_URL } from "./helpers";

/*
 * The usage page (ADR 0065), against the mock's `GET /api/me/usage`: the person's own month as the orchestrator priced
 * it. A test sets what its own session is answered with `POST /__mock/my-usage` (`{ "<YYYY-MM>": MyUsage, "*": MyUsage }`),
 * so the tests beside it, which share the mock, are not answered with it. Amounts are strings and are read as written.
 */

const ORIGIN = "http://127.0.0.1:3000";

type Answers = Record<string, unknown>;

const test = base.extend<{ answers: (answers: Answers) => Promise<void> }>({
  answers: async ({ context }, use) => {
    const session = `usage-${uuidv7()}`;
    await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    await use(async (answers) => {
      const res = await fetch(`${MOCK_URL}/__mock/my-usage?session=${session}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(answers),
      });
      expect(res.status).toBe(204);
    });
  },
});

const NOW = currentMonth();
const BEFORE = previousMonth(NOW) ?? "";

const COMPLETE = {
  month: NOW,
  currency: "EUR",
  prices: "sha256:0123456789abcdef",
  tasks: 7,
  models: [
    {
      provider: "zai",
      model: "glm-5.3",
      inputTokens: 1_250_000,
      outputTokens: 310_500,
      totalTokens: 1_560_500,
      cachedInputTokens: 400_000,
      reasoningTokens: 90_000,
      price: "3.141593",
    },
    {
      model: "local-8b",
      inputTokens: 2_000,
      outputTokens: 500,
      totalTokens: 2_500,
      price: "0.000000",
    },
  ],
  sandbox: [
    {
      environment: "kubernetes",
      cpuMillis: 2000,
      memoryBytes: 4 * 2 ** 30,
      commands: 41,
      milliseconds: 4_320_000,
      price: "0.250000",
    },
  ],
  unmeasured: [],
  total: {
    amount: "3.391593",
    currency: "EUR",
    complete: true,
    unpriced: { models: [], sandbox: 0, unmeasured: 0 },
  },
};

const INCOMPLETE = {
  ...COMPLETE,
  models: [
    { ...COMPLETE.models[0], price: "3.000000" },
    {
      provider: "zai",
      model: "glm-5.3-mini",
      inputTokens: 10,
      outputTokens: 20,
      totalTokens: 30,
      price: null,
    },
  ],
  sandbox: [
    { environment: "kubernetes", commands: 3, milliseconds: 90_000, price: null },
    ...COMPLETE.sandbox,
  ],
  unmeasured: [{ environment: "local", tasks: 2 }],
  total: {
    amount: "3.250000",
    currency: "EUR",
    complete: false,
    unpriced: { models: [{ provider: "zai", model: "glm-5.3-mini" }], sandbox: 1, unmeasured: 2 },
  },
};

const total = (page: Page) => page.getByRole("region", { name: "Total" });

test("lists models, sandbox and total", async ({ page, answers }) => {
  await answers({ "*": COMPLETE });
  await page.goto("/usage");
  await expect(page.getByRole("heading", { level: 1, name: "Usage" })).toBeVisible();
  await expect(page.getByText(monthLabel(NOW), { exact: true })).toBeVisible();
  await expect(total(page)).toContainText("3.391593 EUR");
  await expect(total(page)).not.toContainText(/at least/i);

  const models = page.getByRole("table", { name: "Models" });
  const glm = models.getByRole("row", { name: /glm-5\.3/ });
  await expect(glm).toContainText("zai");
  await expect(glm).toContainText("1,250,000 in");
  await expect(glm).toContainText("400,000 cached");
  await expect(glm).toContainText("310,500 out");
  await expect(glm).toContainText("90,000 reasoning");
  await expect(glm).toContainText("3.141593 EUR");
  // a price of zero is a price, said as one
  await expect(models.getByRole("row", { name: /local-8b/ })).toContainText("0.000000 EUR");

  const sandbox = page.getByRole("table", { name: "Sandbox" });
  const row = sandbox.getByRole("row").nth(1);
  await expect(row).toContainText("kubernetes");
  await expect(row).toContainText("2 cores, 4 GiB");
  await expect(row).toContainText("41 commands");
  await expect(row).toContainText("1 h 12 min");
  await expect(row).toContainText("0.250000 EUR");
  await expectNoHorizontalScroll(page);
});

test("an incomplete month says at least and names the unpriced lines", async ({
  page,
  answers,
}) => {
  await answers({ "*": INCOMPLETE });
  await page.goto("/usage");
  await expect(total(page)).toContainText("At least 3.250000 EUR");
  await expect(total(page)).toContainText("zai / glm-5.3-mini");
  await expect(total(page)).toContainText("1 sandbox shape with no price");
  await expect(total(page)).toContainText("2 tasks ran commands where the sandbox is not measured");
  const mini = page
    .getByRole("table", { name: "Models" })
    .getByRole("row", { name: /glm-5\.3-mini/ });
  await expect(mini).toContainText("Not priced");
  await expect(page.getByRole("table", { name: "Sandbox" }).getByText("Not priced")).toHaveCount(1);
  await expectNoHorizontalScroll(page);
});

test("an empty month is complete at zero", async ({ page, answers }) => {
  await answers({});
  await page.goto("/usage");
  await expect(total(page)).toContainText("0.000000 USD");
  await expect(total(page)).not.toContainText(/at least/i);
  await expect(page.getByText("No model usage this month.")).toBeVisible();
  await expect(page.getByText("No sandbox time this month.")).toBeVisible();
  await expect(page.getByRole("table")).toHaveCount(0);
});

test("a deployment with no prices shows tokens and time and no money", async ({
  page,
  answers,
}) => {
  await answers({
    "*": {
      ...COMPLETE,
      currency: null,
      prices: null,
      models: COMPLETE.models.map((m) => ({ ...m, price: null })),
      sandbox: COMPLETE.sandbox.map((s) => ({ ...s, price: null })),
      total: { ...COMPLETE.total, currency: null, complete: false },
    },
  });
  await page.goto("/usage");
  await expect(page.getByText("No prices are configured, so no cost is shown.")).toBeVisible();
  await expect(page.getByText("1,250,000 in")).toBeVisible();
  await expect(page.getByText("1 h 12 min").first()).toBeVisible();
  await expect(page.locator("main")).not.toContainText(/EUR|USD|Not priced/);
});

test("moves between months by keyboard", async ({ page, answers }) => {
  await answers({ "*": COMPLETE, [BEFORE]: { ...INCOMPLETE, month: BEFORE } });
  await page.goto("/usage");
  await expect(total(page)).toContainText("3.391593 EUR");
  // the future has nothing to read: the button says so and stays focusable (aria-disabled, not disabled)
  const next = page.getByRole("button", { name: "Next month" });
  await expect(next).toHaveAttribute("aria-disabled", "true");
  expect(await next.evaluate((b) => (b as HTMLButtonElement).disabled)).toBe(false);

  const previous = page.getByRole("button", { name: "Previous month" });
  await previous.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByText(monthLabel(BEFORE), { exact: true })).toBeVisible();
  await expect(total(page)).toContainText("At least 3.250000 EUR");
  // the buttons stay where they were: the focus is still on the one that was pressed
  await expect(previous).toBeFocused();

  await page.keyboard.press("Tab");
  await expect(next).toBeFocused();
  await page.keyboard.press("Space");
  await expect(page.getByText(monthLabel(NOW), { exact: true })).toBeVisible();
  await expect(total(page)).toContainText("3.391593 EUR");
  // at the current month the button that was pressed is the one that still has the focus
  await expect(next).toHaveAttribute("aria-disabled", "true");
  await expect(next).toBeFocused();
  // and pressing it again changes nothing
  await page.keyboard.press("Enter");
  await expect(page.getByText(monthLabel(NOW), { exact: true })).toBeVisible();
  await expect(next).toBeFocused();
});

test("a read that fails is said in words and can be tried again", async ({ page, answers }) => {
  await answers({ "*": COMPLETE });
  let fail = true;
  await page.route("**/api/me/usage*", (route) =>
    fail
      ? route.fulfill({
          status: 503,
          contentType: "application/problem+json",
          body: JSON.stringify({ title: "Unavailable", status: 503, detail: "the ledger is down" }),
        })
      : route.continue(),
  );
  await page.goto("/usage");
  await expect(page.getByRole("alert").filter({ hasText: "the ledger is down" })).toBeVisible();
  fail = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(total(page)).toContainText("3.391593 EUR");
});

test("the account menu leads to it, and Back to chat leads home", async ({
  page,
  isMobile,
  answers,
}) => {
  await answers({ "*": COMPLETE });
  await page.goto("/");
  if (isMobile) {
    await page.getByRole("button", { name: "Threads", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Threads" })).toBeVisible();
  }
  await page.getByRole("button", { name: /^Account: / }).click();
  await page.getByRole("menuitem", { name: "Usage" }).click();
  await expect(page).toHaveURL(/\/usage$/);
  await expect(total(page)).toContainText("3.391593 EUR");
  await page.getByRole("link", { name: "Back to chat" }).click();
  await expect(page).toHaveURL(`${ORIGIN}/`);
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`accessibility (${scheme})`, () => {
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    async function serious(page: Page) {
      await animationsDone(page);
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      return results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    }

    test("axe: a complete month and an incomplete one have no serious violations", async ({
      page,
      answers,
    }) => {
      await answers({ "*": COMPLETE, [BEFORE]: { ...INCOMPLETE, month: BEFORE } });
      await page.goto("/usage");
      await expect(total(page)).toContainText("3.391593 EUR");
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Previous month" }).click();
      await expect(total(page)).toContainText("At least");
      expect(await serious(page)).toEqual([]);
    });

    test("axe: an empty month has no serious violations", async ({ page, answers }) => {
      await answers({});
      await page.goto("/usage");
      await expect(page.getByText("No model usage this month.")).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });
  });
}
