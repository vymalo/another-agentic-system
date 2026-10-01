// @vitest-environment jsdom
import { act, cleanup, configure, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { surface } from "@/features/chat/lib/a2ui/testing";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

// the graph of a combined answer is drawn by the real component, with mermaid stood in for
vi.mock("@/features/chat/lib/a2ui/mermaid-render", () => ({
  renderMermaid: vi.fn(
    async () =>
      '<svg id="mmd-1" width="100%" xmlns="http://www.w3.org/2000/svg" style="max-width: 100px" viewBox="0 0 100 50"><g/></svg>',
  ),
}));

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Rec = Record<string, unknown>;
type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const CARDS: Rec[] = [
  {
    title: "Server-side sessions",
    subtitle: "Durable",
    body: "One row per session.",
    url: "https://www.postgresql.org/docs/",
    tags: ["durable", "simple"],
  },
  { title: "Signed cookies", url: "http://example.org/cookies" },
  { title: "A cache", body: "No link here." },
];

/** A surface of the web's own catalog: a title and one Cards (id `options`). */
const cardsSurface = (extra: Rec = {}, cards: Rec[] = CARDS) =>
  surface(
    [
      { id: "root", component: "Column", children: ["intro", "options"] },
      { id: "intro", component: "Text", text: "Three ways" },
      { id: "options", component: "Cards", cards, ...extra },
    ],
    undefined,
    "v0.9.1",
    OWN_CATALOG.catalogId,
  );

async function mountCards(ops = cardsSurface()) {
  const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([ops]));
  return m;
}

const items = () => Array.from(document.querySelectorAll('[data-slot="cards-item"]'));

describe("the Cards component", () => {
  it("is a list of cards, each with its own words", async () => {
    const m = await mountCards(cardsSurface({ title: "The options" }));
    expect(screen.getByText("Three ways")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "The options" })).toBeTruthy();
    const list = screen.getByRole("list");
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(within(rows[0] as HTMLElement).getByText("Server-side sessions")).toBeTruthy();
    expect(within(rows[0] as HTMLElement).getByText("Durable")).toBeTruthy();
    expect(within(rows[0] as HTMLElement).getByText("One row per session.")).toBeTruthy();
    expect(
      within(rows[0] as HTMLElement)
        .getAllByText(/^(durable|simple)$/)
        .map((t) => t.textContent),
    ).toEqual(["durable", "simple"]);
    expect(within(rows[2] as HTMLElement).getByText("No link here.")).toBeTruthy();
    expect(list.getAttribute("data-layout")).toBe("list");
    m.agent.stop();
  });

  it("is a grid when it says so", async () => {
    const m = await mountCards(cardsSurface({ layout: "grid" }));
    expect(screen.getByRole("list").getAttribute("data-layout")).toBe("grid");
    expect(items()).toHaveLength(3);
    m.agent.stop();
  });

  it("a link is a plain link to a new tab, with the host it goes to, and only a card that has one", async () => {
    const m = await mountCards();
    const links = screen.getAllByRole("link");
    expect(links).toHaveLength(2);
    const first = links[0] as HTMLAnchorElement;
    expect(first.getAttribute("href")).toBe("https://www.postgresql.org/docs/");
    expect(first.getAttribute("target")).toBe("_blank");
    expect(first.getAttribute("rel")).toBe("noopener noreferrer");
    // its name is the card's title, and says it opens a new tab
    expect(first.textContent).toBe("Server-side sessions (opens in a new tab)");
    expect(screen.getByRole("link", { name: /Signed cookies/ }).getAttribute("href")).toBe(
      "http://example.org/cookies",
    );
    // the person sees where it goes
    expect(within(items()[0] as HTMLElement).getByText("postgresql.org")).toBeTruthy();
    expect(within(items()[1] as HTMLElement).getByText("example.org")).toBeTruthy();
    // a card without a link has none
    expect(within(items()[2] as HTMLElement).queryByRole("link")).toBeNull();
    m.agent.stop();
  });

  it("the agent's words are text: HTML in them is shown as characters", async () => {
    const html = '<img src=x onerror="alert(1)"><script>alert(2)</script>';
    const m = await mountCards(
      cardsSurface({}, [{ title: html, subtitle: html, body: html, tags: ["<b>x</b>"] }]),
    );
    const item = items()[0] as HTMLElement;
    expect(item.textContent).toContain("<script>alert(2)</script>");
    expect(item.textContent).toContain("<b>x</b>");
    expect(item.querySelector("img, script, iframe, a, b")).toBeNull();
    m.agent.stop();
  });

  it("loads nothing: no image, no favicon, no frame, whatever the cards name", async () => {
    const m = await mountCards();
    const region = screen.getByRole("region", { name: /^Interface from / });
    expect(
      region.querySelector("img, picture, video, audio, iframe, object, embed, link"),
    ).toBeNull();
    expect(document.body.innerHTML).not.toContain("favicon");
    m.agent.stop();
  });

  it("sends nothing, and needs no thread that waits: no note that actions are off", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send }, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    await feed(m, surfaceRun([cardsSurface()], { end: "success" }));
    vi.useFakeTimers({ toFake: ["setTimeout", "setInterval", "Date"] });
    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });
    expect(send).not.toHaveBeenCalled();
    expect(screen.queryByText(/actions of this interface/i)).toBeNull();
    expect(screen.queryByText(/request is finished/i)).toBeNull();
    m.agent.stop();
  });

  it("a card with a link that is not http(s) refuses the whole surface: nothing of it is drawn, nothing is a link", async () => {
    for (const url of [
      "javascript:alert(1)",
      "https://user@evil.example/",
      "https://example.com/\u0007",
    ]) {
      const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
      await feed(m, surfaceRun([cardsSurface({}, [{ title: "Fine" }, { title: "Look", url }])]));
      expect(screen.getByText(/Interface not shown:/), url).toBeTruthy();
      expect(document.querySelector("[data-rule]")?.getAttribute("data-rule"), url).toMatch(
        /^(url|schema)$/,
      );
      expect(screen.queryByRole("link")).toBeNull();
      // not half drawn: the first card, which was fine, is not on the page as a card
      expect(items()).toHaveLength(0);
      expect(screen.queryByRole("region", { name: /^Interface from / })).toBeNull();
      m.agent.stop();
      cleanup();
      resetSeq();
    }
  });

  it("a card that breaks the schema is refused visibly: the rule, the reason with the card, and the raw operations", async () => {
    const m = await mountCards(cardsSurface({}, [{ title: "Fine" }, { subtitle: "no title" }]));
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(document.querySelector("[data-rule]")?.getAttribute("data-rule")).toBe("schema");
    expect(screen.getByText(/component "options" \(Cards\) cards\.1:/)).toBeTruthy();
    const raw = document.querySelector("details pre") as HTMLElement;
    expect(raw.textContent).toContain('"no title"');
    expect(raw.getAttribute("tabindex")).toBe("0");
    expect(items()).toHaveLength(0);
    m.agent.stop();
  });

  it("an image is not a property of a card: the surface is refused, not drawn without it", async () => {
    const m = await mountCards(
      cardsSurface({}, [{ title: "A", image: "https://tracker.example/pixel.png" }]),
    );
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
    expect(document.body.innerHTML).not.toContain('tracker.example"');
    m.agent.stop();
  });

  it("a thread whose catalog is newer than ours says it needs a newer app for a component we lack, not for Cards", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version + 1 });
    await feed(m, surfaceRun([cardsSurface()]));
    expect(items()).toHaveLength(3);
    expect(screen.queryByText(/needs a newer version/)).toBeNull();
    m.agent.stop();
  });
});
