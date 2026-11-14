// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ThreadFork } from "@/features/threads/components/fork-provider";
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
