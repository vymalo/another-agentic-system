"use client";

import { useState } from "react";
import { truncate } from "@/features/chat/lib/findings";
import { cn } from "@/lib/utils";
import { MORE } from "../parts/expandable-text";

/** A command longer than this, or of more than one line, is cut, with a control to read it all. */
export const COMMAND_PREVIEW = 96;

/**
 * A command the agent ran, in a light monospace box: one line, cut when long, with Show more.
 * It is the agent's text: drawn as text, never parsed.
 */
export function CommandText({ command }: { command: string }) {
  const [open, setOpen] = useState(false);
  const firstLine = command.split("\n", 1)[0] ?? "";
  const preview = truncate(firstLine, COMMAND_PREVIEW);
  const cut = preview.cut || firstLine.length < command.length;
  return (
    <div className="flex min-w-0 flex-col items-start gap-1">
      <code
        data-slot="command"
        className={cn(
          "block max-w-full rounded-md border bg-muted/60 px-2 py-1 font-mono text-xs leading-5 text-foreground/90",
          open ? "whitespace-pre-wrap [overflow-wrap:anywhere]" : "truncate",
        )}
      >
        <span className="select-none text-muted-foreground" aria-hidden="true">
          ${" "}
        </span>
        <span className="sr-only">Command: </span>
        {open ? command : cut ? `${preview.text.replace(/…$/, "")}…` : command}
      </code>
      {cut ? (
        <button
          type="button"
          className={cn(MORE, "text-muted-foreground")}
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
        >
          {open ? "Show less" : "Show more"}
        </button>
      ) : null}
    </div>
  );
}
