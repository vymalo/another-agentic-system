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
 * Files an agent made (ADR 0032, plan 10 S12) against the mock orchestrator, in a real browser: the
 * card with its preview and download, the Sources panel, the catalog's `Image`, and a file that was not
 * kept. The mock serves the golden's PNG at the real route with the real headers (`mock/files.ts`).
 * `files` keeps an image, a text file and an archive; `file-image` places the image in the answer by its
 * hash; `file-image-foreign` names a hash the thread does not hold; `file-svg` keeps an SVG written to
 * run a script and load things; `file-lost` is a file the store did not keep.
 */

const cards = (page: Page) => conversation(page).locator('[data-slot="file-card"]');
const cardNamed = (page: Page, name: string) =>
  cards(page).filter({ has: page.locator('[data-slot="file-name"]', { hasText: name }) });

/** The route a file is fetched from, and nothing else is: `/api/threads/<id>/artifacts/<sha256>`. */
const FILE_ROUTE =
  /^http:\/\/127\.0\.0\.1:3000\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}(\?download=1)?$/;

/** Every URL the page asked for. */
function requested(page: Page): string[] {
  const urls: string[] = [];
  page.on("request", (r) => urls.push(r.url()));
  return urls;
}

/** What the page asked for that is not the app itself, a data URL or a file route. */
const outside = (urls: string[]) =>
  urls.filter((u) => !/^(http:\/\/127\.0\.0\.1:(3000|4010)\/|data:|blob:)/.test(u));

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

test.describe("files: an image, a text file and an archive", () => {
  test("each is a card with its name, size and download; the picture and the text are shown", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "files make some", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await expect(cards(page)).toHaveCount(3);

    // the image: a picture of the file's own route, named by its file name, a download beside it
    const chart = cardNamed(page, "results.png");
    await expect(chart.locator('[data-slot="file-details"]')).toHaveText(
      /^\d+(\.\d)? (bytes|KB) · image\/png$/,
    );
    const img = chart.getByRole("img", { name: "results.png" });
    await expectDecoded(img);
    expect(await img.getAttribute("src")).toMatch(
      /^\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/,
    );
    const download = chart.getByRole("link", { name: "Download results.png" });
    expect(await download.getAttribute("href")).toMatch(/\/artifacts\/[0-9a-f]{64}\?download=1$/);

    // the text: read from the route and drawn as text, markup and all
    const notes = cardNamed(page, "notes.txt");
    const pre = notes.locator('[data-slot="file-text"]');
    await expect(pre).toContainText("Results of the run");
    await expect(pre).toContainText("<script>alert(1)</script> is text here, never markup");
    await expect(pre.locator("script")).toHaveCount(0);
    await expect(notes.getByRole("img")).toHaveCount(0);

    // the archive: the card alone
    const zip = cardNamed(page, "export.zip");
    await expect(zip.locator('[data-slot="file-details"]')).toHaveText(
      "28 bytes · application/zip",
    );
    await expect(zip.locator("img, pre")).toHaveCount(0);
    await expect(zip.getByRole("link", { name: "Download export.zip" })).toBeVisible();

    // the page asked only for the app and for the files' own routes
    expect(outside(urls)).toEqual([]);
    const fileUrls = urls.filter((u) => u.includes("/artifacts/"));
    expect(fileUrls.length).toBeGreaterThan(0);
    for (const u of fileUrls) expect(u, u).toMatch(FILE_ROUTE);
    await expectNoHorizontalScroll(page);

    // a reload replays the same cards
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(cards(page)).toHaveCount(3);
    await expectDecoded(cardNamed(page, "results.png").getByRole("img"));
  });

  test("the download link saves the file under its name, as the API sends it", async ({ page }) => {
    await startThread(page, "files make some", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const link = cardNamed(page, "notes.txt").getByRole("link", { name: "Download notes.txt" });
    const [download] = await Promise.all([page.waitForEvent("download"), link.click()]);
    expect(download.suggestedFilename()).toBe("notes.txt");
    await expect(page).toHaveURL(/\/threads\/[0-9a-f-]{36}$/);
  });

  test("the Sources panel lists every file of the thread, with a download", async ({ page }) => {
    await startThread(page, "files make some", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    if ((await panelToggle(page).getAttribute("aria-expanded")) !== "true") {
      await panelToggle(page).click();
    }
    await expect(panel(page)).toBeVisible();
    await panelTab(page, "Sources").click();
    const files = panel(page).getByRole("region", { name: "Files" });
    await expect(files).toBeVisible();
    await expect(files.getByRole("listitem")).toHaveCount(3);
    for (const name of ["results.png", "notes.txt", "export.zip"]) {
      const row = files.getByRole("listitem").filter({ hasText: name });
      await expect(row.getByRole("link", { name: `Download ${name}` })).toHaveAttribute(
        "href",
        /\/artifacts\/[0-9a-f]{64}\?download=1$/,
      );
    }
    await expect(files.getByText(/^File · [\d.]+ (bytes|KB) · image\/png$/)).toBeVisible();
  });
});

test.describe("an SVG written to run a script and load things", () => {
  test("is drawn as a picture and does nothing: no script, no handler, no request", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "file-svg now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const card = cardNamed(page, "diagram.svg");
    const img = card.getByRole("img", { name: "diagram.svg" });
    await expectDecoded(img);
    // give a handler, a script or a stylesheet every chance to run
    await img.click({ position: { x: 20, y: 20 } });
    await page.waitForTimeout(500);
    expect(
      await page.evaluate(() => (window as unknown as Record<string, unknown>).__svgFlag),
    ).toBeUndefined();
    expect(urls.filter((u) => u.includes("evil.example"))).toEqual([]);
    expect(outside(urls)).toEqual([]);
    // the page holds a picture: none of the file's markup is in it
    await expect(card.locator("svg:not([aria-hidden]), object, embed, iframe, script")).toHaveCount(
      0,
    );
  });
});

