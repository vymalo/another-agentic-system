// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Hint } from "./hint";

beforeAll(() => {
  // Radix positions the tooltip with a resize observer, which jsdom does not have
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe("Hint", () => {
  it("shows its words when the keyboard focuses the control, and keeps the control's own name", async () => {
    render(
      <Hint label="Copy the link">
        <button type="button" aria-label="Copy">
          icon
        </button>
      </Hint>,
    );
    const button = screen.getByRole("button", { name: "Copy" });
    expect(screen.queryByRole("tooltip")).toBeNull();
    button.focus();
    expect((await screen.findByRole("tooltip")).textContent).toContain("Copy the link");
    // the name did not change: the tooltip is only a description
    expect(screen.getByRole("button", { name: "Copy" })).toBe(button);
  });

  it("shows its words when a pointer rests on it, and goes when the pointer leaves", async () => {
    render(
      <Hint label="Shared · public">
        <span role="status" aria-label="Shared">
          icon
        </span>
      </Hint>,
    );
    const status = screen.getByRole("status", { name: "Shared" });
    fireEvent.pointerMove(status, { pointerType: "mouse" });
    expect((await screen.findByRole("tooltip")).textContent).toContain("Shared · public");
    // the tooltip can be moved onto (WCAG 1.4.13), so it goes when the pointer is away from both
    fireEvent.pointerLeave(status, { pointerType: "mouse" });
    fireEvent.pointerMove(document.body, { pointerType: "mouse", clientX: 900, clientY: 900 });
    await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
  });

  it("stays shut while it is suppressed (the control's own menu is open), and opens again after", async () => {
    const { rerender } = render(
      <Hint label="Thread options" suppressed>
        <button type="button" aria-label="Thread options">
          icon
        </button>
      </Hint>,
    );
    screen.getByRole("button", { name: "Thread options" }).focus();
    await Promise.resolve();
    expect(screen.queryByRole("tooltip")).toBeNull();
    rerender(
      <Hint label="Thread options">
        <button type="button" aria-label="Thread options">
          icon
        </button>
      </Hint>,
    );
    expect((await screen.findByRole("tooltip")).textContent).toContain("Thread options");
  });

  it("Escape dismisses it, and the control keeps the focus", async () => {
    render(
      <Hint label="Copy the link">
        <button type="button" aria-label="Copy">
          icon
        </button>
      </Hint>,
    );
    const button = screen.getByRole("button", { name: "Copy" });
    button.focus();
    await screen.findByRole("tooltip");
    fireEvent.keyDown(button, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
    expect(document.activeElement).toBe(button);
  });
});
