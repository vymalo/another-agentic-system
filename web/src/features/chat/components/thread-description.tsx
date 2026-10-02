"use client";

import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { ThreadDescriber } from "@/features/chat/hooks/use-describe-thread";
import { MORE } from "./parts/expandable-text";

/** The longest description the API takes (`ThreadPatch.description`). */
export const DESCRIPTION_MAX = 500;

/**
 * The thread's description under its header: one muted line, with a control to read all of it when
 * the line cuts it. It comes from a model, so it is untrusted: it is a text node and nothing here
 * parses it, Markdown included (`**x**` is shown as typed).
 *
 * The control exists only while the text does not fit its line, which is measured (a window resized
 * or a phone turned over changes it); an open description keeps its control to close it again.
 * It is a button with `aria-expanded`, so Enter and Space work, and the whole text is in the page
 * for a screen reader either way.
 */
export function ThreadDescription({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const [cut, setCut] = useState(false);
  const line = useRef<HTMLParagraphElement | null>(null);
  const id = useId();

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new text is measured again
  useEffect(() => {
    const el = line.current;
    if (!el || open) return; // an open line is as wide as its text: it says nothing about the cut
    const measure = () => setCut(el.scrollWidth > el.clientWidth + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [text, open]);

  return (
    <div data-slot="thread-description" className="flex items-start gap-2 text-[0.8125rem]">
      <p
        ref={line}
        id={id}
        className={
          open
            ? "min-w-0 flex-1 py-0.5 break-words text-muted-foreground"
            : "min-w-0 flex-1 truncate py-0.5 text-muted-foreground"
        }
      >
        {text}
      </p>
      {cut || open ? (
        <Button
          type="button"
          variant="link"
          size="xs"
          className={`${MORE} min-h-6 shrink-0 px-1`}
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setOpen((o) => !o)}
        >
          {open ? "Show less" : "Show more"}
        </Button>
      ) : null}
    </div>
  );
}

/** The description as a field while a person writes it: Enter or leaving it saves, Escape gives it up. */
export function DescriptionField({ describer }: { describer: ThreadDescriber }) {
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      describer.save();
    } else if (event.key === "Escape") {
      event.preventDefault();
      describer.cancel();
    }
  };
  return (
    <Input
      ref={describer.field}
      value={describer.draft ?? ""}
      onChange={(event) => describer.change(event.target.value)}
      onKeyDown={onKeyDown}
      onBlur={describer.save}
      maxLength={DESCRIPTION_MAX}
      disabled={describer.saving}
      placeholder="What this chat is about. Empty clears it."
      aria-label="Thread description"
      aria-invalid={describer.error !== null}
      className="h-8 text-[0.8125rem]"
    />
  );
}
