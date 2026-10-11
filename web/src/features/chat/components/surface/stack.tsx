"use client";

import { Children, type ReactNode } from "react";

/*
 * The `Stack` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): its children in a row (the default)
 * or one under another. A row wraps on a narrow screen unless it says `wrap: false`, so a phone never scrolls the page
 * sideways. The children are drawn by the surface, in the order the agent gave them; each one may shrink to the width
 * it has, and its own long text or code scrolls in its own box.
 */

type Rec = Record<string, unknown>;

const GAP = { none: "gap-0", small: "gap-2", medium: "gap-4", large: "gap-6" } as const;
const ALIGN = {
  start: "items-start",
  center: "items-center",
  end: "items-end",
  stretch: "items-stretch",
} as const;
const JUSTIFY = {
  start: "justify-start",
  center: "justify-center",
  end: "justify-end",
  between: "justify-between",
} as const;

/** The class of a value the schema allows; the fallback when the value is absent or not one of its words. */
function pick(table: Readonly<Record<string, string>>, value: unknown, fallback: string): string {
  const key = typeof value === "string" && Object.keys(table).includes(value) ? value : fallback;
  return table[key] ?? "";
}

/** The Stack of a surface; `children` are its children, already drawn. */
export function StackView(props: Rec & { children?: ReactNode }) {
  const vertical = props.direction === "vertical";
  const wrap = !vertical && props.wrap !== false;
  const grow = typeof props.weight === "number" && props.weight > 0 ? props.weight : undefined;
  return (
    <div
      data-slot="stack"
      data-direction={vertical ? "vertical" : "horizontal"}
      data-wrap={wrap ? "true" : "false"}
      data-gap={typeof props.gap === "string" ? props.gap : "medium"}
      style={grow === undefined ? undefined : { flexGrow: grow }}
      className={[
        "flex min-w-0 max-w-full",
        vertical ? "flex-col" : "flex-row",
        wrap ? "flex-wrap" : "flex-nowrap",
        pick(GAP, props.gap, "medium"),
        pick(ALIGN, props.align, "stretch"),
        pick(JUSTIFY, props.justify, "start"),
      ].join(" ")}
    >
      {Children.map(props.children, (child) => (
        <div className="min-w-0 max-w-full">{child}</div>
      ))}
    </div>
  );
}
