import { expect, test } from "@playwright/test";
import { conversation, startThread } from "./helpers";

/*
 * The brand (web/DESIGN.md, "Brand") as the browser meets it: the icons are served, the head
 * declares them, the title is the product's, and the panda is drawn where it belongs.
 */

const PNG = "89504e470d0a1a0a";

test("the tab icons are served: favicon.ico, icon.svg, apple-icon.png", async ({ request }) => {
  const ico = await request.get("/favicon.ico");
  expect(ico.status()).toBe(200);
  expect(ico.headers()["content-type"]).toMatch(/^image\/(x-icon|vnd\.microsoft\.icon)/);
  // an icon directory: reserved 0, type 1, three images
  expect([...(await ico.body()).subarray(0, 6)]).toEqual([0, 0, 1, 0, 3, 0]);

  const svg = await request.get("/icon.svg");
  expect(svg.status()).toBe(200);
  expect(svg.headers()["content-type"]).toMatch(/^image\/svg\+xml/);
  expect(await svg.text()).toContain("<svg");

  const apple = await request.get("/apple-icon.png");
  expect(apple.status()).toBe(200);
  expect(apple.headers()["content-type"]).toBe("image/png");
  expect((await apple.body()).subarray(0, 8).toString("hex")).toBe(PNG);
});

test("the web app manifest names the product and its icons are served", async ({ request }) => {
  const res = await request.get("/manifest.webmanifest");
  expect(res.status()).toBe(200);
  const manifest = (await res.json()) as {
    name: string;
    short_name: string;
    theme_color: string;
    icons: { src: string; sizes: string; type: string; purpose?: string }[];
  };
  expect(manifest.name).toBe("another·agentic");
  expect(manifest.short_name).toBe("agentic");
  expect(manifest.theme_color).toBe("#3F7341");
  expect(manifest.icons.map((i) => i.purpose)).toContain("maskable");
  for (const icon of manifest.icons) {
    const file = await request.get(icon.src);
    expect(file.status(), icon.src).toBe(200);
    expect(file.headers()["content-type"], icon.src).toBe(icon.type);
  }
});

test("the head declares the icons, the manifest, the colours and the title", async ({ page }) => {
  await page.goto("/");
  await expect(page).toHaveTitle("another·agentic");
  const icons = await page
    .locator('head link[rel="icon"]')
    .evaluateAll((links) =>
      links.map((l) => ({ href: l.getAttribute("href"), type: l.getAttribute("type") })),
    );
  expect(icons.length).toBeGreaterThan(0);
  expect(icons.map((i) => i.href).join(" ")).toContain("/icon.svg");
  expect(icons.map((i) => i.href).join(" ")).toContain("/favicon.ico");
  await expect(page.locator('head link[rel="apple-touch-icon"]')).toHaveCount(1);
  await expect(page.locator('head link[rel="manifest"]')).toHaveCount(1);
  const themes = await page
    .locator('head meta[name="theme-color"]')
    .evaluateAll((metas) => metas.map((m) => [m.getAttribute("media"), m.getAttribute("content")]));
  expect(themes).toEqual([
    ["(prefers-color-scheme: light)", "#ffffff"],
    ["(prefers-color-scheme: dark)", "#141614"],
  ]);
});

test("the panda is in the sidebar and over the greeting, and the greeting is ours", async ({
  page,
  isMobile,
}) => {
  await page.goto("/");
  const greeting = page.getByRole("heading", { level: 1, name: "What should we get done?" });
  await expect(greeting).toBeVisible();
  // the mark over the greeting: a picture that loaded, hidden from assistive technology
  const mark = page.locator('main [data-slot="panda-mark"]');
  await expect(mark).toBeVisible();
  expect(await mark.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
  await expect(mark).toHaveAttribute("alt", "");
  if (!isMobile) {
    // the lockup: the panda, then the wordmark as the link home
    const home = page.getByRole("link", { name: "another·agentic" });
    await expect(home).toBeVisible();
    await expect(home.locator('[data-slot="panda-mark"]')).toBeVisible();
  }
});

test("an agent turn has the agent's letter, not the panda", async ({ page }) => {
  await startThread(page, "echo hello");
  const avatar = conversation(page).locator('[data-slot="agent-avatar"]').first();
  await expect(avatar).toBeVisible();
  await expect(avatar).toHaveText("A");
  await expect(avatar).toHaveAttribute("aria-hidden", "true");
  await expect(conversation(page).locator('[data-slot="panda-mark"]')).toHaveCount(0);
});
