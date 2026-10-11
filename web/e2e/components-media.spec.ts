import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, test } from "@playwright/test";
import { badge, conversation, startThread } from "./helpers";

/**
 * Catalog version 6, group 4 (media) against the mock orchestrator, in a real browser: `components-media` is one answer
 * whose surface draws a Gallery of two kept pictures, a LinkPreview, a Video and an Audio, all of files of the thread or
 * of the words the agent gave. The gallery opens a picture larger and moves with the arrow keys; the link fetches
 * nothing; the video says it cannot be played (the mock keeps bytes no browser plays) and keeps its download; the sound
 * is read by the browser. The page is clean to axe in both colour schemes.
 */

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

async function axeViolations(page: Page): Promise<string[]> {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return results.violations.map(
    (v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`,
  );
}

test.describe("catalog version 6: media", () => {
  test("one answer draws the gallery, the link, the video and the sound", async ({ page }) => {
    const hosts = new Set<string>();
    page.on("request", (r) => hosts.add(new URL(r.url()).host));

    await startThread(page, "components-media please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const drawn = ui(page);

    await expect(
      drawn.getByRole("button", { name: "Open the picture: A bar chart of the results" }),
    ).toBeVisible();
    await expect(
      drawn.getByRole("button", { name: "Open the picture: A list of four people" }),
    ).toBeVisible();

    const docs = drawn.getByRole("link", { name: /PostgreSQL documentation/ });
    await expect(docs).toHaveAttribute("href", "https://www.postgresql.org/docs/current/");
    await expect(docs).toHaveAttribute("target", "_blank");
    await expect(docs).toHaveAttribute("rel", "noopener noreferrer");

    await expect(drawn.locator('[data-slot="video"] video')).toHaveAttribute(
      "src",
      /\/artifacts\/[0-9a-f]{64}$/,
    );
    await expect(drawn.getByText(/could not be played/)).toBeVisible({ timeout: 15_000 });
    await expect(drawn.getByRole("link", { name: "Download The demo" })).toBeVisible();

    await expect
      .poll(
        () =>
          drawn
            .locator('[data-slot="audio"] audio')
            .evaluate((el) => (el as HTMLAudioElement).readyState),
        {
          timeout: 15_000,
        },
      )
      .toBeGreaterThanOrEqual(1);

    // nothing the media or the link draws was requested from another origin
    expect([...hosts].filter((h) => h !== new URL(page.url()).host)).toEqual([]);
    expect(await axeViolations(page)).toEqual([]);
  });

  test("a picture opens larger, and the arrow keys move to the next one and back", async ({
    page,
  }) => {
    await startThread(page, "components-media please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await ui(page)
      .getByRole("button", { name: "Open the picture: A bar chart of the results" })
      .click();
    // the dialog is named by the picture it shows, so it is found by its role
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText("1 of 2")).toBeVisible();
    await page.keyboard.press("ArrowRight");
    await expect(dialog.getByText("2 of 2")).toBeVisible();
    await page.keyboard.press("ArrowLeft");
    await expect(dialog.getByText("1 of 2")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
  });

  for (const scheme of ["light", "dark"] as const) {
    test.describe(`catalog version 6: media, axe (${scheme})`, () => {
      // the turn fades in for 160 ms: axe reads colours that are not at rest yet
      test.use({ colorScheme: scheme, contextOptions: { reducedMotion: "reduce" } });

      test("the drawn surface, with a picture open, has no violation", async ({ page }) => {
        test.setTimeout(60_000);
        await startThread(page, "components-media please", "Reviewer");
        await expect(badge(page)).toHaveText("Done");
        await ui(page)
          .getByRole("button", { name: "Open the picture: A bar chart of the results" })
          .click();
        await expect(page.getByRole("dialog")).toBeVisible();
        expect(await axeViolations(page)).toEqual([]);
      });
    });
  }
});