test.describe("the Image component", () => {
  test("places a file of the thread in the answer, by its hash, with its alt and caption", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "file-image make a chart", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const ui = conversation(page).getByRole("region", { name: "Interface from reviewer" });
    await expect(ui.getByText("The results at a glance")).toBeVisible();
    const img = ui.getByRole("img", { name: "A chart of the results of the run" });
    await expectDecoded(img);
    expect(await img.getAttribute("src")).toMatch(
      /^\/api\/threads\/[0-9a-f-]{36}\/artifacts\/[0-9a-f]{64}$/,
    );
    await expect(ui.getByText("Figure 1: the results of the run")).toBeVisible();
    // the card of the same file is in the turn too, once
    await expect(cards(page)).toHaveCount(1);
    expect(outside(urls)).toEqual([]);
    await expectNoHorizontalScroll(page);

    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expectDecoded(
      conversation(page)
        .getByRole("region", { name: "Interface from reviewer" })
        .getByRole("img", { name: "A chart of the results of the run" }),
    );
  });

  test("a hash the thread does not hold: the surface is refused, nothing is drawn or fetched", async ({
    page,
  }) => {
    const urls = requested(page);
    await startThread(page, "file-image-foreign now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText(/Interface not shown:/)).toBeVisible();
    await expect(log.locator('[data-rule="artifact"]')).toContainText(
      "not one of this thread's files",
    );
    await expect(log.getByRole("region", { name: /^Interface from/ })).toHaveCount(0);
    expect(urls.filter((u) => u.includes("0".repeat(64)))).toEqual([]);
  });
});

test.describe("a file that was not kept", () => {
  test("is the old card, with no download, and the error says why", async ({ page }) => {
    await startThread(page, "file-lost now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText("the file is too large to keep")).toBeVisible();
    await expect(log.getByText("dump")).toBeVisible();
    await expect(log.getByRole("link", { name: /Download/ })).toHaveCount(0);
    await expect(log.locator("img")).toHaveCount(0);
  });
});
