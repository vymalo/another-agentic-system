"use client";

import type { ReactNode } from "react";
import { useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { plainName } from "@/features/chat/lib/files";

/*
 * The `Tabs` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): up to eight tabs, each a label
 * and a child component of the surface. Only the open tab's child is drawn. The tabs follow the WAI-ARIA tabs
 * pattern (Radix): the arrow keys move between them, Home and End go to the first and the last, and the open
 * one is the one that is selected. The labels are the agent's text, drawn as text.
 */

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const toArray = (v: ReactNode): ReactNode[] => (Array.isArray(v) ? v : v === undefined ? [] : [v]);

/** The Tabs of a surface; the props are those of the lowered component (`prepare.ts`), `children` its tabs' children. */
export function TabsView(props: Rec & { children?: ReactNode }): ReactNode {
  const labels = (Array.isArray(props.tabs) ? props.tabs : []).map((tab) =>
    plainName(isRecord(tab) ? (str(tab.label) ?? "") : "", 60),
  );
  const panes = toArray(props.children);
  const [value, setValue] = useState("0");
  return (
    <Tabs value={value} onValueChange={setValue} data-slot="tabs" className="min-w-0 gap-3">
      <TabsList className="max-w-full justify-start gap-5 overflow-x-auto">
        {labels.map((label, i) => (
          <TabsTrigger
            // a tab is named by its place in the list: two tabs may have the same words
            // biome-ignore lint/suspicious/noArrayIndexKey: see above
            key={i}
            value={String(i)}
            className="min-h-9 max-w-full whitespace-normal [overflow-wrap:anywhere]"
          >
            {label}
          </TabsTrigger>
        ))}
      </TabsList>
      {labels.map((_, i) => (
        <TabsContent
          // biome-ignore lint/suspicious/noArrayIndexKey: see the tab above
          key={i}
          value={String(i)}
          className="min-w-0"
        >
          {panes[i]}
        </TabsContent>
      ))}
    </Tabs>
  );
}
