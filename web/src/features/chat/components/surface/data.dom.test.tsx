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
import { surface } from "@/features/chat/lib/a2ui/testing";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

/**
 * Catalog version 6, group 3 (data), drawn by the real renderers through the runtime: a Table that sorts by its
 * headers, a KeyValue, Stats with their trends in words, and a bar and a pie chart with their names and data.
 */

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
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

const drawn = (components: Rec[]) =>
  surface(components, undefined, "v0.9.1", OWN_CATALOG.catalogId);

async function mountSurface(components: Rec[]) {
  const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([drawn(components)]));
  return m;
}

const SHELL: Rec[] = [
  { id: "root", component: "Column", children: ["t", "kv", "s", "bars", "pie"] },
  {
    id: "t",
    component: "Table",
    title: "Services",
    columns: [
      { key: "name", label: "Name" },
      { key: "count", label: "Replicas", align: "end" },
      { key: "live", label: "Live" },
    ],
    rows: [
      { name: "web", count: 10, live: true },
      { name: "api", count: 2, live: false },
      { name: "worker", count: 33 },
    ],
  },
  {
    id: "kv",
    component: "KeyValue",
    title: "Deployment",
    pairs: [
      { key: "Region", value: "eu-west" },
      { key: "Public", value: false },
      { key: "Owner", value: null },
    ],
  },
  {
    id: "s",
    component: "Stats",
    tiles: [
      { label: "Users", value: "1,204", delta: "+12%", trend: "up" },
      { label: "Errors", value: "3", delta: "−1", trend: "down" },
      { label: "Uptime", value: "99.9%", trend: "flat" },
    ],
  },
  {
    id: "bars",
    component: "Chart",
    type: "bar",
    title: "Runs",
    labels: ["Mon", "Tue"],
    series: [
      { name: "Passed", values: [3, 5] },
      { name: "Failed", values: [1, 0] },
    ],
  },
  {
    id: "pie",
    component: "Chart",
    type: "pie",
    title: "Share",
    labels: ["Web", "API"],
    series: [{ name: "Traffic", values: [2, 3] }],
  },
];

/** The first column's words, top to bottom, in the rows of the table. */
const firstColumn = () =>
  Array.from(document.querySelectorAll('[data-slot="table"] tbody tr')).map(
    (tr) => tr.querySelector("td")?.textContent,
  );

describe("Table", () => {
  it("is a named, keyboard-reachable region with its rows as they came, and its cells in words", async () => {
    const m = await mountSurface(SHELL);
    const region = screen.getByRole("region", { name: "Services" });
    expect(region.getAttribute("tabindex")).toBe("0");
    expect(firstColumn()).toEqual(["web", "api", "worker"]);
    expect(within(region).getByRole("cell", { name: "Yes" })).toBeTruthy();
    expect(within(region).getByRole("cell", { name: "No" })).toBeTruthy();
    m.agent.stop();
  });

  it("sorts by a column's header: ascending, then descending, then as it came, and says so", async () => {
    const m = await mountSurface(SHELL);
    const header = screen.getByRole("columnheader", { name: /Replicas/ });
    expect(header.getAttribute("aria-sort")).toBe("none");
    fireEvent.click(within(header).getByRole("button"));
    expect(header.getAttribute("aria-sort")).toBe("ascending");
    expect(firstColumn()).toEqual(["api", "web", "worker"]);
    fireEvent.click(within(header).getByRole("button"));
    expect(header.getAttribute("aria-sort")).toBe("descending");
    expect(firstColumn()).toEqual(["worker", "web", "api"]);
    fireEvent.click(within(header).getByRole("button"));
    expect(firstColumn()).toEqual(["web", "api", "worker"]);
    m.agent.stop();
  });
});

describe("KeyValue", () => {
  it("lists each pair as a term and its value, a yes or no in words, and a value that is not set so", async () => {
    const m = await mountSurface(SHELL);
    const list = document.querySelector('[data-slot="key-value"] dl') as HTMLElement;
    expect(within(list).getByText("Region")).toBeTruthy();
    expect(within(list).getByText("eu-west")).toBeTruthy();
    expect(within(list).getByText("No")).toBeTruthy();
    expect(within(list).getByText("Not set")).toBeTruthy();
    m.agent.stop();
  });
});

describe("Stats", () => {
  it("shows each tile's label and value, its change, and its trend in words", async () => {
    const m = await mountSurface(SHELL);
    const tiles = within(screen.getByRole("list", { name: "Figures" })).getAllByRole("listitem");
    expect(tiles).toHaveLength(3);
    expect(within(tiles[0] as HTMLElement).getByText("1,204")).toBeTruthy();
    expect(within(tiles[0] as HTMLElement).getByText("+12%")).toBeTruthy();
    expect(within(tiles[0] as HTMLElement).getByText("Up:", { exact: false })).toBeTruthy();
    expect(within(tiles[1] as HTMLElement).getByText("Down:", { exact: false })).toBeTruthy();
    expect(within(tiles[2] as HTMLElement).getByText("Unchanged:", { exact: false })).toBeTruthy();
    m.agent.stop();
  });
});

describe("Chart", () => {
  it("a bar chart is one picture named for what it shows, with a legend and its data in a disclosure", async () => {
    const m = await mountSurface(SHELL);
    const picture = screen.getByRole("img", { name: "Bar chart: Runs. 2 series over 2 points." });
    expect(picture.querySelectorAll("rect").length).toBeGreaterThanOrEqual(4);
    const figure = document.querySelector('[data-slot="chart"][data-type="bar"]') as HTMLElement;
    const legend = within(figure).getByRole("list", { name: "Legend" });
    expect(within(legend).getByText("Passed")).toBeTruthy();
    expect(within(legend).getByText("Failed")).toBeTruthy();
    const data = figure.querySelector("details table") as HTMLElement;
    expect(within(data).getByRole("cell", { name: "5" })).toBeTruthy();
    m.agent.stop();
  });

  it("a pie of six slices draws six colours, one for each slice", async () => {
    const m = await mountSurface([
      { id: "root", component: "Column", children: ["pie"] },
      {
        id: "pie",
        component: "Chart",
        type: "pie",
        labels: ["a", "b", "c", "d", "e", "f"],
        series: [{ name: "Share", values: [1, 2, 3, 4, 5, 6] }],
      },
    ]);
    const picture = screen.getByRole("img", { name: /Pie chart\. 1 series over 6 points/ });
    const fills = [...picture.querySelectorAll("path")].map((p) => (p as SVGElement).style.fill);
    expect(fills).toHaveLength(6);
    expect(new Set(fills).size).toBe(6);
    m.agent.stop();
  });

  it("a pie is named as one, its slices labelled with their share, and its legend names each label", async () => {
    const m = await mountSurface(SHELL);
    const picture = screen.getByRole("img", { name: "Pie chart: Share. 1 series over 2 points." });
    expect(picture.querySelectorAll("path")).toHaveLength(2);
    expect(within(picture).getByText("40%")).toBeTruthy();
    expect(within(picture).getByText("60%")).toBeTruthy();
    const figure = document.querySelector('[data-slot="chart"][data-type="pie"]') as HTMLElement;
    expect(
      within(within(figure).getByRole("list", { name: "Legend" })).getByText("API"),
    ).toBeTruthy();
    m.agent.stop();
  });
});
