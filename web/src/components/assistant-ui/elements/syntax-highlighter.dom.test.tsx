// @vitest-environment jsdom
import { TextMessagePartProvider } from "@assistant-ui/react";
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

/** The block as the markdown draws it: inside the text part of a message, which is running while it streams. */
function Block({
  code,
  language = "rust",
  running = false,
}: {
  code: string;
  language?: string;
  running?: boolean;
}) {
  return (
    <TextMessagePartProvider text={code} isRunning={running}>
      <SyntaxHighlighter components={components} language={language} code={code} />
    </TextMessagePartProvider>
  );
}

const codeEl = () => screen.getByTestId("code");
const coloured = () => codeEl().querySelector("[data-token]") !== null;

describe("SyntaxHighlighter", () => {
  it("shows the code as plain text at once, then coloured tokens that say the same text", async () => {
    render(<Block code={RUST} />);
    expect(codeEl().textContent).toBe(RUST);
    expect(coloured()).toBe(false);
    await waitFor(() => expect(coloured()).toBe(true));
    expect(codeEl().textContent).toBe(RUST);
    expect(codeEl().closest("pre")).not.toBeNull();
    const keyword = [...codeEl().querySelectorAll<HTMLElement>("[data-token]")].find(
      (t) => t.textContent === "fn",
    );
    expect(keyword?.style.getPropertyValue("--shiki-light")).toMatch(/^#/);
    expect(keyword?.style.getPropertyValue("--shiki-dark")).toMatch(/^#/);
  });

  it("an unknown language stays plain text", async () => {
    const spy = vi.spyOn(highlight, "highlightCode");
    render(<Block language="brainfuck" code="+++[>+<-]" />);
    await new Promise((r) => setTimeout(r, 300));
    expect(codeEl().textContent).toBe("+++[>+<-]");
    expect(coloured()).toBe(false);
    expect(spy).not.toHaveBeenCalled();
  });

  it("no language is plain text too", async () => {
    render(<Block language="" code="just words" />);
    await new Promise((r) => setTimeout(r, 300));
    expect(codeEl().textContent).toBe("just words");
    expect(coloured()).toBe(false);
  });

  it("a language the web dropped for want of a verified licence (YAML, TOML) stays plain text", async () => {
    const spy = vi.spyOn(highlight, "highlightCode");
    for (const language of ["yaml", "yml", "toml"]) {
      cleanup();
      render(<Block language={language} code="key = 1" />);
      await new Promise((r) => setTimeout(r, 100));
      expect(coloured()).toBe(false);
    }
    expect(spy).not.toHaveBeenCalled();
  });

  it("a highlighter that fails leaves the plain text and throws nothing", async () => {
    const spy = vi.spyOn(highlight, "highlightCode").mockRejectedValue(new Error("no grammar"));
    render(<Block code={RUST} />);
    await waitFor(() => expect(spy).toHaveBeenCalled());
    await act(async () => {});
    expect(codeEl().textContent).toBe(RUST);
    expect(coloured()).toBe(false);
  });

  it("never colours a block while the answer streams, and colours it once when it is final", async () => {
    vi.useFakeTimers();
    const spy = vi.spyOn(highlight, "highlightCode");
    const steps = [
      "fn",
      "fn ma",
      "fn main() {",
      "fn main() {\n    let x = 1;",
      "fn main() {\n    let x = 1;\n}",
    ];
    const last = steps[steps.length - 1] ?? "";
    const { rerender } = render(<Block code={steps[0] ?? ""} running />);
    const seen: boolean[] = [];
    for (const next of steps) {
      rerender(<Block code={next} running />);
      // the model pauses between pieces, longer than a debounce would wait
      await act(async () => {
        await vi.advanceTimersByTimeAsync(600);
      });
      seen.push(coloured());
      expect(codeEl().textContent).toBe(next);
    }
    expect(seen).toEqual([false, false, false, false, false]);
    expect(spy).not.toHaveBeenCalled();

    // the answer ends: the colours come, and the text is the same
    rerender(<Block code={last} />);
    vi.useRealTimers();
    await waitFor(() => expect(coloured()).toBe(true));
    expect(codeEl().textContent).toBe(last);
    expect(spy).toHaveBeenCalledTimes(1);
    // and they stay: no coloured, plain, coloured
    await new Promise((r) => setTimeout(r, 200));
    expect(coloured()).toBe(true);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("a block over the size limit stays plain and the highlighter is never asked", async () => {
    const spy = vi.spyOn(highlight, "highlightCode");
    const longLine = "x".repeat(highlight.MAX_CODE_CHARS + 1);
    const manyLines = Array.from(
      { length: highlight.MAX_CODE_LINES + 1 },
      (_, n) => `// ${n}`,
    ).join("\n");
    for (const code of [longLine, manyLines]) {
      cleanup();
      render(<Block code={code} />);
      await new Promise((r) => setTimeout(r, 300));
      expect(codeEl().textContent).toBe(code);
      expect(coloured()).toBe(false);
    }
    expect(spy).not.toHaveBeenCalled();
  });

  it("a block at the size limit is still highlighted", async () => {
    const spy = vi
      .spyOn(highlight, "highlightCode")
      .mockResolvedValue([[{ content: "x", style: {} }]]);
    render(<Block code={"x".repeat(highlight.MAX_CODE_CHARS)} />);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
  });
});
