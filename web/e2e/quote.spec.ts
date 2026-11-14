import AxeBuilder from "@axe-core/playwright";
import { test as base, expect, type Locator, type Page } from "@playwright/test";
import { animationsDone, badge, conversation, MOCK_URL, startThread } from "./helpers";

/*
 * Quote, Copy (docs/api/thread-markdown-v1.md "Client recipe for Copy and Quote"), against the mock: a button under the person's own
 * messages and under each agent turn puts a Markdown block quote into the composer, at its caret, without sending, and leaves the
 * focus in the box. The selection is made by the page's own selection API, as a person's drag makes it.
 */

const ORIGIN = "http://127.0.0.1:3000";

const test = base.extend<{ asReadOnly: () => Promise<void> }>({
  asReadOnly: async ({ context }, use, info) => {
    const session = `e2e-quote-${info.workerIndex}-${info.testId}`;
    await use(async () => {
      const res = await fetch(`${MOCK_URL}/__mock/config?me=read-only&session=${session}`, {
        method: "POST",
      });
      expect(res.ok).toBe(true);
      await context.addCookies([{ name: "mock-registry", value: session, url: ORIGIN }]);
    });
  },
});

const box = (page: Page) => page.getByLabel("Message", { exact: true });
const turn = (page: Page) => conversation(page).locator('[data-slot="agent-turn"]').last();
const mine = (page: Page) => conversation(page).locator('[data-slot="user-message"]').last();
const quoteOf = (message: Locator) => message.getByRole("button", { name: "Quote" });

/** Selects characters `from`..`to` of the first text node under `target`'s first element that matches `within`, as a drag would. */
async function select(target: Locator, from: number, to: number, within?: string) {
  await target.evaluate(
    (root, [a, b, selector]) => {
      const scope = selector ? (root.querySelector(selector as string) ?? root) : root;
      const walker = document.createTreeWalker(scope, NodeFilter.SHOW_TEXT);
      const node = walker.nextNode() as Text;
      const range = document.createRange();
      range.setStart(node, a as number);
      range.setEnd(node, b as number);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    },
    [from, to, within] as const,
  );
}

async function talk(page: Page, text = "talk") {
  await startThread(page, text, "Reviewer");
  await expect(badge(page)).toHaveText("Done");
}

