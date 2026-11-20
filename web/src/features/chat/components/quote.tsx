"use client";

import { useAui } from "@assistant-ui/react";
import { QuoteIcon } from "lucide-react";
import {
  createContext,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import {
  insertQuote,
  limitQuote,
  MESSAGE_ROOT,
  pickSelection,
  QUOTE_ACROSS,
  QUOTE_CUT,
  quoteBlock,
  quoteCode,
} from "@/features/chat/lib/quote";
import { cn } from "@/lib/utils";

/** What the open conversation offers for Quote: a box to put it in (absent on a link and for a person who may only read). */
type QuoteTarget = {
  available: boolean;
  /** Puts the block quote in the box at its caret (else at its end), and moves the focus to the box. */
  insert: (block: string) => void;
};

const NONE: QuoteTarget = { available: false, insert: () => {} };
/** The context itself, for a test that stands in for the composer. */
export const QuoteTargetContext = createContext<QuoteTarget>(NONE);
const Context = QuoteTargetContext;

/** Quote needs the composer: outside a conversation with one, there is nothing to offer. */
export const useQuoteTarget = (): QuoteTarget => useContext(Context);

/**
 * Quote's target is the composer of the open thread. The block is written into the box's text (`composer.setText`, so the
 * mentions follow as for any edit) and nothing is sent. `readOnly` (a thread the person may only read) offers nothing.
 */
export function QuoteProvider({
  composerRef,
  readOnly,
  children,
}: {
  composerRef: RefObject<HTMLTextAreaElement | null>;
  readOnly: boolean;
  children: ReactNode;
}) {
  const aui = useAui();
  const insert = useCallback(
    (block: string) => {
      const box = composerRef.current;
      const current = aui.composer().getState().text;
      const caret = box && box.value === current ? box.selectionEnd : null;
      const next = insertQuote(current, caret, block);
      aui.composer().setText(next.text);
      // after the box has taken the text: the caret goes after the quote, ready for the person's own words
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          const field = composerRef.current;
          if (!field) return;
          field.focus();
          field.setSelectionRange(next.caret, next.caret);
        }),
      );
    },
    [aui, composerRef],
  );
  const value = useMemo<QuoteTarget>(() => ({ available: !readOnly, insert }), [readOnly, insert]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/**
 * Quote, under a message (the person's own and an agent's turn): puts the selection, or the whole message when nothing in it is
 * selected, into the composer as a block quote, and leaves the person in the box to write under it. The whole message is its
 * source text, the Markdown as it was written; a selection is the text as the page shows it, in a fenced block when it is inside
 * a code block. A selection across two messages quotes nothing and says why.
 */
export function QuoteButton({ source, className }: { source: string; className?: string }) {
  const target = useQuoteTarget();
  const button = useRef<HTMLButtonElement>(null);
  const [said, setSaid] = useState<string | null>(null);
  // a note about the last press is not about the next selection
  useEffect(() => {
    if (said === null) return;
    const clear = () => setSaid(null);
    document.addEventListener("selectionchange", clear);
    return () => document.removeEventListener("selectionchange", clear);
  }, [said]);
  if (!target.available || source.trim() === "") return null;

  const quote = () => {
    const root = button.current?.closest(MESSAGE_ROOT);
    const picked = root
      ? pickSelection(root, window.getSelection(), source)
      : { kind: "none" as const };
    if (picked.kind === "across") {
      setSaid(QUOTE_ACROSS);
      return;
    }
    const words =
      picked.kind === "text"
        ? picked.text
        : picked.kind === "code"
          ? picked.code
          : picked.kind === "math"
            ? (picked.source ?? source)
            : source;
    const limited = limitQuote(words);
    setSaid(limited.cut ? QUOTE_CUT : null);
    target.insert(
      picked.kind === "code" ? quoteCode(limited.text, picked.language) : quoteBlock(limited.text),
    );
  };

  return (
    <>
      <TooltipIconButton
        ref={button}
        tooltip="Quote in your reply"
        aria-label="Quote"
        data-slot="quote"
        side="bottom"
        className={cn("size-7 rounded-md p-0 [&_svg]:size-3.5", className)}
        // keeps the selection: a press on the button must not clear what it is about to quote
        onMouseDown={(event) => event.preventDefault()}
        onClick={quote}
      >
        <QuoteIcon aria-hidden="true" />
      </TooltipIconButton>
      <span
        aria-live="polite"
        data-slot="quote-note"
        className={said ? "ms-1 text-xs text-muted-foreground" : "sr-only"}
      >
        {said}
      </span>
    </>
  );
}
