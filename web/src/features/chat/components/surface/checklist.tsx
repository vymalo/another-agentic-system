"use client";

import type { ReactNode } from "react";
import { progressOf, readItems, STATE_WORDS } from "@/features/chat/lib/a2ui/status";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";
import { StateIcon } from "./status-icon";

/*
 * The `Checklist` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): up to fifty items, each a
 * label and a state (done, doing, todo or failed), with a progress bar and a count. Each state is said in words
 * beside its icon; the bar is a progressbar with its value in words. Output only.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The Checklist of a surface; the props are those of the lowered component (`prepare.ts`). */
export function ChecklistView(props: Rec): ReactNode {
  const items = readItems(props.items);
  const { done, total, percent } = progressOf(items);
  const title = plainName(str(props.title) ?? "", 120);
  return (
    <div data-slot="checklist" className="flex min-w-0 flex-col gap-3">
      {title ? <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p> : null}
      <div className="flex min-w-0 items-center gap-3">
        <div
          role="progressbar"
          aria-label="Progress"
          aria-valuemin={0}
          aria-valuemax={total}
          aria-valuenow={done}
          aria-valuetext={`${done} of ${total} done`}
          className="h-2 min-w-0 flex-1 overflow-hidden rounded-full bg-muted"
        >
          <div className="h-full rounded-full bg-brand" style={{ width: `${percent}%` }} />
        </div>
        <p className="shrink-0 text-xs text-muted-foreground tabular-nums">{`${done} of ${total} done`}</p>
      </div>
      <ul
        aria-label={title || "Checklist"}
        className="m-0 flex min-w-0 list-none flex-col gap-2 p-0"
      >
        {items.map((item, i) => (
          // the items are written once and never reordered: the position is the identity of a row
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          <li key={i} data-state={item.state} className="flex min-w-0 items-start gap-2.5 text-sm">
            <StateIcon state={item.state} className="mt-0.5" />
            <span
              className={cn(
                "min-w-0 flex-1 [overflow-wrap:anywhere]",
                item.state === "done" && "text-muted-foreground line-through",
              )}
            >
              {item.label}
            </span>
            <span className="shrink-0 text-xs text-muted-foreground">
              {STATE_WORDS[item.state]}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
