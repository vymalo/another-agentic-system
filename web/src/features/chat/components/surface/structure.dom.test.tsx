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
 * Catalog version 6, group 2 (structure and status), drawn by the real renderers through the runtime:
 * Tabs and Accordion hold their children; Callout, Checklist and Timeline say their states in words.
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
  { id: "root", component: "Column", children: ["tabs", "acc", "note", "list", "line"] },
  {
    id: "tabs",
    component: "Tabs",
    tabs: [
      { label: "Overview", child: "first" },
      { label: "Details", child: "second" },
    ],
  },
  { id: "first", component: "Text", text: "First body" },
  { id: "second", component: "Text", text: "Second body" },
  { id: "acc", component: "Accordion", items: [{ title: "Setup", child: "install" }] },
  { id: "install", component: "Text", text: "Install the packages" },
  {
    id: "note",
    component: "Callout",
    tone: "error",
    title: "Build failed",
    text: "The tests did not run.",
  },
  {
    id: "list",
    component: "Checklist",
    title: "Release",
    items: [
      { label: "Write the tests", state: "done" },
      { label: "Review", state: "doing" },
      { label: "Ship", state: "todo" },
      { label: "Notify", state: "failed" },
    ],
  },
  {
    id: "line",
    component: "Timeline",
    events: [
      { time: "09:00", title: "Started", description: "The run began." },
      { time: "09:05", title: "Stopped", state: "failed" },
    ],
  },
];

describe("Tabs", () => {
  it("shows the first tab's child, and the one the person picks, by a press or the arrow keys", async () => {
    const m = await mountSurface(SHELL);
    expect(screen.getByText("First body")).toBeTruthy();
    expect(screen.queryByText("Second body")).toBeNull();
    const [overview, details] = screen.getAllByRole("tab");
    expect(overview?.getAttribute("aria-selected")).toBe("true");
    fireEvent.mouseDown(details as HTMLElement, { button: 0 });
    expect(await screen.findByText("Second body")).toBeTruthy();
    expect(screen.queryByText("First body")).toBeNull();
    fireEvent.keyDown(details as HTMLElement, { key: "ArrowLeft" });
    expect(await screen.findByText("First body")).toBeTruthy();
    m.agent.stop();
  });
});

describe("Accordion", () => {
  it("is closed until its title is pressed, and says so", async () => {
    const m = await mountSurface(SHELL);
    const title = screen.getByRole("button", { name: "Setup" });
    expect(title.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Install the packages")).toBeNull();
    fireEvent.click(title);
    expect(await screen.findByText("Install the packages")).toBeTruthy();
    expect(title.getAttribute("aria-expanded")).toBe("true");
    m.agent.stop();
  });
});

describe("Callout", () => {
  it("says its tone in words, beside its icon, with its title and text", async () => {
    const m = await mountSurface(SHELL);
    const note = document.querySelector('[data-slot="callout"][data-tone="error"]') as HTMLElement;
    expect(note).toBeTruthy();
    expect(within(note).getByText("Error")).toBeTruthy();
    expect(within(note).getByText("Build failed")).toBeTruthy();
    expect(within(note).getByText("The tests did not run.")).toBeTruthy();
    m.agent.stop();
  });
});

describe("Checklist", () => {
  it("counts the done items, has a progress bar, and says each state in words", async () => {
    const m = await mountSurface(SHELL);
    expect(screen.getByText("1 of 4 done")).toBeTruthy();
    const bar = screen.getByRole("progressbar", { name: "Progress" });
    expect(bar.getAttribute("aria-valuenow")).toBe("1");
    expect(bar.getAttribute("aria-valuemax")).toBe("4");
    const list = screen.getByRole("list", { name: "Release" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((r) => r.getAttribute("data-state"))).toEqual([
      "done",
      "doing",
      "todo",
      "failed",
    ]);
    expect(within(rows[1] as HTMLElement).getByText("In progress")).toBeTruthy();
    expect(within(rows[3] as HTMLElement).getByText("Failed")).toBeTruthy();
    m.agent.stop();
  });
});

describe("Timeline", () => {
  it("lists each event with its time, its title, its description and its state in words", async () => {
    const m = await mountSurface(SHELL);
    const events = within(screen.getByRole("list", { name: "Timeline" })).getAllByRole("listitem");
    expect(events).toHaveLength(2);
    expect(within(events[0] as HTMLElement).getByText("09:00")).toBeTruthy();
    expect(within(events[0] as HTMLElement).getByText("Started")).toBeTruthy();
    expect(within(events[0] as HTMLElement).getByText("The run began.")).toBeTruthy();
    expect(within(events[0] as HTMLElement).getByText("Done")).toBeTruthy();
    expect(within(events[1] as HTMLElement).getByText("Failed")).toBeTruthy();
    m.agent.stop();
  });
});
