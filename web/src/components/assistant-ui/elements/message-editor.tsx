"use client";

import { type FC, type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";

type Props = {
  /** What the message says now: the editor opens on it, the caret at its end. */
  initialText: string;
  /** The new branch is being made: Send waits. */
  busy: boolean;
  /** Send the new words (never empty), as typed. */
  onSend: (text: string) => void;
  /** Leave the message as it is. */
  onCancel: () => void;
};

/**
 * The inline editor of a message of the person (ADR 0029: saying it again makes a new branch of
 * the conversation, the old one stays). It takes the bubble's place, with the focus in it.
 *
 * Keys: Escape cancels, Ctrl or ⌘ with Enter sends; a plain Enter is a new line, as in a text box
 * (the composer's Enter sends because it is one line most of the time; here the person is
 * rewriting). The buttons do the same, and the line under the field says the keys. A message
 * that is empty is not sent. An input method's composing Enter is left alone.
 */
export const MessageEditor: FC<Props> = ({ initialText, busy, onSend, onCancel }) => {
  const [text, setText] = useState(initialText);
  const field = useRef<HTMLTextAreaElement>(null);
  const hintId = useId();
  const canSend = !busy && text.trim() !== "";

  // opens with the focus in the field and the caret after the last word
  useEffect(() => {
    const el = field.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, []);
  // grows with the text (a long message is not a scroll box of three lines)
  // biome-ignore lint/correctness/useExhaustiveDependencies: `text` is what changes the height
  useEffect(() => {
    const el = field.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  const send = () => {
    if (canSend) onSend(text);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      send();
    }
  };

  return (
    <form
      data-slot="message-editor"
      className="flex w-full min-w-0 flex-col gap-2 rounded-[20px] border border-input bg-card p-2 shadow-composer focus-within:border-ring/60 focus-within:ring-4 focus-within:ring-ring/15 sm:max-w-[80%]"
      onSubmit={(e) => {
        e.preventDefault();
        send();
      }}
    >
      <textarea
        ref={field}
        aria-label="What you said"
        aria-describedby={hintId}
        value={text}
        rows={2}
        readOnly={busy}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        className="max-h-80 min-h-14 w-full min-w-0 resize-none bg-transparent px-2 pt-1.5 pb-0.5 text-[0.9375rem] leading-6 outline-none"
      />
      <div className="flex items-center justify-end gap-2">
        <p id={hintId} className="me-auto ps-2 text-xs text-muted-foreground max-sm:sr-only">
          Esc cancels · Ctrl/⌘ Enter sends
        </p>
        <Button
          type="button"
          variant="ghost"
          className="h-9 rounded-full px-4"
          disabled={busy}
          onClick={onCancel}
        >
          Cancel
        </Button>
        <Button type="submit" className="h-9 rounded-full px-4" disabled={!canSend}>
          {busy ? "Sending…" : "Send"}
        </Button>
      </div>
    </form>
  );
};
