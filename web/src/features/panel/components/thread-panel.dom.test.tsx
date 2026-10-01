// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PanelProvider } from "../hooks/use-panel";
import type { SourceGroup } from "../lib/sources";
import { PanelToggle } from "./panel-toggle";
import { ThreadPanel } from "./thread-panel";

let groups: SourceGroup[] = [];
// the panel reads the runtime's messages through this; the sources themselves are sources.test.ts
vi.mock("../hooks/use-sources", () => ({ useSources: () => groups }));
// and the Activity tab reads them too: the step tree has its own tests (chat/components/steps)
vi.mock("@/features/chat/components/steps/steps-panel-content", () => ({
  StepsPanelContent: () => <p>What the agents do shows up here</p>,
}));

const SOURCES: SourceGroup[] = [
  {
    id: "code",
    label: "Pull requests & branches",
    items: [
      {
        key: "https://github.com/acme/demo/pull/12",
        kind: "pull_request",
        title: "acme/demo#12 — Fix the redirect loop",
        detail: "github.com",
        href: "https://github.com/acme/demo/pull/12",
        turns: [{ id: "turn-1", number: 1 }],
      },
    ],
  },
  {
    id: "links",
    label: "Links",
    items: [
      {
        key: "https://docs.rs/axum",
        kind: "link",
        title: "the axum docs",
        detail: "docs.rs",
        href: "https://docs.rs/axum",
        turns: [{ id: "turn-1", number: 1 }],
      },
    ],
  },
];

function setViewport(width: number) {
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true, writable: true });
}

