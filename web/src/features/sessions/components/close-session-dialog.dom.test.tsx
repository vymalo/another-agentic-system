// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { SessionCard } from "../lib/cards";
import { type MockPage, mockPage } from "../test-mock";

/*
 * Closing a subagent, against the mock orchestrator (ADR 0072): an idle one closes at once; one that
 * works has two buttons, "Close when this round ends" and "Stop and close"; the answer's state is handed
 * back to be applied; the two refusals are in words; the focus goes back to what opened the dialog.
 */

let page: MockPage;
let CloseSessionDialog: typeof import("./close-session-dialog").CloseSessionDialog;

beforeAll(async () => {
  page = await mockPage();
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  ({ CloseSessionDialog } = await import("./close-session-dialog"));
});
afterAll(() => page.restore());
afterEach(cleanup);

const cardOf = (child: string, over: Partial<SessionCard> = {}): SessionCard => ({
  session: "s1",
  threadId: child,
  agent: "planner",
  description: "Plan it",
  keepOpen: true,
  state: "idle",
  closing: false,
  closeAsked: false,
  ...over,
});

function mount(card: SessionCard) {
  const onClosed = vi.fn();
  const onOpenChange = vi.fn();
  render(
    <CloseSessionDialog
      open
      onOpenChange={onOpenChange}
      card={card}
      onClosed={onClosed}
      returnTo={{ current: null }}
    />,
  );
  return { onClosed, onOpenChange };
}

describe("an idle subagent", () => {
  it("asks 'Close planner? It takes no more messages.' and closes at once", async () => {
    const parent = await page.api.answered();
    const { child } = await page.api.opened(parent, { keepOpen: true });
    await page.api.until(
      async () => (await page.api.thread(child)).childOf?.state === "idle",
      "idle",
    );
    const { onClosed, onOpenChange } = mount(cardOf(child));
    const dialog = screen.getByRole("dialog", { name: "Close subagent" });
    expect(dialog.textContent).toContain("Close planner? It takes no more messages.");
    expect(within(dialog).queryByRole("button", { name: "Stop and close" })).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Close subagent" }));
    await waitFor(() => expect(onClosed).toHaveBeenCalledWith(child, "closed", false));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    const asked = await page.api.of(child, "child_session_close_asked");
    expect(asked[0]?.data).toMatchObject({ cancel: false });
  });

  it("draws the helper's name as text", async () => {
    mount(cardOf("00000000-0000-4000-8000-0000000000aa", { agent: "<i>planner</i>" }));
    expect(screen.getByRole("dialog").querySelector("i")).toBeNull();
    expect(screen.getByRole("dialog").textContent).toContain("<i>planner</i>");
  });
});

describe("a subagent that works", () => {
  async function working() {
    const parent = await page.api.answered();
    const { child } = await page.api.opened(parent, { keepOpen: true, text: "hold on" });
    await page.api.until(
      async () => (await page.api.thread(child)).childOf?.state === "working",
      "working",
    );
    return child;
  }

  it("offers two ways, and 'Close when this round ends' asks for cancel: false and applies closing", async () => {
    const child = await working();
    const { onClosed } = mount(cardOf(child, { state: "working", keepOpen: true }));
    const dialog = screen.getByRole("dialog");
    expect(dialog.textContent).toContain("It is working on a round now.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Close when this round ends" }));
    await waitFor(() => expect(onClosed).toHaveBeenCalledWith(child, "closing", false));
    expect((await page.api.of(child, "child_session_close_asked"))[0]?.data).toMatchObject({
      cancel: false,
    });
    await page.api.release(child);
  });

  it("'Stop and close' asks for cancel: true", async () => {
    const child = await working();
    const { onClosed } = mount(cardOf(child, { state: "working", keepOpen: true }));
    fireEvent.click(screen.getByRole("button", { name: "Stop and close" }));
    await waitFor(() => expect(onClosed).toHaveBeenCalled());
    expect(onClosed.mock.calls[0]?.[2]).toBe(true);
    expect((await page.api.of(child, "child_session_close_asked"))[0]?.data).toMatchObject({
      cancel: true,
    });
  });

  it("a close that already waits for the round has only the stronger way left", async () => {
    const child = await working();
    mount(cardOf(child, { state: "working", keepOpen: true, closing: true }));
    expect(screen.queryByRole("button", { name: "Close when this round ends" })).toBeNull();
    expect(screen.getByRole("button", { name: "Stop and close" })).toBeTruthy();
    expect(screen.getByText(/It closes when its round ends/)).toBeTruthy();
  });
});

describe("a close a person asked that waits for the round", () => {
  it("has only the stronger way left, as one that is on its way does", () => {
    mount(cardOf("00000000-0000-4000-8000-0000000000aa", { state: "working", closeAsked: true }));
    expect(screen.queryByRole("button", { name: "Close when this round ends" })).toBeNull();
    expect(screen.getByRole("button", { name: "Stop and close" })).toBeTruthy();
    expect(screen.getByText(/It closes when its round ends/)).toBeTruthy();
  });
});

describe("a refusal, in words, with the dialog kept", () => {
  it("not_child_thread: the thread is no subagent's", async () => {
    const parent = await page.api.answered();
    const { onClosed, onOpenChange } = mount(cardOf(parent));
    fireEvent.click(screen.getByRole("button", { name: "Close subagent" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("This thread is not a subagent.");
    expect(onClosed).not.toHaveBeenCalled();
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it("no_child_sessions: the card no longer offers them", async () => {
    const parent = await page.api.answered();
    const { child } = await page.api.opened(parent, { keepOpen: true });
    await page.api.until(
      async () => (await page.api.thread(child)).childOf?.state === "idle",
      "idle",
    );
    await page.api.config("sessions=off");
    mount(cardOf(child));
    fireEvent.click(screen.getByRole("button", { name: "Close subagent" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("This agent does not offer subagents right now.");
    await page.api.config("sessions=on");
  });
});

describe("the focus", () => {
  it("goes back to the button that opened the dialog", async () => {
    function Harness() {
      const from = useRef<HTMLElement | null>(null);
      const [open, setOpen] = useState(false);
      return (
        <>
          <button
            type="button"
            onClick={(e) => {
              from.current = e.currentTarget;
              setOpen(true);
            }}
          >
            Close planner
          </button>
          <CloseSessionDialog
            open={open}
            onOpenChange={setOpen}
            card={cardOf("00000000-0000-4000-8000-0000000000aa")}
            onClosed={() => {}}
            returnTo={from}
          />
        </>
      );
    }
    render(<Harness />);
    const opener = screen.getByRole("button", { name: "Close planner" });
    opener.focus();
    fireEvent.click(opener);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });
});
