// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ThreadDescriber } from "@/features/chat/hooks/use-describe-thread";
import { DescriptionField, ThreadDescription } from "./thread-description";

afterEach(cleanup);

/** jsdom has no layout: the line is as wide as these say, and a resize says it again. */
let widths = { scroll: 100, client: 100 };
let resize: (() => void) | undefined;
beforeEach(() => {
  widths = { scroll: 100, client: 100 };
  resize = undefined;
  globalThis.ResizeObserver = class {
    constructor(callback: () => void) {
      resize = callback;
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockImplementation(() => widths.scroll);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => widths.client);
});
afterEach(() => vi.restoreAllMocks());

const LONG =
  "The person wants a plan for adding a test to the session expiry check, and the agent proposed one.";

describe("ThreadDescription", () => {
  it("a description that fits its line has no control", () => {
    render(<ThreadDescription text="Plan a test." />);
    expect(screen.getByText("Plan a test.")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("a description the line cuts has a Show more button that opens it and closes it again", () => {
    widths = { scroll: 400, client: 200 };
    render(<ThreadDescription text={LONG} />);
    const toggle = screen.getByRole("button", { name: "Show more" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    const text = screen.getByText(LONG);
    expect(toggle.getAttribute("aria-controls")).toBe(text.id);
    expect(text.className).toContain("truncate");

    fireEvent.click(toggle);
    const open = screen.getByRole("button", { name: "Show less" });
    expect(open.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(LONG).className).not.toContain("truncate");

    fireEvent.click(open);
    expect(screen.getByRole("button", { name: "Show more" })).toBeTruthy();
    expect(screen.getByText(LONG).className).toContain("truncate");
  });

  it("the control follows the width: it appears when the line gets narrower", () => {
    render(<ThreadDescription text={LONG} />);
    expect(screen.queryByRole("button")).toBeNull();
    widths = { scroll: 400, client: 200 };
    fireEvent.click(document.body); // (nothing: only the observer measures)
    resize?.();
    // measured through the observer's callback, in a state update
    return vi.waitFor(() => expect(screen.getByRole("button", { name: "Show more" })).toBeTruthy());
  });

  it("is plain text: Markdown and markup are shown as typed and nothing is made of them", () => {
    const text = "**bold** <b>tag</b> [link](https://evil.example) `code` # heading";
    const { container } = render(<ThreadDescription text={text} />);
    expect(screen.getByText(text)).toBeTruthy();
    const line = container.querySelector("[data-slot='thread-description']");
    expect(line?.querySelector("strong, b, a, code, h1, img, script")).toBeNull();
  });
});

function describer(over: Partial<ThreadDescriber> = {}): ThreadDescriber {
  return {
    draft: "Plan a test.",
    saving: false,
    error: null,
    field: { current: null },
    start: vi.fn(),
    change: vi.fn(),
    cancel: vi.fn(),
    save: vi.fn(),
    ...over,
  };
}

describe("DescriptionField", () => {
  it("is a labelled text box limited to what the API takes; typing goes to the editor", () => {
    const d = describer();
    render(<DescriptionField describer={d} />);
    const field = screen.getByRole("textbox", { name: "Thread description" }) as HTMLInputElement;
    expect(field.value).toBe("Plan a test.");
    expect(field.maxLength).toBe(500);
    fireEvent.change(field, { target: { value: "Another." } });
    expect(d.change).toHaveBeenCalledWith("Another.");
  });

  it("Enter and leaving the field save, Escape gives up", () => {
    const d = describer();
    render(<DescriptionField describer={d} />);
    const field = screen.getByRole("textbox", { name: "Thread description" });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(d.save).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(field, { key: "Escape" });
    expect(d.cancel).toHaveBeenCalledTimes(1);
    fireEvent.blur(field);
    expect(d.save).toHaveBeenCalledTimes(2);
  });

  it("says it is invalid after a refusal, and is off while it saves", () => {
    const { rerender } = render(<DescriptionField describer={describer({ error: "no" })} />);
    const field = screen.getByRole("textbox", { name: "Thread description" });
    expect(field.getAttribute("aria-invalid")).toBe("true");
    rerender(<DescriptionField describer={describer({ saving: true })} />);
    expect((field as HTMLInputElement).disabled).toBe(true);
  });
});
