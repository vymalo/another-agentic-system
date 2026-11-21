// @vitest-environment jsdom
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type MockPage, mockPage } from "@/features/sessions/test-mock";

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

/*
 * A subagent's own thread in the app, against the mock orchestrator (child-sessions/v1, ADR 0072): the
 * banner under the top bar, a message box that follows the session (closed, closing, kept open or not),
 * what a subagent leaves out (@, the agent's commands, the tools picker, fork, edit, regenerate, Share, the
 * agent switch) and the refusals of a run, which keep the text in the box.
 */

let page: MockPage;
let ChatShell: typeof import("./chat-shell").ChatShell;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 });
  page = await mockPage();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(() => page.restore());
beforeEach(() => {
  // a wide window: the panel is docked
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  page.seen.length = 0;
});
afterEach(() => {
  cleanup();
  page.fake("POST /agui/agents/adam", null);
});

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

type Child = { parent: string; child: string; parentTitle: string };

/**
 * A subagent the agent has opened: its parent (answered, so that it has a context) and the child thread.
 * `hold` starts a round that waits for `release`; without it the round ends at once (and a session that
 * closes when done is closed).
 */
async function aChild(opts: { keepOpen: boolean; hold: boolean }): Promise<Child> {
  const parent = await page.api.answered();
  const { child } = await page.api.opened(parent, {
    keepOpen: opts.keepOpen,
    text: opts.hold ? "hold go" : "go",
    description: "Plan the docs",
  });
  return { parent, child, parentTitle: (await page.api.thread(parent)).title };
}

const banner = () => document.querySelector('[data-slot="child-banner"]') as HTMLElement;
const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(box(), { target: { value: text } });
const dialog = () => screen.getByRole("dialog", { name: "Close subagent" });
const stateOf = async (child: string) => (await page.api.thread(child)).childOf?.state ?? undefined;
const posts = () => page.seen.filter((c) => c.startsWith("POST /agui/agents/adam"));

describe("the banner", () => {
  it("names the way back to the parent, the helper, how it ends and where it stands", async () => {
    const { parent, child, parentTitle } = await aChild({ keepOpen: true, hold: true });
    shell(child);
    await waitFor(() => expect(banner()).toBeTruthy());
    const back = within(banner()).getByRole("link", { name: /Back to/ });
    expect(back.getAttribute("href")).toBe(`/threads/${parent}`);
    // the title is the parent's, from the list or the lazy read
    await waitFor(() => expect(back.textContent).toContain(parentTitle));
    expect(banner().querySelector('[data-slot="child-agent"]')?.textContent).toBe("planner");
    expect(within(banner()).getByText("Stays open")).toBeTruthy();
    await waitFor(() =>
      expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Working"),
    );
    expect(within(banner()).getByRole("button", { name: "Close planner" })).toBeTruthy();
    await page.api.release(child);
    await waitFor(() =>
      expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Idle"),
    );
  });

  it("says Closes when done for a session that is not kept open", async () => {
    const { child } = await aChild({ keepOpen: false, hold: true });
    shell(child);
    await waitFor(() => expect(banner()).toBeTruthy());
    expect(within(banner()).getByText("Closes when done")).toBeTruthy();
    await page.api.release(child);
  });

  it("is not on a thread that is nobody's helper", async () => {
    const parent = await page.api.answered();
    shell(parent);
    await screen.findByLabelText("Message");
    expect(banner()).toBeNull();
  });
});

