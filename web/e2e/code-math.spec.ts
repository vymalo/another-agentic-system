import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { watchCsp } from "./csp";
import { badge, conversation, startThread } from "./helpers";

/**
 * Code and math in an answer (web/README.md "Code and math in answers"), against the mock orchestrator in a real
 * browser: `code-math` is one answer of Markdown with a fenced `rust` block, a fenced block of a language the web has
 * no grammar for, two display formulas (one wider than the column), a sentence with `$$` in it and a sentence of dollar
 * prices. The highlighter runs without WebAssembly (the
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

/*
 * Strings that only the lazy chunks hold: KaTeX's own message and its stylesheet's font family, and Shiki's error class, a
 * grammar's scope and a theme's key (found in the build's chunks; none is in a chunk the first page loads). The test reads the
 * body of every script and stylesheet the page fetched, so it does not depend on a hashed file name.
 */
const KATEX = ["Too many expansions", "KaTeX_Main"];
const SHIKI = ["ShikiError", "source.rust", "activityBar.activeBorder"];

async function watchBundles(page: Page): Promise<() => Promise<string>> {
  const bodies: Promise<string>[] = [];
  page.on("response", (r) => {
    if (/\/_next\/static\/.+\.(js|css)(\?|$)/.test(r.url())) bodies.push(r.text().catch(() => ""));
  });
  return async () => (await Promise.all(bodies)).join("\n");
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

  test("an answer with no code and no math fetches no highlighter and no KaTeX", async ({
    page,
  }) => {
    const bundles = await watchBundles(page);
    const requested: string[] = [];
    page.on("request", (r) => requested.push(r.url()));
    await startThread(page, "talk please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await expect(conversation(page).getByText("Plan: add a test")).toBeVisible();
    // lazy chunks are fetched as soon as the markdown is drawn; give a stray one the time to arrive
    await page.waitForTimeout(1000);
    const text = await bundles();
    // the scan reads something (the page's own scripts), so an empty read cannot pass for a clean one
    expect(text.length).toBeGreaterThan(100_000);
    for (const marker of [...KATEX, ...SHIKI]) expect(text, marker).not.toContain(marker);
    expect(requested.filter((u) => /KaTeX_.*\.(woff2?|ttf)/.test(u))).toEqual([]);
  });

  test("an answer with code and math fetches them (so the scan above can see what it looks for)", async ({
    page,
  }) => {
    const bundles = await watchBundles(page);
    await answer(page);
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(conversation(page).locator(".katex-display").first()).toBeVisible();
    const text = await bundles();
    for (const marker of [...KATEX, ...SHIKI]) expect(text, marker).toContain(marker);
  });

  test("an unknown language stays plain text", async ({ page }) => {
    await answer(page);
    await expect(plain(page)).toContainText("++++++++[>++++++++<-]>+.");
    // the known block is coloured by now, so the other has had every chance to be
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(plain(page).locator("[data-token]")).toHaveCount(0);
  });

  test("display math is MathML for a screen reader and HTML for the eye, named by no raw TeX", async ({
    page,
  }) => {
    await answer(page);
    const formula = conversation(page).locator(".katex-display").first();
    await expect(formula).toBeVisible();
    await expect(formula.locator(".katex-html")).toHaveAttribute("aria-hidden", "true");
    await expect(formula.locator("math")).toHaveCount(1);
    // the MathML carries the formula; nothing is labelled with its source
    expect((await formula.getAttribute("aria-label")) ?? "").not.toContain("\\");
    const labels = await conversation(page)
      .locator("[aria-label]")
      .evaluateAll((els) => els.map((el) => el.getAttribute("aria-label") ?? ""));
    expect(labels.filter((label) => label.includes("\\"))).toEqual([]);
  });

  test("a display formula is a tab stop only while it is wider than its column", async ({
    page,
  }) => {
    await answer(page);
    const formulas = conversation(page).locator(".katex-display");
    await expect(formulas).toHaveCount(2);
    // the sum of sixty terms is wider than any column: it scrolls, so a keyboard must reach it
    const wide = formulas.nth(1);
    await expect(wide).toHaveAttribute("tabindex", "0");
    await expect(wide).toHaveAccessibleName("Formula");
    await wide.focus();
    await expect(wide).toBeFocused();
    // the other is a tab stop exactly when it overflows (on a phone it may, on a desktop it does not)
    await expect
      .poll(() =>
        formulas
          .first()
          .evaluate(
            (el) => (el.getAttribute("tabindex") === "0") === el.scrollWidth > el.clientWidth,
          ),
      )
      .toBe(true);
  });

  test("prices are not read as math", async ({ page }) => {
    await answer(page);
    await expect(
      conversation(page).getByText(
        "The two plans cost $5 and $10 a month, and the extra seat is $20 more.",
      ),
    ).toBeVisible();
    // `$$` inside a sentence is text, as written, and not a formula
    await expect(
      conversation(page).getByText("In bash, $$ is the process id; print $$ twice."),
    ).toBeVisible();
    // the two formulas of the answer are the only ones KaTeX drew, and the "$5" in the code is code
    await expect(conversation(page).locator(".katex")).toHaveCount(2);
    await expect(conversation(page).locator(".katex-error")).toHaveCount(0);
  });

  test("no CSP violation, no WebAssembly, and everything comes from the page's own origin", async ({
    page,
  }) => {
    const violations = await watchCsp(page);
    const requested: string[] = [];
    page.on("request", (r) => requested.push(r.url()));
    await answer(page);
    await expect(rust(page).locator("[data-token]").first()).toBeVisible();
    await expect(conversation(page).locator(".katex-display").first()).toBeVisible();
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
      await expect(conversation(page).locator(".katex-display").first()).toBeVisible();
      const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(
        results.violations.map(
          (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
        ),
      ).toEqual([]);
    });
  }
});
