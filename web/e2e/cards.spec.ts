import { expect, type Page, test } from "@playwright/test";
import {
  agentMessage,
  badge,
  conversation,
  expectNoHorizontalScroll,
  startThread,
} from "./helpers";

/**
 * Cards and Mermaid (ADR 0023, catalog version 3) against the mock orchestrator, in a real browser
 * with the real mermaid: `cards-mermaid` is one answer of words, three cards and a graph;
 * `cards-bad` and `cards-bad-url` are surfaces that break the schema of Cards and are refused;
 * `mermaid-bad` is a graph that does not parse; `mermaid-hostile` is a graph written to get out of
 * the picture.
 */

const ui = (page: Page) =>
  conversation(page).getByRole("region", { name: "Interface from reviewer" });

/** Every URL the page asked for, to know that nothing outside the app was fetched. */
function requested(page: Page): string[] {
  const urls: string[] = [];
  page.on("request", (r) => urls.push(r.url()));
  return urls;
}

/** The SVG of the drawn graph, as the image holds it. */
async function drawnSvg(page: Page): Promise<string> {
  const src = await page.locator('[data-slot="mermaid-image"]').getAttribute("src");
  expect(src?.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
  return decodeURIComponent((src ?? "").split(",").slice(1).join(","));
}

/** The image is decoded by the browser: it has a size, so it is a picture and not a broken icon. */
async function expectDecoded(page: Page) {
  const img = page.locator('[data-slot="mermaid-image"]');
  await expect(img).toBeVisible();
  await expect
    .poll(() =>
      img.evaluate(
        (el) => (el as HTMLImageElement).complete && (el as HTMLImageElement).naturalWidth,
      ),
    )
    .toBeGreaterThan(50);
}

test.describe("Cards and Mermaid: one answer", () => {
  test("words, three cards and a picture of the graph, in one turn", async ({ page }) => {
    const urls = requested(page);
    await startThread(page, "cards-mermaid please", "Reviewer");
    await expect(badge(page)).toHaveText("Done");

    // one turn holds all three
    const turn = agentMessage(page, "I compared three ways to keep a login session");
    await expect(turn).toHaveCount(1);
    const surface = turn.getByRole("region", { name: "Interface from reviewer" });
    await expect(surface).toBeVisible();

    // the cards: a list of three, the sources linked, the last without a link
    await expect(
      surface.getByText("Compared on how each one handles revocation and lookups."),
    ).toBeVisible();
    await expect(
      surface.getByRole("heading", { name: "Three ways to keep a session" }),
    ).toBeVisible();
    const cards = surface.getByRole("list").getByRole("listitem");
    await expect(cards).toHaveCount(3);
    await expect(cards.first().getByText("Durable, one more query per request")).toBeVisible();
    await expect(cards.first().getByText("durable", { exact: true })).toBeVisible();
    const link = surface.getByRole("link", { name: /Server-side sessions in Postgres/ });
    await expect(link).toHaveAttribute("href", "https://www.postgresql.org/docs/current/");
    await expect(link).toHaveAttribute("target", "_blank");
    await expect(link).toHaveAttribute("rel", "noopener noreferrer");
    await expect(cards.first().getByText("postgresql.org")).toBeVisible();
    await expect(surface.getByRole("link")).toHaveCount(2);
    await expect(cards.nth(2).getByRole("link")).toHaveCount(0);

    // the graph: an image the browser decoded, named by its title, the caption under it
    await expectDecoded(page);
    await expect(surface.getByRole("img", { name: "How a request meets a session" })).toBeVisible();
    await expect(surface.getByText("A request without a cookie gets a new session.")).toBeVisible();

    // its text alternative is one click away: the source
    const source = surface.locator('[data-slot="mermaid-source"]');
    await expect(source.getByText("Diagram source")).toBeVisible();
    await expect(source.locator("pre")).toBeHidden();
    await source.getByText("Diagram source").click();
    await expect(source.locator("pre")).toContainText("flowchart TD");
    await expect(source.locator("pre")).toContainText("B -- no --> D[Create a session]");

    // the picture is of the graph: its nodes' words are in the SVG, as SVG text and not as HTML
    const svg = await drawnSvg(page);
    const words = svg.replace(/<style>[\s\S]*?<\/style>/g, "").replace(/<[^>]+>/g, "");
    for (const label of ["Request arrives", "Session cookie?", "Create a session"]) {
      expect(words).toContain(label);
    }
    expect(svg).not.toContain("<foreignObject");
    expect(svg).not.toMatch(/<script/i);

    // nothing outside the app was asked for: no image, no font, no favicon of the cards' hosts
    const outside = urls.filter((u) => !/^(http:\/\/127\.0\.0\.1:(3000|4010)\/|data:)/.test(u));
    expect(outside).toEqual([]);
    await expectNoHorizontalScroll(page);

    // a reload replays the same answer
    await page.reload();
    await expect(badge(page)).toHaveText("Done");
    await expect(page.getByRole("region", { name: "Interface from reviewer" })).toBeVisible();
    await expectDecoded(page);
  });

  test("the library is loaded when a graph is drawn, not with every thread", async ({ page }) => {
    const scripts = (list: string[]) => new Set(list.filter((u) => /\.js(\?|$)/.test(u)));
    const plain = requested(page);
    await startThread(page, "echo hello", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const without = scripts(plain);

    const graph = await page.context().newPage();
    const withGraph = requested(graph);
    await startThread(graph, "cards-mermaid please", "Reviewer");
    await expect(badge(graph)).toHaveText("Done");
    await expectDecoded(graph);
    const withScripts = scripts(withGraph);

    // every script the plain thread needed is the graph thread's, and the graph thread needed more
    for (const u of without) expect(withScripts.has(u), u).toBe(true);
    expect(withScripts.size).toBeGreaterThan(without.size);
  });

  test.describe("dark", () => {
    test.use({ colorScheme: "dark" });

    test("the graph is drawn in the colours of the dark page", async ({ page }) => {
      await startThread(page, "cards-mermaid please", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expectDecoded(page);
      const svg = await drawnSvg(page);
      // the page's own tokens: dark text on the dark card would be unreadable
      expect(svg.toLowerCase()).toContain("#e8e9e4"); // --foreground, dark
      expect(svg.toLowerCase()).toContain("#232723"); // --muted, dark: the nodes
      expect(svg.toLowerCase()).not.toContain("#f3f2ec"); // --muted, light
    });
  });

  test.describe("light", () => {
    test.use({ colorScheme: "light" });

    test("the graph is drawn in the colours of the light page", async ({ page }) => {
      await startThread(page, "cards-mermaid please", "Reviewer");
      await expect(badge(page)).toHaveText("Done");
      await expectDecoded(page);
      const svg = await drawnSvg(page);
      expect(svg.toLowerCase()).toContain("#1a1d1a"); // --foreground, light
      expect(svg.toLowerCase()).toContain("#f3f2ec"); // --muted, light: the nodes
      expect(svg.toLowerCase()).not.toContain("#232723"); // --muted, dark
    });
  });
});

test.describe("a surface that breaks the schema of Cards", () => {
  test("a card without a title (and a javascript: link): refused by the schema, with the reason, nothing drawn", async ({
    page,
  }) => {
    await startThread(page, "cards-bad now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText(/Interface not shown:/)).toBeVisible();
    await expect(log.getByText(/component "options" \(Cards\) cards\.0/)).toBeVisible();
    await expect(log.locator('[data-rule="schema"]')).toHaveCount(1);
    // not half drawn: the title of the surface is in the raw operations only
    await expect(log.getByRole("region", { name: /^Interface from/ })).toHaveCount(0);
    await expect(log.locator('[data-slot="cards"]')).toHaveCount(0);
    await log.getByText("Raw operations").click();
    await expect(log.locator("details pre")).toContainText('"component": "Cards"');
    await expect(log.locator("details pre")).toContainText("A card needs a title");
    await expect(log.locator('a[href^="javascript" i]')).toHaveCount(0);
  });

  test("a link that passes the schema but not ADR 0013's rule (user information) is refused by the URL rule", async ({
    page,
  }) => {
    await startThread(page, "cards-bad-url now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const log = conversation(page);
    await expect(log.getByText(/Interface not shown:/)).toBeVisible();
    await expect(log.locator("[data-rule]")).toHaveAttribute("data-rule", "url");
    await expect(
      log.getByText(
        /component "options" \(Cards\) names, in card 1, a URL that is not an absolute http\(s\) URL/,
      ),
    ).toBeVisible();
    await expect(log.getByRole("link", { name: /Look here/ })).toHaveCount(0);
    await expect(log.locator('a[href*="evil.example"]')).toHaveCount(0);
    await expect(log.locator('[data-slot="cards"]')).toHaveCount(0);
  });
});

test.describe("a graph that does not parse", () => {
  test("says so with the reason and the source, and the cards beside it are drawn", async ({
    page,
  }) => {
    await startThread(page, "mermaid-bad now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const surface = ui(page);
    await expect(surface.getByText("The graph could not be drawn.")).toBeVisible();
    // mermaid's own words, and the source it could not read
    await expect(surface.locator('[data-slot="mermaid-error"]')).toContainText(/\S/);
    await expect(surface.locator('[data-slot="mermaid-code"]')).toContainText("flowchart TD");
    await expect(surface.locator('[data-slot="mermaid-code"]')).toContainText("B{{ not a graph");
    await expect(surface.locator('[data-slot="mermaid-image"]')).toHaveCount(0);
    // it is the graph's failure: the cards of the same surface are there, and it is not a refusal
    await expect(surface.getByRole("list").getByRole("listitem")).toHaveCount(1);
    await expect(conversation(page).getByText(/Interface not shown/)).toHaveCount(0);
    // mermaid leaves nothing of its own error in the page
    await expect(page.locator('[id^="dmmd-"], [id^="mmd-"]')).toHaveCount(0);
    await expect(page.getByText("Syntax error in text")).toHaveCount(0);
  });
});

test.describe("a graph written to get out of the picture", () => {
  test("is drawn as a plain image: no script, no handler, no link, no request", async ({
    page,
  }) => {
    const urls = requested(page);
    await page.addInitScript(() => {
      const w = window as unknown as Record<string, unknown>;
      w.__mermaidFlagCallback = () => {
        w.__mermaidFlag = "callback";
      };
    });
    await startThread(page, "mermaid-hostile now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    await expectDecoded(page);
    // give a handler or a callback every chance to run
    const img = page.locator('[data-slot="mermaid-image"]');
    await img.click({ position: { x: 10, y: 10 } });
    await img.click({ position: { x: 60, y: 40 } });
    await page.waitForTimeout(500);

    const flag = await page.evaluate(
      () => (window as unknown as Record<string, unknown>).__mermaidFlag,
    );
    expect(flag).toBeUndefined();

    const svg = await drawnSvg(page);
    expect(svg).not.toMatch(/<script/i);
    expect(svg).not.toMatch(/<foreignObject/i);
    expect(svg).not.toMatch(/\son[a-z]+\s*=/i);
    expect(svg).not.toContain("<img");
    // the labels that were HTML are text now, if drawn at all
    expect(svg.replace(/<style>[\s\S]*?<\/style>/g, "").replace(/<[^>]+>/g, "")).toContain(
      "Plain end",
    );
    // the page itself has no link, no script and no extra element from the graph
    const surface = ui(page);
    await expect(surface.locator("a, script, iframe, object, embed")).toHaveCount(0);
    expect(urls.filter((u) => u.includes("example.com"))).toEqual([]);
    expect(urls.filter((u) => !/^(http:\/\/127\.0\.0\.1:(3000|4010)\/|data:)/.test(u))).toEqual([]);
  });
});

test.describe("the kinds of graph an agent writes", () => {
  test("ten kinds, each drawn as a picture, none refused", async ({ page }) => {
    test.setTimeout(90_000);
    await startThread(page, "mermaid-kinds now", "Reviewer");
    await expect(badge(page)).toHaveText("Done");
    const surface = ui(page);
    const images = surface.locator('[data-slot="mermaid-image"]');
    await expect(images).toHaveCount(10, { timeout: 60_000 });
    await expect(surface.locator('[data-slot="mermaid-error"]')).toHaveCount(0);
    // every one is decoded by the browser, with a size of its own
    const sizes = await images.evaluateAll((all) =>
      all.map((img) => [
        (img as HTMLImageElement).naturalWidth,
        (img as HTMLImageElement).naturalHeight,
      ]),
    );
    for (const [w, h] of sizes) {
      expect(w).toBeGreaterThan(40);
      expect(h).toBeGreaterThan(40);
    }
    await expectNoHorizontalScroll(page);
  });
});
