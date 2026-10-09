import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  badge,
  conversation,
  expectNoHorizontalScroll,
  panel,
  panelTab,
  panelToggle,
  startThread,
} from "./helpers";

/**
 * Images in an agent's answer that mean files it shared (the owner's thread of 2026-10-09): the coder
 * shared screenshots with `share_file` and wrote `![…](shots/4-matches.png)`. `inline-images` of the
 * mock is that thread: two screenshots and an archive shared, each by a step "Share a file" with the
 * path it was called with and an artifact; an answer that places the screenshots by their paths and one
 * path nobody shared. The pictures are drawn in the words from the file's own route, the files they show
 * are not cards as well, a path that means no file is a small placeholder, and nothing is ever requested
 * from the path an agent wrote.
 */

const cards = (page: Page) => conversation(page).locator('[data-slot="file-card"]');
const words = (page: Page) => conversation(page).locator('[data-slot="agent-message"]');

/** The route a file is fetched from, and nothing else is: `/api/threads/<id>/artifacts/<sha256>`. */
const FILE_ROUTE =
  /^http:\/\/127\.0\.0\.1:3000\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}(\?download=1)?$/;

/** Every URL the page asked for. */
function requested(page: Page): string[] {
  const urls: string[] = [];
  page.on("request", (r) => urls.push(r.url()));
  return urls;
}

/** The image is decoded by the browser: it has a size, so it is a picture and not a broken icon. */
async function expectDecoded(img: Locator) {
  await expect(img).toBeVisible();
  await expect
    .poll(() =>
      img.evaluate(
        (el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth,
      ),
    )
    .toBeGreaterThan(0);
}

test.describe("images that mean files the agent shared", () => {
  test("are drawn in the answer, the placeholder stands for a path nobody shared, and only the unshown file is a card", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "inline-images show me", "Reviewer");
    await expect(badge(page)).toHaveText("Done");

    // the two screenshots are in the words, from the files' own route, with the agent's alt text
    const list = words(page).getByRole("img", { name: "The list of people" });
    const matches = words(page).getByRole("img", { name: "Matches list with percentages" });
    await expectDecoded(list);
    await expectDecoded(matches);
    for (const img of [list, matches]) {
      expect(await img.getAttribute("src")).toMatch(
        /^\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/,
      );
    }
    expect(await list.getAttribute("src")).not.toBe(await matches.getAttribute("src"));

    // the path nobody shared is an icon and its alt text: no picture, no broken glyph
    const missing = words(page).locator('[data-slot="md-image-text"]');
    await expect(missing).toHaveCount(1);
    await expect(missing).toContainText("The login page");
    await expect(missing.locator("svg")).toHaveCount(1);
    await expect(words(page).getByRole("img", { name: "The login page" })).toHaveCount(0);

    // no image of the page is broken
    const broken = await conversation(page)
      .locator("img")
      .evaluateAll(
        (els) =>
          els.filter(
            (e) => !(e as HTMLImageElement).complete || (e as HTMLImageElement).naturalWidth === 0,
          ).length,
      );
    expect(broken).toBe(0);

    // the files the words show are not repeated after them; the file no image names stays
    await expect(cards(page)).toHaveCount(1);
    await expect(cards(page).locator('[data-slot="file-name"]')).toHaveText("export.zip");
    await expect(cards(page).getByRole("link", { name: "Download export.zip" })).toBeVisible();

    // nothing was asked of the path an agent wrote, nor of any place outside the app and its files
    expect(urls.filter((u) => /shots\/|9-login|3-list\.png|4-matches\.png/.test(u))).toEqual([]);
    const outside = urls.filter(
      (u) => !/^(http:\/\/127\.0\.0\.1:(3000|4010)\/|data:|blob:)/.test(u),
    );
    expect(outside).toEqual([]);
    const fileUrls = urls.filter((u) => u.includes("/artifacts/"));
    expect(fileUrls.length).toBeGreaterThanOrEqual(2);
    for (const u of fileUrls) expect(u, u).toMatch(FILE_ROUTE);
    await expectNoHorizontalScroll(page);
  });

  test("the Sources panel still lists every shared file, with its download", async ({ page }) => {
    await startThread(page, "inline-images show me", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
      await panelToggle(page).click();
    }
    await expect(panel(page)).toBeVisible();
    await panelTab(page, "Sources").click();
    const files = panel(page).getByRole("region", { name: "Files" });
    await expect(files.getByRole("listitem")).toHaveCount(3);
    for (const name of ["3-list.png", "4-matches.png", "export.zip"]) {
      await expect(
        files
          .getByRole("listitem")
          .filter({ hasText: name })
          .getByRole("link", {
            name: `Download ${name}`,
          }),
      ).toHaveAttribute("href", /\/artifacts\/[0-9a-f]{64}\?download=1$/);
    }
  });

  test("a reload replays the same pictures, placeholder and card", async ({ page }) => {
    await startThread(page, "inline-images show me", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expectDecoded(words(page).getByRole("img", { name: "The list of people" }));
    await expectDecoded(words(page).getByRole("img", { name: "Matches list with percentages" }));
    await expect(words(page).locator('[data-slot="md-image-text"]')).toHaveCount(1);
    await expect(cards(page)).toHaveCount(1);
  });
});
