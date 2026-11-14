// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { QuoteTargetContext } from "@/features/chat/components/quote";
import type { ThreadFork } from "@/features/threads/components/fork-provider";
import { COPY_FAILED } from "@/hooks/use-copy-to-clipboard";
import { REGENERATE_WAIT, TurnActions } from "./turn-actions";

/*
 * What a person can do under a finished agent turn: Copy, Fork from here and, on the newest turn
 * only, Regenerate (an edit with the same words, ADR 0029). The fork actions are the page's own
 * (`useThreadFork`); they are replaced here by spies.
 */

let fork: ThreadFork;
vi.mock("@/features/threads/components/fork-provider", () => ({ useThreadFork: () => fork }));

const forkOf = (over: Partial<ThreadFork> = {}): ThreadFork => ({
  available: true,
  readOnly: null,
  turnOpen: false,
  busy: false,
  error: null,
  errorKind: "fork",
  clearError: () => {},
  canForkRun: () => true,
  forkRun: vi.fn(),
  continueWith: async () => false,
  seqOfMessage: () => undefined,
  editMessage: async () => false,
  regenerate: vi.fn(async () => true),
  ...over,
});

const began = { seq: 3, text: "echo first" };

const show = (
  props: Partial<Parameters<typeof TurnActions>[0]> = {},
  over: Partial<ThreadFork> = {},
) => {
  fork = forkOf(over);
  render(
    <TooltipProvider>
      <TurnActions text="echo: echo first" runId="run-1" last began={began} {...props} />
    </TooltipProvider>,
  );
  return fork;
};
const button = () => screen.queryByRole("button", { name: "Regenerate" });

afterEach(cleanup);

describe("Regenerate under the newest finished turn", () => {
  it("is a real button with a name, and says the person's words again", () => {
    const f = show();
    const b = button();
    expect(b?.tagName).toBe("BUTTON");
    expect(b?.getAttribute("aria-disabled")).not.toBe("true");
    fireEvent.click(b as HTMLElement);
    expect(f.regenerate).toHaveBeenCalledWith(3, "echo first");
  });

  it("waits while the turn is going on: aria-disabled, focusable, and a click does nothing", () => {
    const f = show({}, { turnOpen: true });
    const b = button() as HTMLElement;
    expect(b.getAttribute("aria-disabled")).toBe("true");
    expect(b.hasAttribute("disabled")).toBe(false);
    expect(REGENERATE_WAIT).toMatch(/once the agent has finished/);
    fireEvent.click(b);
    expect(f.regenerate).not.toHaveBeenCalled();
  });

  it("waits while a fork is being made", () => {
    const f = show({}, { busy: true });
    fireEvent.click(button() as HTMLElement);
    expect(f.regenerate).not.toHaveBeenCalled();
  });

  it("is not on an earlier turn", () => {
    show({ last: false });
    expect(button()).toBeNull();
  });

  it("is not offered when the turn did not begin with a message to say again", () => {
    show({ began: undefined });
    expect(button()).toBeNull();
    // the other actions are still there
    expect(screen.getByRole("button", { name: "Copy" })).toBeTruthy();
  });

  it("is not offered to a person who may not act on the thread", () => {
    show(
      {},
      { available: false, readOnly: "Read only: your roles do not let you write in threads." },
    );
    expect(button()).toBeNull();
  });
});

describe("Copy and Quote under a turn", () => {
  const stubClipboard = (writeText: (text: string) => Promise<void>) =>
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });

  it("Copy puts the source text on the clipboard, not what the page draws", async () => {
    const writeText = vi.fn(async () => {});
    stubClipboard(writeText);
    show({ text: "# Title\n\n**bold** and `code`" });
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("# Title\n\n**bold** and `code`"));
    await waitFor(() => expect(screen.getAllByText("Copied").length).toBeGreaterThan(0));
  });

  it("says in words when the browser refuses", async () => {
    stubClipboard(() => Promise.reject(new Error("denied")));
    show();
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(screen.getAllByText(COPY_FAILED).length).toBeGreaterThan(0));
  });

  it("says it too when there is no clipboard at all (an insecure page)", async () => {
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    show();
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(screen.getAllByText(COPY_FAILED).length).toBeGreaterThan(0));
  });

  it("Quote is offered only where there is a composer, and quotes the whole message when nothing is selected", () => {
    show();
    expect(screen.queryByRole("button", { name: "Quote" })).toBeNull();
    cleanup();
    const insert = vi.fn();
    fork = forkOf();
    render(
      <TooltipProvider>
        <QuoteTargetContext.Provider value={{ available: true, insert }}>
          <div data-slot="agent-turn">
            <TurnActions text={"one\n\ntwo"} runId="run-1" last began={began} />
          </div>
        </QuoteTargetContext.Provider>
      </TooltipProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Quote" }));
    expect(insert).toHaveBeenCalledWith("> one\n>\n> two");
  });

  it("Quote is not offered where the composer is not (a link, a person who may only read)", () => {
    render(
      <TooltipProvider>
        <QuoteTargetContext.Provider value={{ available: false, insert: vi.fn() }}>
          <TurnActions text="words" runId="run-1" last={false} />
        </QuoteTargetContext.Provider>
      </TooltipProvider>,
    );
    expect(screen.queryByRole("button", { name: "Quote" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy" })).toBeTruthy();
  });
});