test.describe("Quote", () => {
  test("quotes the selection into the composer without sending", async ({ page }) => {
    await talk(page);
    const posts: string[] = [];
    page.on("request", (r) => {
      if (r.method() === "POST" && r.url().includes("/agui/agents/")) posts.push(r.url());
    });
    const words = turn(page).locator('[data-slot="agent-message"]');
    await expect(words).toContainText("Plan: add a test");
    await select(words, 6, 16); // "add a test"
    await quoteOf(turn(page)).click();
    await expect(box(page)).toHaveValue("> add a test\n\n");
    await expect(box(page)).toBeFocused();
    // nothing was sent: the thread has its one message and no run started
    await expect(conversation(page).locator('[data-slot="user-message"]')).toHaveCount(1);
    await expect(badge(page)).toHaveText("Done");
    expect(posts).toEqual([]);
  });

  test("quotes the whole message when nothing in it is selected, at the end of what was typed", async ({
    page,
  }) => {
    await talk(page);
    await box(page).fill("As you said");
    await quoteOf(turn(page)).click();
    await expect(box(page)).toHaveValue("As you said\n\n> Plan: add a test\n\n");
    await expect(box(page)).toBeFocused();
    // the person's own message can be quoted too
    await box(page).fill("");
    await mine(page).hover();
    await quoteOf(mine(page)).click();
    await expect(box(page)).toHaveValue("> talk\n\n");
  });

  test("a selection across two messages is not quoted, and the page says why", async ({ page }) => {
    await talk(page);
    await page.evaluate(() => {
      const user = document.querySelector('[data-slot="user-message"]') as HTMLElement;
      const agent = document.querySelector('[data-slot="agent-message"]') as HTMLElement;
      const range = document.createRange();
      range.setStart(user.querySelector("p, span, div") as Node, 0);
      range.setEnd(agent, agent.childNodes.length);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    });
    await quoteOf(turn(page)).click();
    await expect(box(page)).toHaveValue("");
    await expect(turn(page).getByRole("status")).toContainText("Select text inside one message");
  });

  test("a code selection stays code", async ({ page }) => {
    await talk(page, "code-math please");
    const code = conversation(page).locator("pre code", { hasText: "fn total" });
    await expect(code).toBeVisible();
    // `prices.iter().sum()` is on the third line of the block: select from its start to the end of the block
    await page.evaluate(() => {
      const code = [...document.querySelectorAll("pre code")].find((c) =>
        c.textContent?.includes("fn total"),
      ) as HTMLElement;
      const text = code.textContent ?? "";
      const at = text.indexOf("prices.iter().sum()");
      const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
      let seen = 0;
      const range = document.createRange();
      for (let n = walker.nextNode() as Text | null; n; n = walker.nextNode() as Text | null) {
        const len = n.data.length;
        if (!range.startContainer.parentElement?.closest("code") && at < seen + len) {
          range.setStart(n, at - seen);
        }
        if (at + "prices.iter().sum()".length <= seen + len) {
          range.setEnd(n, at + "prices.iter().sum()".length - seen);
          break;
        }
        seen += len;
      }
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    });
    await quoteOf(turn(page)).click();
    await expect(box(page)).toHaveValue("> ```rust\n> prices.iter().sum()\n> ```\n\n");
  });

  test("works from the keyboard and leaves the focus in the box", async ({ page }) => {
    await talk(page);
    const quote = quoteOf(turn(page));
    // Copy comes first, Quote is the next stop
    await turn(page).getByRole("button", { name: "Copy" }).focus();
    await page.keyboard.press("Tab");
    await expect(quote).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(box(page)).toBeFocused();
    await expect(box(page)).toHaveValue("> Plan: add a test\n\n");
    // the person writes under the quote and the caret is after it
    await page.keyboard.type("Why?");
    await expect(box(page)).toHaveValue("> Plan: add a test\n\nWhy?");
  });

  test("is not offered to a person who may only read", async ({ page, asReadOnly }) => {
    await talk(page);
    await asReadOnly();
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(turn(page).getByRole("button", { name: "Copy" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Quote" })).toHaveCount(0);
  });

  test("the quote buttons pass axe", async ({ page }) => {
    await talk(page);
    await mine(page).hover();
    await page.emulateMedia({ reducedMotion: "reduce" });
    await animationsDone(page);
    const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(
      results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
    ).toEqual([]);
  });
});

test.describe("Copy", () => {
  test("puts the source text on the clipboard as text, and says when the browser refuses", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: ORIGIN });
    await talk(page, "code-math please");
    await turn(page).getByRole("button", { name: "Copy" }).last().click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    // the Markdown as written: fences, `$$` lines and all
    expect(copied).toContain("```rust\nfn total(prices: &[u32]) -> u32 {");
    expect(copied).toContain("$$\n");
    // a code block's own Copy copies only its code
    await conversation(page)
      .locator(".aui-code-header-root", { hasText: "rust" })
      .getByRole("button", { name: "Copy" })
      .click();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
      'fn total(prices: &[u32]) -> u32 {\n    // sum in cents, so "$5" is 500\n    prices.iter().sum()\n}',
    );

    await page.evaluate(() => {
      Object.defineProperty(navigator.clipboard, "writeText", {
        value: () => Promise.reject(new DOMException("denied", "NotAllowedError")),
        configurable: true,
      });
    });
    await turn(page).getByRole("button", { name: "Copy" }).last().click();
    await expect(turn(page).getByText("Could not copy").first()).toBeVisible();
  });
});
