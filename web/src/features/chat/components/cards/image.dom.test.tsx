// @vitest-environment jsdom
import { cleanup, configure, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { resetSeq, stubLayout } from "../surface/testing";
import { card, goldenWith, HREF, imageSurface, mount, play, SHA } from "./files-testing";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
afterAll(() => vi.restoreAllMocks());

describe("the Image component", () => {
  const image = (over: Record<string, unknown> = {}) => ({
    id: "pic",
    component: "Image",
    artifact: SHA,
    alt: "A chart of the results",
    ...over,
  });
  const withImage = (...parts: Record<string, unknown>[]) =>
    goldenWith([
      imageSurface([
        {
          id: "root",
          component: "Column",
          children: ["intro", ...parts.map((p) => p.id as string)],
        },
        { id: "intro", component: "Text", text: "Here is the chart" },
        ...parts,
      ]),
    ]);

  it("draws a file of the thread by its hash: an <img> of the file's own href, with the agent's alt and caption", async () => {
    const m = mount();
    await play(m, withImage(image({ caption: "Figure 1: results" })));
    const region = await screen.findByRole("region", { name: /^Interface from / });
    const img = within(region).getByRole("img", {
      name: "A chart of the results",
    }) as HTMLImageElement;
    expect(img.getAttribute("src")).toBe(HREF);
    expect(within(region).getByText("Figure 1: results")).toBeTruthy();
    expect(region.querySelector("figure")?.contains(img)).toBe(true);
    // the card of the same file is in the turn as well
    expect(card()).toBeTruthy();
    m.agent.stop();
  });

  it("a hash the thread does not hold refuses the whole surface, and nothing is fetched", async () => {
    const other = "9".repeat(64);
    const fetcher = vi.fn(async () => new Response(""));
    vi.stubGlobal("fetch", fetcher);
    const m = mount();
    await play(m, withImage(image({ artifact: other })));
    await screen.findByText(/Interface not shown:/);
    const refused = document.querySelector('[data-rule="artifact"]') as HTMLElement;
    expect(refused.textContent).toContain("not one of this thread's files");
    expect(screen.queryByText("Here is the chart")).toBeNull();
    expect(document.querySelector(`img[src*="${other}"]`)).toBeNull();
    expect(fetcher).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("a URL in place of a hash is refused by the schema, and loads nothing", async () => {
    const m = mount();
    await play(m, withImage(image({ artifact: "https://evil.example/a.png" })));
    await screen.findByText(/Interface not shown:/);
    expect(document.querySelector('[data-rule="schema"]')).toBeTruthy();
    expect(document.querySelector('img[src*="evil.example"]')).toBeNull();
    m.agent.stop();
  });

  it("the agent's alt and caption are text: markup in them is shown as characters", async () => {
    const html = '<b onclick="alert(1)">bold</b>';
    const m = mount();
    await play(m, withImage(image({ alt: html, caption: html })));
    const region = await screen.findByRole("region", { name: /^Interface from / });
    expect(within(region).getByRole("img", { name: html })).toBeTruthy();
    expect(within(region).getByText(html)).toBeTruthy();
    expect(region.querySelector("b")).toBeNull();
    m.agent.stop();
  });
});