beforeEach(() => {
  groups = [];
  window.localStorage.clear();
  delete document.documentElement.dataset.panel;
  delete document.documentElement.dataset.panelDragging;
  setViewport(1440);
  Element.prototype.scrollIntoView = vi.fn();
  // Radix measures; jsdom has no layout
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

function mount() {
  return render(
    <PanelProvider>
      <header>
        <PanelToggle />
      </header>
      <main>
        <div data-turn-id="turn-1">
          <div data-slot="turn-header" tabIndex={-1}>
            Coder
          </div>
        </div>
      </main>
      <ThreadPanel />
    </PanelProvider>,
  );
}

const toggle = () => screen.getByRole("button", { name: "Thread details" });
const separator = () => screen.getByRole("separator", { name: "Resize the details panel" });

describe("the toggle", () => {
  it("is one button with a stable name, its state in aria-expanded, naming the panel and its shortcut", () => {
    mount();
    const button = toggle();
    expect(button.getAttribute("aria-expanded")).toBe("true"); // 1440 px: open by default
    expect(button.getAttribute("aria-controls")).toBe("thread-panel");
    expect(button.getAttribute("aria-keyshortcuts")).toBe("Control+Shift+Period Meta+Shift+Period");
    fireEvent.click(button);
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(toggle().getAttribute("aria-label")).toBe("Thread details");
  });

  it("is nothing where there is no panel (the new-chat page)", () => {
    render(<PanelToggle />);
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("the docked panel", () => {
  it("is a complementary landmark, 'Thread details', that the toggle opens and closes", () => {
    mount();
    const aside = screen.getByRole("complementary", { name: "Thread details" });
    expect(aside.id).toBe("thread-panel");
    expect(aside.getAttribute("data-state")).toBe("open");
    expect(aside.hasAttribute("inert")).toBe(false);
    fireEvent.click(toggle());
    // closed: still in the page (the toggle's aria-controls stays valid), but out of reach
    const closed = document.getElementById("thread-panel");
    expect(closed?.getAttribute("data-state")).toBe("closed");
    expect(closed?.hasAttribute("inert")).toBe(true);
    expect(screen.queryByRole("complementary", { name: "Thread details" })).toBeNull();
  });

  it("has the tabs Activity and Sources, Activity first, and says how many sources there are", () => {
    groups = SOURCES;
    mount();
    const tabs = within(screen.getByRole("tablist", { name: "Sections" })).getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual(["Activity", "Sources2"]);
    expect(tabs[0]?.getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tabpanel").textContent).toContain("What the agents do shows up here");
  });

  it("moves between the tabs with the arrows and remembers the tab", async () => {
    groups = SOURCES;
    mount();
    const activity = screen.getByRole("tab", { name: "Activity" });
    activity.focus();
    fireEvent.keyDown(activity, { key: "ArrowRight" });
    await waitFor(() =>
      expect(screen.getByRole("tab", { name: /^Sources/ }).getAttribute("aria-selected")).toBe(
        "true",
      ),
    );
    expect(window.localStorage.getItem("chat.panel.tab")).toBe("sources");
    expect(screen.getByRole("heading", { name: "Pull requests & branches" })).toBeTruthy();
  });

  it("closing from its own button puts the focus back on the toggle", () => {
    mount();
    const close = screen.getByRole("button", { name: "Close details" });
    close.focus();
    fireEvent.click(close);
    expect(document.activeElement).toBe(toggle());
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
  });

  it("the shortcut closes it from inside, and the focus lands on the toggle", () => {
    mount();
    const tab = screen.getByRole("tab", { name: "Activity" });
    tab.focus();
    fireEvent.keyDown(tab, { code: "Period", key: ".", ctrlKey: true, shiftKey: true });
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(toggle());
  });
});

describe("the resize handle", () => {
  it("is a focusable vertical separator whose value is the panel's width", () => {
    mount();
    const handle = separator();
    expect(handle.tabIndex).toBe(0);
    expect(handle.getAttribute("aria-orientation")).toBe("vertical");
    expect(handle.getAttribute("aria-valuemin")).toBe("300");
    expect(handle.getAttribute("aria-valuemax")).toBe("560");
    expect(handle.getAttribute("aria-valuenow")).toBe("360");
  });

  it("Left widens by 16 px, Right narrows, Home and End go to the limits, and it stays in them", () => {
    mount();
    const handle = separator();
    const now = () => Number(separator().getAttribute("aria-valuenow"));
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(now()).toBe(376);
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(now()).toBe(344);
    fireEvent.keyDown(handle, { key: "Home" });
    expect(now()).toBe(300);
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(now()).toBe(300);
    fireEvent.keyDown(handle, { key: "End" });
    expect(now()).toBe(560);
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(now()).toBe(560);
    // other keys do nothing, and are not swallowed
    const unhandled = fireEvent.keyDown(handle, { key: "a" });
    expect(unhandled).toBe(true);
  });

  it("remembers the width, and the stylesheet reads it from --panel-width", () => {
    mount();
    fireEvent.keyDown(separator(), { key: "ArrowLeft" });
    expect(window.localStorage.getItem("chat.panel.width")).toBe("376");
    expect(document.documentElement.style.getPropertyValue("--panel-width")).toBe("376px");
  });

  it("the widest it may be leaves the chat 560 px beside the sidebar", () => {
    setViewport(1280);
    mount();
    expect(separator().getAttribute("aria-valuemax")).toBe("448");
    fireEvent.keyDown(separator(), { key: "End" });
    expect(separator().getAttribute("aria-valuenow")).toBe("448");
  });

  it("is dragged with the pointer: left widens, and the width is kept when it is let go", () => {
    mount();
    const handle = separator();
    fireEvent.pointerDown(handle, { pointerId: 1, button: 0, clientX: 1000 });
    expect(document.documentElement.dataset.panelDragging).toBeDefined();
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 950 });
    expect(handle.getAttribute("aria-valuenow")).toBe("410");
    // not remembered at every step
    expect(window.localStorage.getItem("chat.panel.width")).toBeNull();
    fireEvent.pointerMove(handle, { pointerId: 1, clientX: 1100 });
    expect(handle.getAttribute("aria-valuenow")).toBe("300");
    fireEvent.pointerUp(handle, { pointerId: 1 });
    expect(document.documentElement.dataset.panelDragging).toBeUndefined();
    expect(window.localStorage.getItem("chat.panel.width")).toBe("300");
  });
});

describe("the sources tab", () => {
  it("lists each group under a heading, its items as links that open in a new tab", async () => {
    groups = SOURCES;
    mount();
    fireEvent.mouseDown(screen.getByRole("tab", { name: /^Sources/ }));
    screen.getByRole("tab", { name: /^Sources/ }).focus();
    await waitFor(() => expect(screen.getByRole("heading", { name: "Links" })).toBeTruthy());
    const pr = screen.getByRole("link", { name: /^acme\/demo#12 — Fix the redirect loop/ });
    expect(pr.getAttribute("href")).toBe("https://github.com/acme/demo/pull/12");
    expect(pr.getAttribute("target")).toBe("_blank");
    expect(pr.getAttribute("rel")).toBe("noopener noreferrer");
    expect(pr.textContent).toContain("(opens in a new tab)");
    expect(
      within(pr.closest("li") as HTMLElement).getByText("Pull request · github.com"),
    ).toBeTruthy();
  });

  it("has an empty state that says what the tab is for", async () => {
    mount();
    fireEvent.mouseDown(screen.getByRole("tab", { name: "Sources" }));
    screen.getByRole("tab", { name: "Sources" }).focus();
    await waitFor(() => expect(screen.getByText("Nothing shared yet")).toBeTruthy());
    expect(
      screen.getByText("Links, files and pull requests the agents share show up here."),
    ).toBeTruthy();
  });

  it("a Turn button scrolls the chat to the turn and focuses its header, the panel stays open", async () => {
    groups = SOURCES;
    window.localStorage.setItem("chat.panel.tab", "sources");
    mount();
    await waitFor(() => screen.getByRole("heading", { name: "Links" }));
    const [first] = screen.getAllByRole("button", { name: "Show turn 1 in the conversation" });
    expect(first?.textContent).toBe("Turn 1");
    fireEvent.click(first as HTMLElement);
    expect(Element.prototype.scrollIntoView).toHaveBeenCalled();
    expect(document.activeElement?.getAttribute("data-slot")).toBe("turn-header");
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
  });
});

describe("the sheet", () => {
  it("on a tablet is a dialog from the right: opened by the toggle, closed by Escape, focus back", async () => {
    setViewport(1000);
    mount();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    toggle().focus();
    fireEvent.click(toggle());
    const dialog = await screen.findByRole("dialog", { name: "Thread details" });
    expect(dialog.getAttribute("data-side")).toBe("right");
    expect(within(dialog).getByRole("tab", { name: "Activity" })).toBeTruthy();
    expect(dialog.id).toBe("thread-panel");

    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(toggle()));
    // a sheet is this visit only
    expect(window.localStorage.getItem("chat.panel")).toBeNull();
  });

  it("on a phone is a sheet from the bottom, with a close button of its own", async () => {
    setViewport(390);
    mount();
    fireEvent.click(toggle());
    const dialog = await screen.findByRole("dialog", { name: "Thread details" });
    expect(dialog.getAttribute("data-side")).toBe("bottom");
    fireEvent.click(within(dialog).getByRole("button", { name: "Close details" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("a Turn button closes it, and the focus goes to the turn, not back to the toggle", async () => {
    setViewport(390);
    groups = SOURCES;
    window.localStorage.setItem("chat.panel.tab", "sources");
    mount();
    toggle().focus();
    fireEvent.click(toggle());
    const dialog = await screen.findByRole("dialog", { name: "Thread details" });
    const [turn] = within(dialog).getAllByRole("button", {
      name: "Show turn 1 in the conversation",
    });
    fireEvent.click(turn as HTMLElement);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() =>
      expect(document.activeElement?.getAttribute("data-slot")).toBe("turn-header"),
    );
    expect(Element.prototype.scrollIntoView).toHaveBeenCalled();
  });

  it("becomes the docked panel when the window grows", async () => {
    setViewport(900);
    mount();
    fireEvent.click(toggle());
    await screen.findByRole("dialog");
    act(() => {
      setViewport(1440);
      window.dispatchEvent(new Event("resize"));
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // docked now, with the choice the browser had (nothing remembered, a window that was small: closed)
    const aside = document.getElementById("thread-panel");
    expect(aside?.tagName).toBe("ASIDE");
    expect(aside?.getAttribute("data-state")).toBe("closed");
  });
});
