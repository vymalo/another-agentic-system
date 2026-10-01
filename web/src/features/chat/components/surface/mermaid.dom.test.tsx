// @vitest-environment jsdom
import {
  act,
  cleanup,
  configure,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { renderMermaid } from "@/features/chat/lib/a2ui/mermaid-render";
import { surface } from "@/features/chat/lib/a2ui/testing";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

// mermaid draws with layout, which jsdom has not: it is stood in for here (its security options
// are tested against the real library in lib/a2ui/mermaid.security.test.ts, and the drawing in a
// browser in e2e/cards.spec.ts)
vi.mock("@/features/chat/lib/a2ui/mermaid-render", () => ({ renderMermaid: vi.fn() }));
const draw = vi.mocked(renderMermaid);

const SVG =
  '<svg id="mmd-1" width="100%" xmlns="http://www.w3.org/2000/svg" style="max-width: 345px" viewBox="0 0 345 120"><g/></svg>';

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(() => {
  resetSeq();
  draw.mockReset();
  draw.mockResolvedValue(SVG);
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
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

const CODE = "graph TD; A-->B";

const mermaidSurface = (extra: Rec = {}) =>
  surface(
    [
      { id: "root", component: "Column", children: ["intro", "flow"] },
      { id: "intro", component: "Text", text: "How it flows" },
      { id: "flow", component: "Mermaid", code: CODE, ...extra },
    ],
    undefined,
    "v0.9.1",
    OWN_CATALOG.catalogId,
  );

async function mountMermaid(extra: Rec = {}) {
  const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([mermaidSurface(extra)]));
  return m;
}

/** The configuration mermaid was given in call number `n`. */
const configOf = (n: number) => draw.mock.calls[n]?.[1] as { themeVariables: Rec } & Rec;

const image = () =>
  document.querySelector('[data-slot="mermaid-image"]') as HTMLImageElement | null;
const source = () => document.querySelector('[data-slot="mermaid-source"]') as HTMLDetailsElement;

/** A `matchMedia` that says the scheme is `dark`, and can change its mind. */
function stubScheme(initial: "light" | "dark") {
  let dark = initial === "dark";
  const listeners = new Set<() => void>();
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return query.includes("dark") && dark;
    },
    media: query,
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  }));
  return (next: "light" | "dark") => {
    dark = next === "dark";
    for (const l of listeners) l();
  };
}

