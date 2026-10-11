"use client";

import { ChevronDownIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";

/*
 * The `Accordion` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): up to twenty sections,
 * each a title and a child component of the surface, closed until its title is pressed. One section is open at
 * a time unless the component says `multiple`. A title is a button that says whether its section is open
 * (aria-expanded); its child is not in the page while the section is closed.
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const toArray = (v: ReactNode): ReactNode[] => (Array.isArray(v) ? v : v === undefined ? [] : [v]);

/** The Accordion of a surface; the props are those of the lowered component (`prepare.ts`). */
export function AccordionView(props: Rec & { children?: ReactNode }): ReactNode {
  const titles = (Array.isArray(props.items) ? props.items : []).map((item) =>
    plainName(isRecord(item) ? (str(item.title) ?? "") : "", 200),
  );
  const panes = toArray(props.children);
  const multiple = props.multiple === true;
  const [open, setOpen] = useState<ReadonlySet<number>>(() => new Set());
  const toggle = (i: number) =>
    setOpen((prev) => {
      if (prev.has(i)) return new Set(multiple ? [...prev].filter((x) => x !== i) : []);
      return new Set(multiple ? [...prev, i] : [i]);
    });
  return (
    <div data-slot="accordion" className="flex min-w-0 flex-col divide-y rounded-xl border px-3.5">
      {titles.map((title, i) => (
        <Collapsible
          // a section is named by its place: two sections may have the same title
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          key={i}
          open={open.has(i)}
          onOpenChange={() => toggle(i)}
          className="min-w-0"
        >
          <CollapsibleTrigger className="flex w-full min-w-0 items-center justify-between gap-3 py-3 text-left text-sm font-medium [overflow-wrap:anywhere] outline-none focus-visible:rounded-sm focus-visible:ring-3 focus-visible:ring-ring/50">
            {title}
            <ChevronDownIcon
              aria-hidden="true"
              className={cn(
                "size-4 shrink-0 text-muted-foreground transition-transform",
                open.has(i) && "rotate-180",
              )}
            />
          </CollapsibleTrigger>
          <CollapsibleContent className="pb-3">{panes[i]}</CollapsibleContent>
        </Collapsible>
      ))}
    </div>
  );
}
