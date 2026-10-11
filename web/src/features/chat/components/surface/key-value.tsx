"use client";

import type { ReactNode } from "react";
import { cellText, readPairs } from "@/features/chat/lib/a2ui/data";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";

/*
 * The `KeyValue` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3): up to fifty pairs
 * of a name and a value, in order, as a definition list. A yes or no is written as one; a value that is not set
 * says so in words. Every string is the agent's, drawn as text.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The KeyValue of a surface; the props are those of the lowered component (`prepare.ts`). */
export function KeyValueView(props: Rec): ReactNode {
  const pairs = readPairs(props.pairs);
  const title = plainName(str(props.title) ?? "", 120);
  return (
    <div data-slot="key-value" className="flex min-w-0 flex-col gap-2">
      {title ? <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p> : null}
      <dl className="m-0 grid min-w-0 grid-cols-[minmax(0,auto)_minmax(0,1fr)] gap-x-4 gap-y-2 text-sm">
        {pairs.map((pair, i) => (
          // the pairs are written once and never reordered: the position is the identity of a pair
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          <div key={i} className="contents">
            <dt className="text-muted-foreground [overflow-wrap:anywhere]">{pair.key}</dt>
            <dd
              className={cn(
                "m-0 min-w-0 [overflow-wrap:anywhere]",
                (pair.value === null || pair.value === "") && "text-muted-foreground italic",
              )}
            >
              {pair.value === null ? "Not set" : cellText(pair.value)}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}
