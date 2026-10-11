import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

/**
 * Catalog version 6, group 3 (data) against the mock orchestrator, in a real browser: `components-data` is one answer
 * whose surface draws three Stats tiles, a bar, a line and a pie chart, a Table that sorts by its headers, and a
 * KeyValue. Nothing is requested from another origin, the table is reachable and sortable from the keyboard, and the
 * page is clean to axe in both colour schemes.
 */

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

async function axeViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.map(
    (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
  );
}

/** The first column's words, top to bottom, of the table the answer draws. */
const firstColumn = (page: Page) =>
  ui(page)
    .locator('[data-slot="table"] tbody tr')
    .evaluateAll((rows) => rows.map((r) => r.querySelector("td")?.textContent ?? ""));

test.describe("catalog version 6: data", () => {
  test("one answer draws the stats, the three charts, the table and the key-value list", async ({
    page,
  }) => {
    const hosts = new Set<string>();
    page.on("request", (r) => hosts.add(new URL(r.url()).host));

    await startThread(page, "components-data please", "Reviewer");
    const drawn = ui(page);
    await expect(drawn).toBeVisible();

    await expect(drawn.getByRole("list", { name: "Figures" })).toContainText("1,204");
    await expect(drawn.getByText("Up:", { exact: false })).toBeVisible();

    await expect(
      drawn.getByRole("img", { name: "Bar chart: Runs per day. 2 series over 3 points." }),
    ).toBeVisible();
    await expect(
      drawn.getByRole("img", { name: "Line chart: Latency. 2 series over 4 points." }),
    ).toBeVisible();
    await expect(
      drawn.getByRole("img", { name: "Pie chart: Traffic by service. 1 series over 3 points." }),
    ).toBeVisible();

    await expect(drawn.getByRole("region", { name: "Services" })).toBeVisible();
    await expect(drawn.locator('[data-slot="key-value"] dl')).toContainText("eu-west");

    expect([...hosts].filter((h) => h !== new URL(page.url()).host)).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
  });

  test("the table sorts by a header from the keyboard, and says how it is sorted", async ({
    page,
  }) => {
    await startThread(page, "components-data please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await expect(ui(page).locator('[data-slot="table"] tbody tr')).toHaveCount(3);
    expect(await firstColumn(page)).toEqual(["web", "api", "worker"]);
    const replicas = ui(page).getByRole("columnheader", { name: /Replicas/ });
    await replicas.getByRole("button").focus();
    await page.keyboard.press("Enter");
    await expect(replicas).toHaveAttribute("aria-sort", "ascending");
    expect(await firstColumn(page)).toEqual(["api", "web", "worker"]);
    await page.keyboard.press("Enter");
    await expect(replicas).toHaveAttribute("aria-sort", "descending");
    expect(await firstColumn(page)).toEqual(["worker", "web", "api"]);
  });

  test("a sort the person chose once the turn is done is still there a moment later", async ({
    page,
  }) => {
    await startThread(page, "components-data please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const replicas = ui(page).getByRole("columnheader", { name: /Replicas/ });
    await replicas.getByRole("button").click();
    await expect(replicas).toHaveAttribute("aria-sort", "ascending");
    await page.waitForTimeout(1500);
    await expect(replicas).toHaveAttribute("aria-sort", "ascending");
    expect(await firstColumn(page)).toEqual(["api", "web", "worker"]);
  });

  test("the data behind a chart opens from its disclosure", async ({ page }) => {
    await startThread(page, "components-data please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const bars = ui(page).locator('[data-slot="chart"][data-type="bar"]');
    await bars.getByText("Data", { exact: true }).click();
    await expect(bars.getByRole("cell", { name: "42" })).toBeVisible();
  });

  for (const scheme of ["light", "dark"] as const) {
    test.describe(`catalog version 6: data, axe (${scheme})`, () => {
      // the turn fades in for 160 ms: axe reads colours that are not at rest yet
      test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

      test("the drawn surface, with a chart's data and a sorted table open, has no violation", async ({
        page,
      }) => {
        test.setTimeout(60_000);
        await startThread(page, "components-data please", "Reviewer");
        await ui(page)
          .locator('[data-slot="chart"][data-type="bar"]')
          .getByText("Data", { exact: true })
          .click();
        await ui(page)
          .getByRole("columnheader", { name: /Replicas/ })
          .getByRole("button")
          .click();
        expect(await axeViolations(page)).toEqual([]);
      });
    });
  }
});
