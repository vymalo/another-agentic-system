"use client";

import { ArrowDownIcon, ArrowUpDownIcon, ArrowUpIcon } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { cellOf, cellText, readTable, type SortDir, sortRows } from "@/features/chat/lib/a2ui/data";
import { cn } from "@/lib/utils";

/*
 * The `Table` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3): up to twelve columns
 * and two hundred rows. A click on a column's header sorts the rows by it, ascending, then descending, then as
 * they came; the header says how it is sorted in words (aria-sort, and the words beside its arrow). The table is
 * a scrollable region that a keyboard reaches. Every cell is the agent's text, drawn as text.
 */

type Rec = Record<string, unknown>;
type Sort = { key: string; dir: SortDir } | undefined;

const SORT_WORDS: Record<SortDir, string> = {
  asc: ", sorted ascending",
  desc: ", sorted descending",
};
const ARIA_SORT: Record<SortDir, "ascending" | "descending"> = {
  asc: "ascending",
  desc: "descending",
};

/** The next sort of a column that was clicked: none to ascending, ascending to descending, descending to none. */
function nextSort(current: Sort, key: string): Sort {
  if (current?.key !== key) return { key, dir: "asc" };
  if (current.dir === "asc") return { key, dir: "desc" };
  return undefined;
}

/** The Table of a surface; the props are those of the lowered component (`prepare.ts`). */
export function TableView(props: Rec): ReactNode {
  const spec = readTable(props);
  const [sort, setSort] = useState<Sort>(undefined);
  const rows = useMemo(() => sortRows(spec?.rows ?? [], sort?.key, sort?.dir), [spec, sort]);
  if (!spec) return null;
  return (
    // biome-ignore lint/a11y/useSemanticElements: a scrollable box, not a section of the page
    <div
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
      tabIndex={0}
      role="region"
      aria-label={spec.title || "Table"}
      data-slot="table"
      className="max-w-full min-w-0 overflow-x-auto rounded-xl border"
    >
      <table className="w-full caption-bottom border-collapse text-sm">
        {spec.title ? (
          <caption className="border-b px-3 py-2 text-left text-sm font-medium [overflow-wrap:anywhere]">
            {spec.title}
          </caption>
        ) : null}
        <thead>
          <tr className="border-b">
            {spec.columns.map((column) => {
              const active = sort?.key === column.key ? sort.dir : undefined;
              const Icon =
                active === undefined
                  ? ArrowUpDownIcon
                  : active === "asc"
                    ? ArrowUpIcon
                    : ArrowDownIcon;
              return (
                <th
                  key={column.key}
                  scope="col"
                  aria-sort={active === undefined ? "none" : ARIA_SORT[active]}
                  className={cn(
                    "px-3 py-2 font-medium text-muted-foreground",
                    column.align === "end" ? "text-right" : "text-left",
                  )}
                >
                  <button
                    type="button"
                    onClick={() => setSort((s) => nextSort(s, column.key))}
                    className="inline-flex max-w-full cursor-pointer items-center gap-1 rounded-sm text-left font-medium text-foreground outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
                  >
                    <span className="[overflow-wrap:anywhere]">{column.label}</span>
                    <Icon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
                    {active === undefined ? null : (
                      <span className="sr-only">{SORT_WORDS[active]}</span>
                    )}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            // the rows are the agent's and a sort may move them: a row is named by where it came from
            // biome-ignore lint/suspicious/noArrayIndexKey: see above
            <tr key={i} className="border-b last:border-b-0">
              {spec.columns.map((column) => (
                <td
                  key={column.key}
                  className={cn(
                    "px-3 py-2 align-top [overflow-wrap:anywhere]",
                    column.align === "end" && "text-right tabular-nums",
                  )}
                >
                  {cellText(cellOf(row, column.key))}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
