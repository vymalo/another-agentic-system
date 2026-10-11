"use client";

import type { ReactNode } from "react";
import { type DiffKind, diffLines, visibleControls } from "@/features/chat/lib/a2ui/code";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";

/*
 * The `Diff` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a unified diff, its file
 * headers and hunks in muted type, its added and removed lines tinted. A line is never told apart by its
 * tint alone: its sign is printed, and a screen reader hears "Added:" or "Removed:" before its text.
 * The lines are React text, in a box that scrolls and is reachable by keyboard.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const ROW: Record<DiffKind, string> = {
  add: "bg-success/15",
  remove: "bg-destructive/15",
  hunk: "bg-muted text-muted-foreground",
  header: "font-medium text-muted-foreground",
  context: "",
};
const SIGN: Record<DiffKind, string> = { add: "+", remove: "-", hunk: "", header: "", context: "" };
const SAID: Partial<Record<DiffKind, string>> = { add: "Added: ", remove: "Removed: " };

/** The Diff of a surface; the props are those of the lowered component (`prepare.ts`). */
export function DiffView(props: Rec): ReactNode {
  const diff = visibleControls(str(props.diff) ?? "");
  const title = plainName(str(props.title) ?? "", 120);
  const lines = diffLines(diff);
  return (
    <figure data-slot="diff" className="m-0 flex min-w-0 flex-col gap-1.5">
      {title ? (
        <figcaption className="text-sm font-medium [overflow-wrap:anywhere]">{title}</figcaption>
      ) : null}
      {/* biome-ignore lint/a11y/useSemanticElements: a scrollable box, not a section of the page */}
      <div
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
        tabIndex={0}
        role="region"
        aria-label={title || "Diff"}
        className="max-h-96 min-w-0 overflow-auto rounded-xl border bg-card py-1 font-mono text-xs leading-relaxed"
      >
        {lines.map((line, i) => (
          <div
            // a diff is written once and never reordered: the position is the identity of a line
            // biome-ignore lint/suspicious/noArrayIndexKey: see above
            key={i}
            data-kind={line.kind}
            className={cn("flex min-w-0 px-3 whitespace-pre", ROW[line.kind])}
          >
            <span aria-hidden="true" className="w-4 shrink-0 select-none">
              {SIGN[line.kind]}
            </span>
            {SAID[line.kind] ? <span className="sr-only">{SAID[line.kind]}</span> : null}
            <span className="min-w-0">{line.text}</span>
          </div>
        ))}
      </div>
    </figure>
  );
}