describe("the message box follows the session", () => {
  it("is a notice, with the way back, once the session is closed", async () => {
    const { parent, child } = await aChild({ keepOpen: false, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "closed", "the session closed");
    shell(child);
    const notice = await waitFor(() => {
      const n = document.querySelector('[data-slot="session-closed-notice"]');
      expect(n).toBeTruthy();
      return n as HTMLElement;
    });
    expect(notice.textContent).toContain("This subagent is closed and takes no more messages.");
    expect(
      within(notice)
        .getByRole("link", { name: /Back to/ })
        .getAttribute("href"),
    ).toBe(`/threads/${parent}`);
    expect(screen.queryByLabelText("Message")).toBeNull();
    // and no Close: nothing is left to close
    expect(within(banner()).queryByRole("button", { name: /Close/ })).toBeNull();
    expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Closed");
  });

  it("turns into the notice when the person closes the subagent", async () => {
    const { child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    shell(child);
    await screen.findByLabelText("Message");
    fireEvent.click(await screen.findByRole("button", { name: "Close planner" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Close subagent" }));
    await waitFor(() =>
      expect(document.querySelector('[data-slot="session-closed-notice"]')).toBeTruthy(),
    );
    expect(screen.queryByLabelText("Message")).toBeNull();
  });

  it("says Closing… while a close waits for the round, keeps the box, and can be made stronger once", async () => {
    const { child } = await aChild({ keepOpen: true, hold: true });
    shell(child);
    await waitFor(() =>
      expect(banner()?.querySelector('[data-slot="child-state"]')?.textContent).toBe("Working"),
    );
    fireEvent.click(within(banner()).getByRole("button", { name: "Close planner" }));
    // a round works: two ways to end, and the gentle one is asked
    fireEvent.click(within(dialog()).getByRole("button", { name: "Close when this round ends" }));
    await waitFor(() =>
      expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Closing…"),
    );
    expect(screen.getByText(/Closing… this subagent takes no more messages/)).toBeTruthy();
    expect(screen.getByLabelText("Message")).toBeTruthy();

    // still offered, because it was asked with cancel: false; the dialog has only the stronger way
    fireEvent.click(within(banner()).getByRole("button", { name: "Close planner" }));
    expect(
      within(dialog()).queryByRole("button", { name: "Close when this round ends" }),
    ).toBeNull();
    fireEvent.click(within(dialog()).getByRole("button", { name: "Stop and close" }));
    await waitFor(() =>
      expect(document.querySelector('[data-slot="session-closed-notice"]')).toBeTruthy(),
    );
  });

  it("offers no second Stop and close once a close that stopped the round was asked", async () => {
    const { child } = await aChild({ keepOpen: true, hold: true });
    shell(child);
    await waitFor(() =>
      expect(banner()?.querySelector('[data-slot="child-state"]')?.textContent).toBe("Working"),
    );
    // the answer says `closing` and the round is held: nothing stronger remains
    page.fake("POST /api/threads/:id/session/close", {
      status: 202,
      body: { state: "closing" },
    } as never);
    fireEvent.click(within(banner()).getByRole("button", { name: "Close planner" }));
    fireEvent.click(within(dialog()).getByRole("button", { name: "Stop and close" }));
    await waitFor(() =>
      expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Closing…"),
    );
    expect(within(banner()).queryByRole("button", { name: /Close planner/ })).toBeNull();
    page.fake("POST /api/threads/:id/session/close", null);
    await page.api.release(child);
  });
});

describe("sending in a subagent", () => {
  it("starts the next round when idle, and the answer is in the log", async () => {
    const { child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    shell(child);
    await screen.findByLabelText("Message");
    type("second round");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(screen.getByText(/Round 2: second round/)).toBeTruthy());
  });

  it("offers Stop and send only when the session is kept open", async () => {
    const kept = await aChild({ keepOpen: true, hold: true });
    const view = shell(kept.child);
    await waitFor(() => expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy());
    type("steer me");
    const more = await screen.findByRole("button", { name: "Delivery options" });
    more.focus();
    fireEvent.keyDown(more, { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: /Stop and send/ })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await page.api.release(kept.child);
    view.unmount();

    const closes = await aChild({ keepOpen: false, hold: true });
    shell(closes.child);
    await waitFor(() => expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy());
    type("steer me");
    await screen.findByRole("button", { name: "Send" });
    expect(screen.queryByRole("button", { name: "Delivery options" })).toBeNull();
    // Ctrl/⌘+Shift+Enter is not Stop and send here: nothing is sent
    posts().length = 0;
    page.seen.length = 0;
    fireEvent.keyDown(box(), { key: "Enter", shiftKey: true, ctrlKey: true });
    await new Promise((r) => setTimeout(r, 100));
    expect(posts()).toEqual([]);
    await page.api.release(closes.child);
  });

  it("says in its Stop hint what Stop does to the subagent", async () => {
    const kept = await aChild({ keepOpen: true, hold: true });
    const view = shell(kept.child);
    const stop = await screen.findByRole("button", { name: "Stop" });
    fireEvent.pointerMove(stop);
    fireEvent.focus(stop);
    expect(
      (await screen.findAllByText("Stops this round, the subagent stays open")).length,
    ).toBeGreaterThan(0);
    await page.api.release(kept.child);
    view.unmount();
    const closes = await aChild({ keepOpen: false, hold: true });
    shell(closes.child);
    const stop2 = await screen.findByRole("button", { name: "Stop" });
    fireEvent.pointerMove(stop2);
    fireEvent.focus(stop2);
    expect(
      (await screen.findAllByText("Stops this round and closes the subagent")).length,
    ).toBeGreaterThan(0);
    await page.api.release(closes.child);
  });

  it("stops the round: the subagent that is kept open goes idle", async () => {
    const { child } = await aChild({ keepOpen: true, hold: true });
    shell(child);
    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle after Stop");
    await waitFor(() =>
      expect(banner().querySelector('[data-slot="child-state"]')?.textContent).toBe("Idle"),
    );
  });
});

describe("what a subagent leaves out", () => {
  it("has no @ list, no agent commands and no tools picker, where its parent has the tools picker", async () => {
    const { parent, child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    const parentView = shell(parent);
    await screen.findByLabelText("Message");
    expect(await screen.findByRole("button", { name: "Tools" })).toBeTruthy();
    parentView.unmount();

    shell(child);
    await screen.findByLabelText("Message");
    expect(screen.queryByRole("button", { name: "Tools" })).toBeNull();
    type("@");
    box().setSelectionRange(1, 1);
    fireEvent.select(box());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("listbox", { name: /agent/i })).toBeNull();
    expect(document.querySelector('[data-slot="mention-listbox"]')).toBeNull();
  });

  it("has no fork, edit, regenerate or Share, and says why the agent cannot be switched", async () => {
    const { parent, child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    // the control: the parent's finished turn can be forked and regenerated
    const parentView = shell(parent);
    await screen.findByLabelText("Message");
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: /Fork/ }).length).toBeGreaterThan(0),
    );
    parentView.unmount();

    shell(child);
    await screen.findByLabelText("Message");
    await waitFor(() => expect(screen.getByText(/Round 1: go/)).toBeTruthy());
    expect(screen.queryByRole("button", { name: /Fork/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Regenerate" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();

    const options = screen.getByRole("button", { name: "Thread options" });
    options.focus();
    fireEvent.keyDown(options, { key: "Enter" });
    await screen.findByRole("menu");
    expect(screen.queryByRole("menuitem", { name: /Share/ })).toBeNull();
    expect(screen.getByRole("menuitem", { name: /Rename/ })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });

    // the agent menu: the other agents are not offered, and the reason is in view
    const switcher = document.querySelector('[data-slot="agent-menu-trigger"]') as HTMLElement;
    switcher.focus();
    fireEvent.keyDown(switcher, { key: "Enter" });
    expect(await screen.findByText("A subagent is part of its parent chat")).toBeTruthy();
  });
});

describe("before the thread is known", () => {
  const openAt = async () => {
    type("");
    type("ask @");
    await new Promise((r) => setTimeout(r, 100));
    return screen.queryByRole("listbox", { name: "Agents to mention" });
  };

  it("offers no @ or fork on a subagent's page until the thread, or its own card, says what it is", async () => {
    const { parent, child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    // the control: a thread that is nobody's helper gets them once it is read
    const release = page.gate("GET /api/threads/:id");
    const parentView = shell(parent);
    await screen.findByLabelText("Message");
    await waitFor(() =>
      expect(screen.getAllByText(/Round 1: go|echo|Plan/).length).toBeGreaterThan(0),
    );
    expect(await openAt()).toBeNull();
    expect(screen.queryByRole("button", { name: /Fork/ })).toBeNull();
    release();
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: /Fork/ }).length).toBeGreaterThan(0),
    );
    await waitFor(async () => expect(await openAt()).toBeTruthy());
    parentView.unmount();

    // the subagent's page: with its resource unread, the stream's card may come first; either way nothing
    // a subagent lacks is there before it is known, and nothing after
    const releaseChild = page.gate("GET /api/threads/:id");
    shell(child);
    await screen.findByLabelText("Message");
    await waitFor(() => expect(screen.getByText(/Round 1: go/)).toBeTruthy());
    expect(await openAt()).toBeNull();
    expect(screen.queryByRole("button", { name: /Fork/ })).toBeNull();
    releaseChild();
    await waitFor(() => expect(banner()).toBeTruthy());
    expect(await openAt()).toBeNull();
    expect(screen.queryByRole("button", { name: /Fork/ })).toBeNull();
  });
});

describe("a run that is refused keeps the text in the box", () => {
  const refuse = (status: number, code: string) =>
    page.fake("POST /agui/agents/adam", {
      status,
      body: { title: "Refused", status, detail: "the server's own words", code },
    });

  it.each([
    [409, "session_closed", "This subagent is closed and takes no more messages."],
    [422, "child_mentions", "A subagent cannot ask other agents"],
  ])("%s %s: the screen's words, and the draft is back", async (status, code, words) => {
    const { child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    shell(child);
    await screen.findByLabelText("Message");
    refuse(status, code);
    type("my careful message");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(words);
    await waitFor(() => expect(box().value).toBe("my careful message"));
  });

  it("session_closed reads the thread again, so the closed notice can show", async () => {
    const { child } = await aChild({ keepOpen: true, hold: false });
    await page.api.until(async () => (await stateOf(child)) === "idle", "idle");
    shell(child);
    await screen.findByLabelText("Message");
    refuse(409, "session_closed");
    const reads = () => page.seen.filter((c) => c.startsWith("GET /api/threads/:id ")).length;
    const before = reads();
    type("too late");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("alert");
    await waitFor(() => expect(reads()).toBeGreaterThan(before));
  });

  it("session_closes_on_stop while the round works: the words, and the draft is back", async () => {
    const { child } = await aChild({ keepOpen: true, hold: true });
    shell(child);
    await waitFor(() => expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy());
    // the replay is on screen before a send is offered
    await waitFor(() => expect(within(screen.getByRole("log")).getByText("hold go")).toBeTruthy());
    refuse(422, "session_closes_on_stop");
    type("stop and say this");
    const more = await screen.findByRole("button", { name: "Delivery options" });
    more.focus();
    fireEvent.keyDown(more, { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: /Stop and send/ }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("closes when its work is stopped");
    await waitFor(() => expect(box().value).toBe("stop and say this"));
    page.fake("POST /agui/agents/adam", null);
    await page.api.release(child);
  });
});
