"use client";

import { ArrowDownIcon, ArrowUpIcon, MinusIcon } from "lucide-react";
import type { ReactNode } from "react";
import { readTiles, TREND_WORDS, type Trend } from "@/features/chat/lib/a2ui/data";
import { plainName } from "@/features/chat/lib/files";

/*
 * The `Stats` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3): up to twelve tiles, each
 * a label and a value, with an optional change and trend. The trend is an arrow and its word ("Up", "Down",
 * "Unchanged"), never a colour; the change is the agent's text. Output only.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const ARROW: Record<Trend, typeof ArrowUpIcon> = {
  up: ArrowUpIcon,
  down: ArrowDownIcon,
  flat: MinusIcon,
};

/** The Stats of a surface; the props are those of the lowered component (`prepare.ts`). */
export function StatsView(props: Rec): ReactNode {
  const tiles = readTiles(props.tiles);
  const title = plainName(str(props.title) ?? "", 120);
  return (
    <div data-slot="stats" className="flex min-w-0 flex-col gap-2">
      {title ? <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p> : null}
      <ul
        aria-label={title || "Figures"}
        className="m-0 grid min-w-0 list-none grid-cols-[repeat(auto-fit,minmax(8.5rem,1fr))] gap-2 p-0"
      >
        {tiles.map((tile, i) => {
          const Arrow = tile.trend ? ARROW[tile.trend] : undefined;
          return (
            <li
              // the tiles are written once and never reordered: the position is the identity of a tile
              // biome-ignore lint/suspicious/noArrayIndexKey: see above
              key={i}
              data-slot="stat"
              className="flex min-w-0 flex-col gap-1 rounded-xl border bg-card px-3.5 py-3"
            >
              <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{tile.label}</p>
              <p className="text-xl font-semibold tabular-nums [overflow-wrap:anywhere]">
                {tile.value}
              </p>
              {tile.delta || Arrow ? (
                <p className="inline-flex min-w-0 flex-wrap items-center gap-1 text-xs text-muted-foreground">
                  {Arrow && tile.trend ? (
                    <>
                      <Arrow aria-hidden="true" className="size-3.5 shrink-0" />
                      <span className="sr-only">{`${TREND_WORDS[tile.trend]}: `}</span>
                    </>
                  ) : null}
                  {tile.delta ? (
                    <span className="tabular-nums [overflow-wrap:anywhere]">{tile.delta}</span>
                  ) : null}
                </p>
              ) : null}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
