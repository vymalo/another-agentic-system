"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { truncate } from "@/features/chat/lib/findings";

/** The look of the "Show more" control of the findings and of a CI summary. */
export const MORE =
  "h-auto cursor-pointer p-0 text-xs underline underline-offset-2 hover:underline focus-visible:ring-3";

/**
 * Untrusted text cut at `limit` characters, with a control to read the rest. It is a text node:
 * React escapes it, and nothing here parses markup.
 */
export function ExpandableText({ text, limit }: { text: string; limit: number }) {
  const [open, setOpen] = useState(false);
  const preview = truncate(text, limit);
  return (
    <>
      <span className="whitespace-pre-wrap">{open ? text : preview.text}</span>
      {preview.cut ? (
        <>
          {" "}
          <Button
            type="button"
            variant="link"
            size="xs"
            className={MORE}
            aria-expanded={open}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? "Show less" : "Show more"}
          </Button>
        </>
      ) : null}
    </>
  );
}
