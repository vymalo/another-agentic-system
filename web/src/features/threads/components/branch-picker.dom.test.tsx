// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { BranchPicker } from "./branch-picker";

afterEach(cleanup);

// the picker is a plain component of its own; the one that reads the thread's branches is in
// chat-shell-branches.dom.test.tsx
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }) }));

const picker = (index: number, total: number, onSelect = vi.fn()) => {
  render(
    <TooltipProvider>
      <BranchPicker index={index} total={total} onSelect={onSelect} />
    </TooltipProvider>,
  );
  const group = screen.getByRole("group", { name: "Versions of this message" });
  return {
    onSelect,
    group,
    previous: within(group).getByRole("button", { name: "Previous version" }) as HTMLButtonElement,
    next: within(group).getByRole("button", { name: "Next version" }) as HTMLButtonElement,
  };
};

describe("the picker of a message's versions", () => {
  it("shows which version of how many, counting from 1", () => {
    const { group } = picker(1, 3);
    expect(group.textContent).toContain("2/3");
  });

  it("the first version has no way back, the last no way on, and neither wraps round", () => {
    const first = picker(0, 2);
    expect(first.previous.disabled).toBe(true);
    expect(first.next.disabled).toBe(false);
    fireEvent.click(first.previous);
    expect(first.onSelect).not.toHaveBeenCalled();
    cleanup();
    const last = picker(1, 2);
    expect(last.previous.disabled).toBe(false);
    expect(last.next.disabled).toBe(true);
  });

  it("the arrows choose the neighbours", () => {
    const { previous, next, onSelect } = picker(1, 3);
    fireEvent.click(previous);
    expect(onSelect).toHaveBeenLastCalledWith(0);
    fireEvent.click(next);
    expect(onSelect).toHaveBeenLastCalledWith(2);
  });

  it("says the version in a polite live region, in words, and hides the eye's 2/3 from a screen reader", () => {
    const { group } = picker(1, 3);
    const status = within(group).getByRole("status");
    expect(status.textContent).toBe("Version 2 of 3");
    const eye = within(group).getByText("2/3");
    expect(eye.getAttribute("aria-hidden")).toBe("true");
  });

  it("the live region follows the count when it changes", () => {
    const { rerender } = render(
      <TooltipProvider>
        <BranchPicker index={0} total={2} onSelect={vi.fn()} />
      </TooltipProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Version 1 of 2");
    rerender(
      <TooltipProvider>
        <BranchPicker index={1} total={3} onSelect={vi.fn()} />
      </TooltipProvider>,
    );
    expect(screen.getByRole("status").textContent).toBe("Version 2 of 3");
  });
});
