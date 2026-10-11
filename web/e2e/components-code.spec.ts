import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { conversation, startThread } from "./helpers";

/**
 * Catalog version 6, group 1 (code and files) against the mock orchestrator, in a real browser:
 * `components-code` is one answer whose surface draws a Code, a Diff, a Terminal whose output folds, a
 * PullRequest and a FileCard of a kept file. Nothing the surface draws may fetch from another origin, and
 * the page is clean to axe in both colour schemes.
 */

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

async function axeViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.map(
    (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
  );
}

test.describe("catalog version 6: code and files", () => {
  test("one answer draws the code, the diff, the run, the pull request and the file", async ({
    page,
  }) => {
    const hosts = new Set<string>();
    page.on("request", (r) => hosts.add(new URL(r.url()).host));

    await startThread(page, "components-code please", "Reviewer");
    const drawn = ui(page);
    await expect(drawn).toBeVisible();

    const code = drawn.locator('[data-slot="code"]');
    await expect(code.locator("pre code")).toContainText("return password.length > 0;");
    await expect(code.getByRole("button", { name: "Copy" })).toBeEnabled();

    const added = drawn.locator('[data-kind="add"]');
    await expect(added).toHaveCount(1);
    await expect(added).toContainText("Added: return password.length > 0;");
    await expect(drawn.locator('[data-kind="remove"]')).toContainText("Removed: return true;");

    await expect(drawn.getByText(/Exit 0 · passed/)).toBeVisible();
    const output = drawn.locator('[data-slot="terminal-output"]');
    await expect(output).not.toContainText("case 14 passes");
    await drawn.getByRole("button", { name: "Show all 17 lines" }).click();
    await expect(output).toContainText("case 14 passes");
    await expect(drawn.getByRole("button", { name: "Show less" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    const link = drawn.getByRole("link", { name: /Reject the empty password/ });
    await expect(link).toHaveAttribute("href", "https://github.com/acme/demo/pull/42");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await expect(drawn.getByRole("list", { name: "Checks" })).toContainText("2 passed");

    const download = drawn.getByRole("link", { name: "Download notes.txt" });
    await expect(download).toHaveAttribute("href", /\/artifacts\/[0-9a-f]{64}\?download=1$/);

    // the page talks to its own origin only: its API is behind the same server
    expect([...hosts].filter((h) => h !== new URL(page.url()).host)).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
  });

  test("the folded output opens from the keyboard", async ({ page }) => {
    await startThread(page, "components-code please", "Reviewer");
    const toggle = ui(page).getByRole("button", { name: "Show all 17 lines" });
    await toggle.focus();
    await page.keyboard.press("Enter");
    await expect(ui(page).locator('[data-slot="terminal-output"]')).toContainText("case 14 passes");
  });
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`catalog version 6: code and files, axe (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("the drawn surface, with its folded output open, has no violation", async ({ page }) => {
      test.setTimeout(60_000);
      await startThread(page, "components-code please", "Reviewer");
      await expect(ui(page).locator('[data-slot="code"]')).toBeVisible();
      await ui(page).getByRole("button", { name: "Show all 17 lines" }).click();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
