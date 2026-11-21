// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { ThreadViewProvider } from "@/features/chat/components/thread-view";
import { SessionsProvider } from "@/features/sessions/components/sessions-provider";
import { CHILD_SESSIONS_URI } from "@/features/sessions/lib/offer";
import type { ApiThread } from "@/lib/api/types";
import { PanelProvider } from "../hooks/use-panel";
import { PanelToggle } from "./panel-toggle";
import { ThreadPanel } from "./thread-panel";

// the other tabs have tests of their own (thread-panel.dom.test.tsx)
vi.mock("../hooks/use-sources", () => ({ useSources: () => [], useTurnsHeld: () => 0 }));
vi.mock("../hooks/use-plan", () => ({ useThreadPlan: () => undefined }));
vi.mock("@/features/chat/components/steps/steps-panel-content", () => ({
  StepsPanelContent: () => <p>What the agents do shows up here</p>,
}));

const PARENT = "00000000-0000-4000-8000-0000000000f0";
const OFFER = { agents: [{ name: "planner", description: "Plans." }], maxOpen: 4 };
const caps = (entry: unknown): AgentCapabilities => ({
  status: "ready",
  supports: () => true,
  entry: (uri) => (uri === CHILD_SESSIONS_URI ? entry : undefined),
  refresh: () => {},
});
const child = (n: number, state: string) =>
  ({
    id: `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
    title: `Task ${n}`,
    state: "done",
    target: { agentId: "adam" },
    childOf: { threadId: PARENT, session: `s${n}`, agent: "planner", state, keepOpen: true },
  }) as unknown as ApiThread;

beforeEach(() => {
  window.localStorage.clear();
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function mount(
  entry: unknown,
  rows: ApiThread[] = [],
  wrap: (n: ReactNode) => ReactNode = (n) => n,
) {
  return render(
    <ThreadViewProvider value={{ state: "done", waiting: false, agentId: "adam" }}>
      <SessionsProvider
        threadId={PARENT}
        thread={null}
        rows={rows}
        capabilities={caps(entry)}
        tier={undefined}
        readOnly={false}
        refreshRows={() => {}}
      >
        <PanelProvider>
          {wrap(<PanelToggle />)}
          <ThreadPanel />
        </PanelProvider>
      </SessionsProvider>
    </ThreadViewProvider>,
  );
}

const tabs = () =>
  within(screen.getByRole("tablist", { name: "Sections" }))
    .getAllByRole("tab")
    .map((t) => t.textContent);

describe("the Subagents tab of the right-hand panel", () => {
  it("is there while the agent offers subagents, after Activity and Sources, and holds the list", () => {
    mount(OFFER);
    expect(tabs()).toEqual(["Activity", "Sources", "Subagents"]);
    fireEvent.mouseDown(screen.getByRole("tab", { name: "Subagents" }), { button: 0 });
    fireEvent.click(screen.getByRole("tab", { name: "Subagents" }));
    const panel = screen.getByRole("tabpanel");
    expect(panel.textContent).toContain("No subagents yet");
    expect(within(panel).getByRole("button", { name: "New subagent" })).toBeTruthy();
  });

  it("says how many are open, as the other tabs say their number", () => {
    mount(OFFER, [child(1, "idle"), child(2, "working"), child(3, "closed")]);
    expect(tabs()).toEqual(["Activity", "Sources", "Subagents2"]);
  });

  it("is there for a thread that has subagents when the agent no longer offers them", () => {
    mount(null, [child(1, "closed")]);
    expect(tabs()).toEqual(["Activity", "Sources", "Subagents"]);
  });

  it("is not there for an agent that offers none and a thread that has none", () => {
    mount(null);
    expect(tabs()).toEqual(["Activity", "Sources"]);
  });

  it("falls back to Activity when the tab a person left open is gone", () => {
    window.localStorage.setItem("chat.panel.tab", "sessions");
    mount(null);
    expect(screen.getByRole("tab", { name: "Activity" }).getAttribute("aria-selected")).toBe(
      "true",
    );
  });

  it("is a sheet on a phone as every tab is, with Subagents in it", async () => {
    Object.defineProperty(window, "innerWidth", { value: 390, configurable: true, writable: true });
    mount(OFFER);
    fireEvent.click(screen.getByRole("button", { name: "Thread details" }));
    const sheet = await screen.findByRole("dialog");
    expect(within(sheet).getByRole("tab", { name: "Subagents" })).toBeTruthy();
  });
});
