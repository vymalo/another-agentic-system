// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PANEL_KEY, PANEL_TAB_KEY, PANEL_WIDTH_KEY } from "../lib/panel-state";
import { PanelProvider, usePanel } from "./use-panel";
import { StepsPanelTestProvider, useStepsExpansion, useStepsPanel } from "./use-steps-panel";

/** jsdom's window is 1024 wide: tests say how wide theirs is. */
function setViewport(width: number) {
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true, writable: true });
}

beforeEach(() => {
  window.localStorage.clear();
  delete document.documentElement.dataset.panel;
  document.documentElement.style.removeProperty("--panel-width");
  setViewport(1440);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function Probe() {
  const panel = usePanel();
  const steps = useStepsPanel();
  if (!panel) return <p>no panel</p>;
  return (
    <div>
      <output data-testid="state">
        {JSON.stringify({
          layout: panel.layout,
          open: panel.open,
          tab: panel.tab,
          width: panel.width,
          stepsOpen: steps.open,
          stepsTab: steps.tab,
          focus: steps.focus,
        })}
      </output>
      <button type="button" ref={panel.toggleRef} onClick={panel.toggle}>
        toggle
      </button>
      <button type="button" onClick={() => panel.setTab("sources")}>
        sources
      </button>
      <button type="button" onClick={() => panel.setWidth(420)}>
        wider
      </button>
      <button type="button" onClick={() => steps.openSteps("turn-a")}>
        open steps a
      </button>
    </div>
  );
}

const state = () =>
  JSON.parse(screen.getByTestId("state").textContent ?? "{}") as {
    layout: string | null;
    open: boolean;
    tab: string;
    width: number;
    stepsOpen: boolean;
    stepsTab: string;
    focus: { turnId: string; key: number } | null;
  };

function mount(extra?: ReactNode) {
  return render(
    <PanelProvider>
      <Probe />
      {extra}
    </PanelProvider>,
  );
}

describe("what the panel starts as", () => {
  it("is open on a wide window when the person never chose, closed on a narrower one", () => {
    mount();
    expect(state()).toMatchObject({ layout: "docked", open: true });
    cleanup();

    setViewport(1200); // docked, but not wide enough to open by itself
    mount();
    expect(state()).toMatchObject({ layout: "docked", open: false });
    cleanup();

    setViewport(1000);
    mount();
    expect(state()).toMatchObject({ layout: "sheet-right", open: false });
    cleanup();

    setViewport(390);
    mount();
    expect(state()).toMatchObject({ layout: "sheet-bottom", open: false });
  });

  it("follows what the browser remembers: open or closed, the tab and the width", () => {
    window.localStorage.setItem(PANEL_KEY, "closed");
    window.localStorage.setItem(PANEL_TAB_KEY, "sources");
    window.localStorage.setItem(PANEL_WIDTH_KEY, "440");
    mount();
    expect(state()).toMatchObject({ open: false, tab: "sources", width: 440 });
    expect(document.documentElement.dataset.panel).toBeUndefined();
    cleanup();

    window.localStorage.setItem(PANEL_KEY, "open");
    setViewport(1200);
    mount();
    expect(state().open).toBe(true);
    expect(document.documentElement.dataset.panel).toBe("open");
  });

  it("ignores what is remembered when it is not one of ours", () => {
    window.localStorage.setItem(PANEL_TAB_KEY, "steps");
    window.localStorage.setItem(PANEL_WIDTH_KEY, "9999");
    mount();
    expect(state()).toMatchObject({ tab: "activity", width: 360 });
  });

  it("a remembered open panel does not open a sheet by itself on a small window", () => {
    window.localStorage.setItem(PANEL_KEY, "open");
    setViewport(390);
    mount();
    expect(state()).toMatchObject({ layout: "sheet-bottom", open: false });
  });

  it("works when storage is blocked: the default applies and toggling does not throw", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("SecurityError");
    });
    mount();
    expect(state().open).toBe(true);
    fireEvent.click(screen.getByText("toggle"));
    expect(state().open).toBe(false);
    fireEvent.click(screen.getByText("sources"));
    expect(state().tab).toBe("sources");
  });
});

describe("toggling", () => {
  it("remembers the choice and marks <html> for the stylesheet", () => {
    mount();
    expect(document.documentElement.dataset.panel).toBe("open");
    fireEvent.click(screen.getByText("toggle"));
    expect(state().open).toBe(false);
    expect(window.localStorage.getItem(PANEL_KEY)).toBe("closed");
    expect(document.documentElement.dataset.panel).toBeUndefined();
    fireEvent.click(screen.getByText("toggle"));
    expect(state().open).toBe(true);
    expect(window.localStorage.getItem(PANEL_KEY)).toBe("open");
    expect(document.documentElement.dataset.panel).toBe("open");
  });

  it("a sheet is this visit only: opening it remembers nothing", () => {
    setViewport(1000);
    mount();
    fireEvent.click(screen.getByText("toggle"));
    expect(state().open).toBe(true);
    expect(window.localStorage.getItem(PANEL_KEY)).toBeNull();
    expect(document.documentElement.dataset.panel).toBeUndefined();
  });

  it("keeps the tab and the width, clamped to the window", () => {
    mount();
    fireEvent.click(screen.getByText("sources"));
    fireEvent.click(screen.getByText("wider"));
    expect(state()).toMatchObject({ tab: "sources", width: 420 });
    expect(window.localStorage.getItem(PANEL_TAB_KEY)).toBe("sources");
    expect(window.localStorage.getItem(PANEL_WIDTH_KEY)).toBe("420");
    // the stylesheet reads the width from here
    expect(document.documentElement.style.getPropertyValue("--panel-width")).toBe("420px");
  });

  it("changes layout with the window: a sheet closed, then docked as it was left", () => {
    mount();
    expect(state()).toMatchObject({ layout: "docked", open: true });
    act(() => {
      setViewport(900);
      window.dispatchEvent(new Event("resize"));
    });
    return vi.waitFor(() => expect(state()).toMatchObject({ layout: "sheet-right", open: false }));
  });
});

