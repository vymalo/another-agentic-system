// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { prepareSurface } from "@/features/chat/lib/a2ui/prepare";
import { PanelProvider } from "../hooks/use-panel";
import type { ThreadPlan } from "../hooks/use-plan";
import { PanelToggle } from "./panel-toggle";
import { ThreadPanel } from "./thread-panel";

/*
 * The Plan tab (docs/api/ui-catalog-v1.md, "The plan"; web/DESIGN.md, "Panel"): a third tab beside Activity and
 * Sources that exists only while the thread has a plan, with its progress after its name, and that shows the
 * plan's newest copy. The tab is not remembered into a thread that has no plan.
 */

let plan: ThreadPlan | undefined;
vi.mock("../hooks/use-plan", () => ({ useThreadPlan: () => plan }));
vi.mock("../hooks/use-sources", () => ({ useSources: () => [], useTurnsHeld: () => 0 }));
vi.mock("@/features/chat/hooks/use-thread-files", () => ({ useThreadFilesList: () => [] }));
vi.mock("@/features/chat/components/steps/steps-panel-content", () => ({
  StepsPanelContent: () => <p>What the agents do shows up here</p>,
}));

const planOf = (items: { label: string; state: string; note?: string }[]): ThreadPlan => {
  const operations = [
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "plan-1", catalogId: OWN_CATALOG.catalogId },
    },
    {
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "plan-1",
        components: [
          { id: "root", component: "Column", children: ["steps"] },
          { id: "steps", component: "Checklist", title: "Fix the parser", items },
        ],
      },
    },
  ];
  return {
    prepared: prepareSurface(operations, { threadVersion: 8 }),
    operations,
    actor: { type: "agent", name: "Coder" },
  };
};

beforeEach(() => {
  plan = undefined;
  window.localStorage.clear();
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

const mount = () =>
  render(
    <PanelProvider>
      <PanelToggle />
      <ThreadPanel />
    </PanelProvider>,
  );
const tabs = () =>
  within(screen.getByRole("tablist", { name: "Sections" }))
    .getAllByRole("tab")
    .map((t) => t.textContent);

describe("the Plan tab", () => {
  it("is not there while the thread has no plan", () => {
    mount();
    expect(tabs()).toEqual(["Activity", "Sources"]);
  });

  it("is the third tab with the plan's progress, dropped items counted out", () => {
    plan = planOf([
      { label: "Read the test", state: "done" },
      { label: "Fix it", state: "doing" },
      { label: "Port it", state: "dropped", note: "Out of scope" },
      { label: "Open the pull request", state: "todo" },
    ]);
    mount();
    expect(tabs()).toEqual(["Activity", "Sources", "Plan1/3"]);
  });

  it("says no progress in its name when every item is dropped: nothing is left to count", () => {
    plan = planOf([
      { label: "Port it", state: "dropped" },
      { label: "Tag it", state: "dropped" },
    ]);
    mount();
    expect(tabs()).toEqual(["Activity", "Sources", "Plan"]);
  });

  it("shows the plan's checklist, its dropped item in words, and its note", async () => {
    plan = planOf([
      { label: "Read the test", state: "done" },
      { label: "Port it", state: "dropped", note: "Out of scope" },
    ]);
    window.localStorage.setItem("chat.panel.tab", "plan");
    mount();
    const panel = screen.getByRole("tabpanel");
    expect(screen.getByRole("tab", { name: /^Plan/ }).getAttribute("aria-selected")).toBe("true");
    expect(panel.textContent).toContain("Fix the parser");
    expect(panel.textContent).toContain("1 of 1 done");
    expect(panel.textContent).toContain("Dropped");
    expect(panel.textContent).toContain("Out of scope");
    expect(within(panel).getAllByRole("listitem")).toHaveLength(2);
  });

  it("a remembered Plan tab opens on Activity in a thread that has no plan", () => {
    window.localStorage.setItem("chat.panel.tab", "plan");
    mount();
    expect(screen.getByRole("tab", { name: "Activity" }).getAttribute("aria-selected")).toBe(
      "true",
    );
    expect(screen.getByRole("tabpanel").textContent).toContain("What the agents do shows up here");
  });

  it("a plan that is refused says so in the tab", () => {
    plan = planOf([{ label: "x", state: "skipped" }]);
    window.localStorage.setItem("chat.panel.tab", "plan");
    mount();
    expect(screen.getByRole("tabpanel").textContent).toContain("Interface not shown");
  });
});
