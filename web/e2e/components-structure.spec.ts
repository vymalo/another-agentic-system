import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { conversation, startThread } from "./helpers";

/**
 * Catalog version 6, group 2 (structure and status) against the mock orchestrator, in a real browser:
 * `components-structure` is one answer whose surface draws a Callout, a Checklist, a Timeline, Tabs and an
 * Accordion, whose children are components of the same surface. The tabs and the accordion work from the
 * keyboard, nothing is requested from another origin, and the page is clean to axe in both colour schemes.
 */

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

async function axeViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.map(
    (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
  );
}

test.describe("catalog version 6: structure and status", () => {
  test("one answer draws the callout, the checklist, the timeline, the tabs and the accordion", async ({
    page,
  }) => {
    const hosts = new Set<string>();
    page.on("request", (r) => hosts.add(new URL(r.url()).host));

    await startThread(page, "components-structure please", "Reviewer");
    const drawn = ui(page);
    await expect(drawn).toBeVisible();

    const note = drawn.locator('[data-slot="callout"][data-tone="warning"]');
    await expect(note).toContainText("The database is read-only");
    await expect(note.getByText("Warning", { exact: true })).toBeVisible();

    await expect(drawn.getByText("1 of 4 done")).toBeVisible();
    await expect(drawn.getByRole("progressbar", { name: "Progress" })).toHaveAttribute(
      "aria-valuenow",
      "1",
    );
    await expect(
      drawn.getByRole("list", { name: "Migration" }).getByText("In progress", { exact: true }),
    ).toBeVisible();
    await expect(drawn.getByText("Failed", { exact: true }).first()).toBeVisible();

    await expect(drawn.locator('[data-slot="timeline"]')).toContainText(
      "Dropping the old columns failed",
    );

    await expect(drawn.getByText("Two of four steps are done.")).toBeVisible();
    await drawn.getByRole("tab", { name: "Details" }).click();
    await expect(drawn.getByText("The copy reads 1.2 million rows.")).toBeVisible();
    await expect(drawn.getByText("Two of four steps are done.")).toHaveCount(0);

    const check = drawn.getByRole("button", { name: "How it was checked" });
    await expect(check).toHaveAttribute("aria-expanded", "false");
    await check.click();
    await expect(check).toHaveAttribute("aria-expanded", "true");
    await expect(
      drawn.getByText("Every table was counted before and after the copy."),
    ).toBeVisible();

    expect([...hosts].filter((h) => h !== new URL(page.url()).host)).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
  });

  test("the tabs move with the arrow keys", async ({ page }) => {
    await startThread(page, "components-structure please", "Reviewer");
    await ui(page).getByRole("tab", { name: "Overview" }).focus();
    await page.keyboard.press("ArrowRight");
    await expect(ui(page).getByRole("tab", { name: "Details" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(ui(page).getByText("The copy reads 1.2 million rows.")).toBeVisible();
  });

  test("the accordion opens from the keyboard", async ({ page }) => {
    await startThread(page, "components-structure please", "Reviewer");
    const check = ui(page).getByRole("button", { name: "How it was checked" });
    await check.focus();
    await page.keyboard.press("Enter");
    await expect(check).toHaveAttribute("aria-expanded", "true");
  });
});

for (const scheme of ["light", "dark"] as const) {
  test.describe(`catalog version 6: structure and status, axe (${scheme})`, () => {
    // the turn fades in for 160 ms: axe reads colours that are not at rest yet
    test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

    test("the drawn surface, with the second tab and the accordion open, has no violation", async ({
      page,
    }) => {
      test.setTimeout(60_000);
      await startThread(page, "components-structure please", "Reviewer");
      await ui(page).getByRole("tab", { name: "Details" }).click();
      await ui(page).getByRole("button", { name: "How it was checked" }).click();
      expect(await axeViolations(page)).toEqual([]);
    });
  });
}