describe("the shortcut", () => {
  const press = (init: KeyboardEventInit) =>
    fireEvent.keyDown(window, { code: "Period", key: ".", ...init });

  it("Ctrl+Shift+. and ⌘+Shift+. toggle the panel", () => {
    mount();
    press({ ctrlKey: true, shiftKey: true });
    expect(state().open).toBe(false);
    press({ metaKey: true, shiftKey: true });
    expect(state().open).toBe(true);
  });

  it("goes by the physical key: another layout's character does not matter", () => {
    mount();
    press({ ctrlKey: true, shiftKey: true, key: ":" });
    expect(state().open).toBe(false);
  });

  it("is nothing without Ctrl or ⌘, without Shift, with Alt, on another key, or while composing", () => {
    mount();
    press({ shiftKey: true });
    press({ ctrlKey: true });
    press({ ctrlKey: true, shiftKey: true, altKey: true });
    fireEvent.keyDown(window, { code: "Comma", key: ",", ctrlKey: true, shiftKey: true });
    press({ ctrlKey: true, shiftKey: true, isComposing: true });
    expect(state().open).toBe(true);
  });

  it("leaves a key that something else handled alone", () => {
    mount();
    const claim = (e: KeyboardEvent) => e.preventDefault();
    document.body.addEventListener("keydown", claim);
    fireEvent.keyDown(document.body, { code: "Period", key: ".", ctrlKey: true, shiftKey: true });
    document.body.removeEventListener("keydown", claim);
    expect(state().open).toBe(true);
  });

  it("works from the box the person is typing in", () => {
    mount(<textarea aria-label="Message" />);
    const box = screen.getByLabelText("Message");
    box.focus();
    fireEvent.keyDown(box, { code: "Period", key: ">", ctrlKey: true, shiftKey: true });
    expect(state().open).toBe(false);
  });
});

describe("useStepsPanel: what the step tree needs of the shell", () => {
  it("openSteps opens the panel, selects Activity and asks for the turn, with a new key each time", () => {
    window.localStorage.setItem(PANEL_KEY, "closed");
    window.localStorage.setItem(PANEL_TAB_KEY, "sources");
    mount();
    expect(state()).toMatchObject({ stepsOpen: false, stepsTab: "sources", focus: null });
    fireEvent.click(screen.getByText("open steps a"));
    expect(state()).toMatchObject({
      stepsOpen: true,
      stepsTab: "activity",
      focus: { turnId: "turn-a", key: 1 },
    });
    // the same turn again: a new key, so the pane focuses it again
    fireEvent.click(screen.getByText("open steps a"));
    expect(state().focus).toEqual({ turnId: "turn-a", key: 2 });
  });

  it("asked for, not chosen: it leaves the remembered choice alone", () => {
    window.localStorage.setItem(PANEL_KEY, "closed");
    mount();
    fireEvent.click(screen.getByText("open steps a"));
    expect(window.localStorage.getItem(PANEL_KEY)).toBe("closed");
  });

  it("opens the sheet where the panel is one", () => {
    setViewport(390);
    mount();
    fireEvent.click(screen.getByText("open steps a"));
    expect(state()).toMatchObject({ layout: "sheet-bottom", open: true, stepsOpen: true });
  });

  it("exposes the panel's id, the one the toggle's aria-controls names", () => {
    let id = "";
    const Id = () => {
      id = useStepsPanel().panelId;
      return null;
    };
    mount(<Id />);
    expect(id).toBe("thread-panel");
  });

  it("outside a panel (the new-chat page) it is closed and asking does nothing", () => {
    let steps: ReturnType<typeof useStepsPanel> | undefined;
    const Outside = () => {
      steps = useStepsPanel();
      return null;
    };
    render(<Outside />);
    expect(steps).toMatchObject({
      panelId: "thread-panel",
      open: false,
      tab: "activity",
      focus: null,
    });
    expect(() => steps?.openSteps("x")).not.toThrow();
    expect(steps?.focus).toBeNull();
  });

  it("keeps the tree's expansion above the panel, for the visit", () => {
    const Tree = () => {
      const [expanded, setExpanded] = useStepsExpansion<string[]>([]);
      return (
        <button type="button" onClick={() => setExpanded([...expanded, "turn"])}>
          expanded: {expanded.join(",")}
        </button>
      );
    };
    const view = mount(<Tree />);
    fireEvent.click(screen.getByText("expanded:"));
    expect(screen.getByText("expanded: turn")).toBeTruthy();
    // the tree unmounts (a sheet that closes) and comes back: what was open is still open
    view.rerender(
      <PanelProvider>
        <Probe />
      </PanelProvider>,
    );
    view.rerender(
      <PanelProvider>
        <Probe />
        <Tree />
      </PanelProvider>,
    );
    expect(screen.getByText("expanded: turn")).toBeTruthy();
  });

  it("the test provider is a working panel contract of its own", () => {
    let seen: ReturnType<typeof useStepsPanel> | undefined;
    const Seen = () => {
      seen = useStepsPanel();
      return (
        <button type="button" onClick={() => seen?.openSteps("t1")}>
          go
        </button>
      );
    };
    render(
      <StepsPanelTestProvider>
        <Seen />
      </StepsPanelTestProvider>,
    );
    expect(seen?.open).toBe(false);
    fireEvent.click(screen.getByText("go"));
    expect(seen).toMatchObject({ open: true, focus: { turnId: "t1", key: 1 } });
  });
});
