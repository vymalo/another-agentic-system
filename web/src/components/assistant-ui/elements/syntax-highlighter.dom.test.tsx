// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import type { ComponentPropsWithoutRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as highlight from "@/lib/highlight";
import { SyntaxHighlighter } from "./syntax-highlighter";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const components = {
  Pre: (props: ComponentPropsWithoutRef<"pre">) => <pre data-testid="pre" {...props} />,
  Code: (props: ComponentPropsWithoutRef<"code">) => <code data-testid="code" {...props} />,
};

const RUST = 'fn main() {\n    println!("hi");\n}';

describe("SyntaxHighlighter", () => {
  it("shows the code as plain text at once, then coloured tokens that say the same text", async () => {
    render(<SyntaxHighlighter components={components} language="rust" code={RUST} />);
    const code = screen.getByTestId("code");
    expect(code.textContent).toBe(RUST);
    expect(code.querySelector("[data-token]")).toBeNull();
    await waitFor(() => expect(code.querySelector("[data-token]")).not.toBeNull());
    expect(code.textContent).toBe(RUST);
    expect(code.closest("pre")).not.toBeNull();
    const keyword = [...code.querySelectorAll<HTMLElement>("[data-token]")].find(
      (t) => t.textContent === "fn",
    );
    expect(keyword?.style.getPropertyValue("--shiki-light")).toMatch(/^#/);
    expect(keyword?.style.getPropertyValue("--shiki-dark")).toMatch(/^#/);
  });

  it("an unknown language stays plain text", async () => {
    const spy = vi.spyOn(highlight, "highlightCode");
    render(<SyntaxHighlighter components={components} language="brainfuck" code="+++[>+<-]" />);
    await new Promise((r) => setTimeout(r, 300));
    const code = screen.getByTestId("code");
    expect(code.textContent).toBe("+++[>+<-]");
    expect(code.querySelector("[data-token]")).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("no language is plain text too", async () => {
    render(<SyntaxHighlighter components={components} language="" code="just words" />);
    await new Promise((r) => setTimeout(r, 300));
    expect(screen.getByTestId("code").textContent).toBe("just words");
    expect(screen.getByTestId("code").querySelector("[data-token]")).toBeNull();
  });

  it("a highlighter that fails leaves the plain text and throws nothing", async () => {
    const spy = vi.spyOn(highlight, "highlightCode").mockRejectedValue(new Error("no grammar"));
    render(<SyntaxHighlighter components={components} language="rust" code={RUST} />);
    await waitFor(() => expect(spy).toHaveBeenCalled());
    await act(async () => {});
    const code = screen.getByTestId("code");
    expect(code.textContent).toBe(RUST);
    expect(code.querySelector("[data-token]")).toBeNull();
  });

  it("waits for the code to stop changing before it highlights (a streaming answer)", async () => {
    vi.useFakeTimers();
    const spy = vi.spyOn(highlight, "highlightCode").mockResolvedValue(null);
    const { rerender } = render(
      <SyntaxHighlighter components={components} language="rust" code="fn" />,
    );
    for (const next of ["fn m", "fn ma", "fn mai", "fn main"]) {
      await act(async () => {
        vi.advanceTimersByTime(40);
      });
      rerender(<SyntaxHighlighter components={components} language="rust" code={next} />);
    }
    expect(spy).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(400);
    });
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith("fn main", "rust");
  });

  it("drops colours that belong to older code", async () => {
    const { rerender } = render(
      <SyntaxHighlighter components={components} language="rust" code="fn a() {}" />,
    );
    const code = screen.getByTestId("code");
    await waitFor(() => expect(code.querySelector("[data-token]")).not.toBeNull());
    rerender(<SyntaxHighlighter components={components} language="rust" code="fn ab() {}" />);
    expect(screen.getByTestId("code").textContent).toBe("fn ab() {}");
    expect(screen.getByTestId("code").querySelector("[data-token]")).toBeNull();
  });
});
