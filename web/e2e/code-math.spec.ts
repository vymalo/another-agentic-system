import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { watchCsp } from "./csp";
import { badge, conversation, startThread } from "./helpers";

/**
 * Code and math in an answer (web/README.md "Code and math in answers"), against the mock orchestrator in a real
 * browser: `code-math` is one answer of Markdown with a fenced `rust` block, a fenced block of a language the web has
 * no grammar for, a display formula and a sentence of dollar prices. The highlighter runs without WebAssembly (the
 * page's policy has no `wasm-unsafe-eval`), its fonts and chunks are the page's own, and the page is clean to axe
 * in both colour schemes.
 */

const rust = (page: Page) => conversation(page).locator("pre code", { hasText: "fn total" });
const plain = (page: Page) => conversation(page).locator("pre code", { hasText: "++++++++" });

async function answer(page: Page) {
  await startThread(page, "code-math please", "Reviewer");
  await expect(badge(page)).toHaveText("Done");
  await expect(rust(page)).toBeVisible();
}

async function colourOf(page: Page, text: string): Promise<string> {
  return rust(page)
    .locator("[data-token]", { hasText: text })
    .first()
    .evaluate((el) => getComputedStyle(el).color);
}

test.describe("code and math in an answer", () => {
  test("a fenced block is highlighted by language, in the colours of each scheme", async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await answer(page);
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(
      conversation(page).locator(".aui-code-header-language", { hasText: "rust" }),
    ).toBeVisible();
    // every character is still there, whatever the colours
    await expect(rust(page)).toContainText('// sum in cents, so "$5" is 500');
    await expect(rust(page)).toContainText("prices.iter().sum()");

    const keywordLight = await colourOf(page, "fn");
    const commentLight = await colourOf(page, "// sum in cents");
    expect(keywordLight).not.toBe(commentLight);
    await page.emulateMedia({ colorScheme: "dark" });
    expect(await colourOf(page, "fn")).not.toBe(keywordLight);
  });

  test("an unknown language stays plain text", async ({ page }) => {
    await answer(page);
    await expect(plain(page)).toContainText("++++++++[>++++++++<-]>+.");
    // the known block is coloured by now, so the other has had every chance to be
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(plain(page).locator("[data-token]")).toHaveCount(0);
  });

  test("display math renders with an accessible name", async ({ page }) => {
    await answer(page);
    const formula = conversation(page).getByRole("math", { name: /\\int_\{-\\infty\}/ });
    await expect(formula).toBeVisible();
    await expect(formula).toHaveAccessibleName(
      "\\int_{-\\infty}^{\\infty} e^{-x^2}\\,dx = \\sqrt{\\pi}",
    );
    // the picture of the formula is KaTeX's HTML, and a screen reader gets the MathML beside it
    await expect(formula.locator(".katex-html")).toHaveAttribute("aria-hidden", "true");
    await expect(formula.locator("math")).toHaveCount(1);
    // it is a tab stop: a formula wider than the column scrolls
    await expect(formula).toHaveAttribute("tabindex", "0");
  });

  test("prices are not read as math", async ({ page }) => {
    await answer(page);
    await expect(
      conversation(page).getByText(
        "The two plans cost $5 and $10 a month, and the extra seat is $20 more.",
      ),
    ).toBeVisible();
    // the one formula of the answer is the only one KaTeX drew, and the "$5" in the code is code
    await expect(conversation(page).locator(".katex")).toHaveCount(1);
  });

  test("no CSP violation, no WebAssembly, and everything comes from the page's own origin", async ({
    page,
  }) => {
    const violations = await watchCsp(page);
    const requested: string[] = [];
    page.on("request", (r) => requested.push(r.url()));
    await answer(page);
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(conversation(page).locator(".katex-display")).toBeVisible();
    expect(await violations()).toEqual([]);

    const own = new URL(page.url()).origin;
    const foreign = requested.filter((u) => u.startsWith("http") && new URL(u).origin !== own);
    expect(foreign, "requests to another origin").toEqual([]);
    expect(requested.filter((u) => /\.wasm(\?|$)/.test(u))).toEqual([]);
    // KaTeX's own fonts are the ones the page drew with
    expect(requested.some((u) => /KaTeX_.*\.woff2/.test(u))).toBe(true);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`axe passes on the answer in the ${scheme} scheme`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
      await answer(page);
      await expect(rust(page).locator("[data-token]").first()).toBeVisible();
      await expect(conversation(page).locator(".katex-display")).toBeVisible();
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(
        results.violations.map(
          (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
        ),
      ).toEqual([]);
    });
  }
});