describe("the Mermaid component", () => {
  it("draws the graph as an image of a data URL, and asks mermaid once, with the source and a strict configuration", async () => {
    const m = await mountMermaid({ title: "Flow", caption: "A goes to B" });
    await waitFor(() => expect(image()).not.toBeNull());
    expect(draw).toHaveBeenCalledTimes(1);
    const [code, config] = draw.mock.calls[0] as [string, Rec];
    expect(code).toBe(CODE);
    expect(config).toMatchObject({
      securityLevel: "strict",
      htmlLabels: false,
      startOnLoad: false,
      theme: "base",
      darkMode: false,
    });
    const img = image() as HTMLImageElement;
    expect(img.getAttribute("src")?.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
    expect(img.getAttribute("alt")).toBe("Flow");
    expect(img.getAttribute("width")).toBe("345");
    expect(img.getAttribute("height")).toBe("120");
    expect(screen.getByRole("img", { name: "Flow" })).toBe(img);
    // the title is a heading, the caption is the figure's
    expect(screen.getByRole("heading", { name: "Flow" })).toBeTruthy();
    expect(document.querySelector("figcaption")?.textContent).toBe("A goes to B");
    expect(screen.getByText("How it flows")).toBeTruthy();
    // the SVG is not markup of the page: nothing but the image is in the DOM
    expect(
      screen.getByRole("region", { name: /^Interface from / }).querySelector("svg"),
    ).toBeNull();
    m.agent.stop();
  });

  it("the image's text alternative is the title, else the caption, else 'Diagram'", async () => {
    for (const [extra, alt] of [
      [{ title: "T", caption: "C" }, "T"],
      [{ caption: "C" }, "C"],
      [{}, "Diagram"],
    ] as const) {
      const m = await mountMermaid(extra);
      await waitFor(() => expect(image()).not.toBeNull());
      expect(image()?.getAttribute("alt")).toBe(alt);
      m.agent.stop();
      cleanup();
      resetSeq();
    }
  });

  it("keeps the source, which is text, one click away: a disclosure, closed", async () => {
    const m = await mountMermaid();
    await waitFor(() => expect(image()).not.toBeNull());
    expect(source().open).toBe(false);
    expect(within(source()).getByText("Diagram source")).toBeTruthy();
    expect(source().querySelector("pre")?.textContent).toBe(CODE);
    expect(source().querySelector("pre")?.getAttribute("tabindex")).toBe("0");
    m.agent.stop();
  });

  it("says it is drawing until mermaid is ready, and never shows a picture before", async () => {
    let finish: (svg: string) => void = () => {};
    draw.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    const m = await mountMermaid();
    expect(screen.getByText("Drawing the graph…")).toBeTruthy();
    expect(image()).toBeNull();
    // the source is there while it draws
    expect(source().querySelector("pre")?.textContent).toBe(CODE);
    await act(async () => finish(SVG));
    await waitFor(() => expect(image()).not.toBeNull());
    expect(screen.queryByText("Drawing the graph…")).toBeNull();
    m.agent.stop();
  });

  it("a graph that does not parse says so, with mermaid's reason and the source in place; the rest of the surface stays", async () => {
    draw.mockRejectedValue(
      new Error("Parse error on line 2:\n  A[Start] -->\n---^\nExpecting 'SEMI'"),
    );
    const m = await mountMermaid({ title: "Bad one" });
    await waitFor(() => expect(screen.getByText("The graph could not be drawn.")).toBeTruthy());
    expect(screen.getByText(/Parse error on line 2/)).toBeTruthy();
    expect(document.querySelector('[data-slot="mermaid-code"]')?.textContent).toBe(CODE);
    expect(image()).toBeNull();
    expect(source()).toBeNull();
    // the person is not told off by an alert, a replay must not announce it again
    expect(screen.queryByRole("alert")).toBeNull();
    // it is the graph's failure, not the surface's: the words and the title are drawn
    expect(screen.getByText("How it flows")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Bad one" })).toBeTruthy();
    expect(screen.queryByText(/Interface not shown/)).toBeNull();
    m.agent.stop();
  });

  it("a library that cannot be loaded is the same failure, with the source", async () => {
    draw.mockRejectedValue(new TypeError("Failed to fetch dynamically imported module"));
    const m = await mountMermaid();
    await waitFor(() => expect(screen.getByText("The graph could not be drawn.")).toBeTruthy());
    expect(screen.getByText(/Failed to fetch dynamically imported module/)).toBeTruthy();
    expect(document.querySelector('[data-slot="mermaid-code"]')?.textContent).toBe(CODE);
    m.agent.stop();
  });

  it("a drawing that is not a picture this page may show (a script in it) is not shown", async () => {
    draw.mockResolvedValue(SVG.replace("<g/>", "<script>window.__ran = 1</script>"));
    const m = await mountMermaid();
    await waitFor(() => expect(screen.getByText("The graph could not be drawn.")).toBeTruthy());
    expect(image()).toBeNull();
    expect(document.querySelector("script")).toBeNull();
    expect((window as unknown as Rec).__ran).toBeUndefined();
    m.agent.stop();
  });

  it("an image the browser cannot show falls back to the failure, with the source", async () => {
    const m = await mountMermaid();
    await waitFor(() => expect(image()).not.toBeNull());
    fireEvent.error(image() as HTMLImageElement);
    await waitFor(() => expect(screen.getByText("The graph could not be drawn.")).toBeTruthy());
    expect(screen.getByText(/the browser could not show the drawing/)).toBeTruthy();
    m.agent.stop();
  });

  it("is drawn in the colours of the scheme, and again when the scheme changes, keeping the picture meanwhile", async () => {
    const setScheme = stubScheme("dark");
    const m = await mountMermaid();
    await waitFor(() => expect(image()).not.toBeNull());
    expect(draw).toHaveBeenCalledTimes(1);
    expect(configOf(0)).toMatchObject({ darkMode: true });
    // the page's own tokens are read when drawing; jsdom has none, so the dark palette stands in
    expect(configOf(0).themeVariables.background).toBe("#1a1d1a");

    let finish: (svg: string) => void = () => {};
    draw.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    act(() => setScheme("light"));
    await waitFor(() => expect(draw).toHaveBeenCalledTimes(2));
    expect(configOf(1)).toMatchObject({ darkMode: false });
    expect(configOf(1).themeVariables.background).toBe("#ffffff");
    // the old picture stays until the new one is ready
    expect(image()).not.toBeNull();
    await act(async () => finish(SVG));
    expect(image()).not.toBeNull();
    m.agent.stop();
  });

  it("a later copy of the surface with another graph draws again, and shows nothing of the old one meanwhile", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    await feed(m, surfaceRun([mermaidSurface()], { end: "success" }));
    await waitFor(() => expect(image()).not.toBeNull());
    expect(draw).toHaveBeenCalledTimes(1);
    let finish: (svg: string) => void = () => {};
    draw.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    // the same surface, replaced in place by the agent, with another graph
    const next = surface(
      [
        { id: "root", component: "Column", children: ["flow"] },
        { id: "flow", component: "Mermaid", code: "graph LR; X-->Y" },
      ],
      undefined,
      "v0.9.1",
      OWN_CATALOG.catalogId,
    );
    await feed(m, surfaceRun([next], { runId: "run-2", messageId: "a2ui-3", user: false }));
    await waitFor(() => expect(draw).toHaveBeenCalledTimes(2));
    expect(draw.mock.calls[1]?.[0]).toBe("graph LR; X-->Y");
    expect(image()).toBeNull();
    await act(async () => finish(SVG));
    await waitFor(() => expect(image()).not.toBeNull());
    m.agent.stop();
  });

  it("a graph under the basic catalog is refused before anything is drawn", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    await feed(
      m,
      surfaceRun([
        surface([
          { id: "root", component: "Column", children: ["flow"] },
          { id: "flow", component: "Mermaid", code: CODE },
        ]),
      ]),
    );
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(draw).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("a graph over the schema's limit is refused before anything is drawn, with the raw operations", async () => {
    const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
    await feed(m, surfaceRun([mermaidSurface({ code: "g".repeat(20_001) })]));
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(document.querySelector("[data-rule]")?.getAttribute("data-rule")).toBe("schema");
    expect(screen.getByText(/component "flow" \(Mermaid\) code:/)).toBeTruthy();
    expect(draw).not.toHaveBeenCalled();
    m.agent.stop();
  });
});
