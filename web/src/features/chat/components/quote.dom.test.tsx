// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { MAX_QUOTE, QUOTE_ACROSS, QUOTE_CUT } from "@/features/chat/lib/quote";
import { QuoteButton, QuoteTargetContext } from "./quote";

/*
 * The two promises of Quote that only the component keeps: it says when a quote was cut at its bound, and the note about a
 * selection across messages goes at the next selection.
 */

afterEach(cleanup);

const note = () => document.querySelector('[data-slot="quote-note"]') as HTMLElement;

describe("Quote's bound", () => {
  it("cuts a long message at a whole line, quotes what fits and says it was cut", () => {
    const insert = vi.fn();
    const source = Array.from({ length: 3000 }, (_, i) => `line ${i} of a very long message`).join(
      "\n",
    );
    expect(source.length).toBeGreaterThan(MAX_QUOTE);
    render(
      <TooltipProvider>
        <QuoteTargetContext.Provider value={{ available: true, insert }}>
          <div data-slot="agent-turn">
            <QuoteButton source={source} />
          </div>
        </QuoteTargetContext.Provider>
      </TooltipProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Quote" }));
    const block = insert.mock.calls[0]?.[0] as string;
    // the prefix `> ` is two characters a line over the bound of the text itself
    const text = block
      .split("\n")
      .map((l) => l.slice(2))
      .join("\n");
    expect(text.length).toBeLessThanOrEqual(MAX_QUOTE);
    expect(source.startsWith(text)).toBe(true);
    expect(text.endsWith("of a very long message")).toBe(true);
    expect(note().textContent).toBe(QUOTE_CUT);
  });

  it("says nothing when the quote fits", () => {
    const insert = vi.fn();
    render(
      <TooltipProvider>
        <QuoteTargetContext.Provider value={{ available: true, insert }}>
          <div data-slot="agent-turn">
            <QuoteButton source="short" />
          </div>
        </QuoteTargetContext.Provider>
      </TooltipProvider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Quote" }));
    expect(insert).toHaveBeenCalledWith("> short");
    expect(note().textContent).toBe("");
  });
});

describe("the note about a selection across messages", () => {
  it("is shown, quotes nothing, and goes at the next selection", async () => {
    const insert = vi.fn();
    render(
      <TooltipProvider>
        <QuoteTargetContext.Provider value={{ available: true, insert }}>
          <div data-slot="user-message">
            <p id="first">the first message</p>
          </div>
          <div data-slot="agent-turn">
            <p id="second">the second message</p>
            <QuoteButton source="the second message" />
          </div>
        </QuoteTargetContext.Provider>
      </TooltipProvider>,
    );
    const range = document.createRange();
    range.setStart(document.getElementById("first")?.firstChild as Node, 4);
    range.setEnd(document.getElementById("second")?.firstChild as Node, 6);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);

    fireEvent.click(screen.getByRole("button", { name: "Quote" }));
    expect(note().textContent).toBe(QUOTE_ACROSS);
    expect(insert).not.toHaveBeenCalled();

    await act(async () => {
      document.dispatchEvent(new Event("selectionchange"));
    });
    expect(note().textContent).toBe("");
  });
});
