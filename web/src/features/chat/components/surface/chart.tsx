"use client";

import type { ReactNode } from "react";
import {
  arcPath,
  type ChartSpec,
  niceTicks,
  pieSlices,
  readChart,
} from "@/features/chat/lib/a2ui/chart";
import { plainName } from "@/features/chat/lib/files";

/*
 * The `Chart` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3): a bar, line or pie
 * chart of up to four series over labelled points, drawn as an SVG of plain shapes (no chart library is a
 * dependency of the web). The picture is one image with a name that says what it shows; its data is a table
 * behind a disclosure, and a legend names each series in words beside its colour, so no colour is needed to read
 * it. The series colours are the `--chart-*` tokens of the page, which hold their contrast in both schemes.
 * Every label and name is the agent's text, drawn as text, never as markup.
 */

type Rec = Record<string, unknown>;

const COLOR = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-6)",
] as const;
const KIND: Record<ChartSpec["type"], string> = { bar: "Bar", line: "Line", pie: "Pie" };
const W = 360;
const H = 220;
const PAD = { top: 12, right: 12, bottom: 30, left: 48 };

/** A number as a person reads it: at most two decimals, the thousands grouped. */
const number = (n: number) =>
  new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 }).format(n);
/** A category label cut for the axis; the full words are in the data table. */
const cut = (s: string, n = 8) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

function Bars({ spec, lo, hi }: { spec: ChartSpec; lo: number; hi: number }) {
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const y = (v: number) => PAD.top + plotH * (1 - (v - lo) / (hi - lo || 1));
  const band = plotW / spec.labels.length;
  const width = (band * 0.7) / spec.series.length;
  return (
    <>
      {spec.series.map((s, i) =>
        s.values.map((v, j) => {
          const top = y(Math.max(v, 0));
          const bottom = y(Math.min(v, 0));
          return (
            <rect
              // the bars are drawn once, in their order: the series and the point are their identity
              // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
              key={`${i}-${j}`}
              x={PAD.left + j * band + band * 0.15 + i * width}
              y={top}
              width={Math.max(width - 1, 1)}
              height={Math.max(bottom - top, 0)}
              style={{ fill: COLOR[i % COLOR.length] }}
            />
          );
        }),
      )}
    </>
  );
}

function Lines({ spec, lo, hi }: { spec: ChartSpec; lo: number; hi: number }) {
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const y = (v: number) => PAD.top + plotH * (1 - (v - lo) / (hi - lo || 1));
  const band = plotW / spec.labels.length;
  const x = (j: number) => PAD.left + band * (j + 0.5);
  return (
    <>
      {spec.series.map((s, i) => (
        <g
          // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
          key={i}
          data-series={i}
        >
          <polyline
            points={s.values.map((v, j) => `${x(j).toFixed(1)},${y(v).toFixed(1)}`).join(" ")}
            fill="none"
            strokeWidth={2}
            style={{ stroke: COLOR[i % COLOR.length] }}
          />
          {s.values.map((v, j) => (
            <circle
              // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
              key={j}
              cx={x(j)}
              cy={y(v)}
              r={3}
              style={{ fill: COLOR[i % COLOR.length] }}
            />
          ))}
        </g>
      ))}
    </>
  );
}

function Pie({ spec }: { spec: ChartSpec }) {
  const values = spec.series[0]?.values ?? [];
  const slices = pieSlices(values);
  const total = values.reduce((a, b) => a + Math.max(b, 0), 0);
  const cx = 90;
  const cy = 90;
  const r = 82;
  return (
    <>
      {slices.map((slice, i) => (
        <path
          // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
          key={i}
          d={arcPath(cx, cy, r, slice.start, slice.end)}
          style={{ fill: COLOR[i % COLOR.length] }}
          stroke="var(--card)"
          strokeWidth={1.5}
        />
      ))}
      {slices.map((slice, i) =>
        slice.fraction >= 0.06 ? (
          <text
            // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
            key={`t${i}`}
            x={cx + r * 0.62 * Math.sin((((slice.start + slice.end) / 2) * Math.PI) / 180)}
            y={cy - r * 0.62 * Math.cos((((slice.start + slice.end) / 2) * Math.PI) / 180)}
            textAnchor="middle"
            dominantBaseline="middle"
            fontSize={11}
            style={{ fill: "var(--card-foreground)" }}
          >
            {`${Math.round(slice.fraction * 100)}%`}
          </text>
        ) : null,
      )}
      <title>
        {total > 0 ? `Total ${number(total)}${spec.unit ? ` ${spec.unit}` : ""}` : "No values"}
      </title>
    </>
  );
}

/** The Chart of a surface; the props are those of the lowered component (`prepare.ts`). */
export function ChartView(props: Rec): ReactNode {
  const spec = readChart(props);
  if (!spec) return null;
  const values = spec.series.flatMap((s) => s.values);
  const ticks = niceTicks(Math.min(0, ...values), Math.max(0, ...values));
  const lo = ticks[0] ?? 0;
  const hi = ticks.at(-1) ?? 0;
  const title = plainName(spec.title ?? "", 120);
  const caption = plainName(spec.caption ?? "", 500);
  const unit = spec.unit ? plainName(spec.unit, 20) : undefined;
  const n = spec.series.length;
  const m = spec.labels.length;
  const label = `${KIND[spec.type]} chart${title ? `: ${title}` : ""}. ${n === 1 ? "1 series" : `${n} series`} over ${m === 1 ? "1 point" : `${m} points`}.`;
  const band = (W - PAD.left - PAD.right) / m;
  const plotH = H - PAD.top - PAD.bottom;
  const y = (v: number) => PAD.top + plotH * (1 - (v - lo) / (hi - lo || 1));
  // a pie's legend names its slices (its labels); the other charts name their series
  const legend =
    spec.type === "pie"
      ? spec.labels.map((name, i) => ({ name, color: i }))
      : spec.series.map((s, i) => ({ name: s.name, color: i }));
  return (
    <figure data-slot="chart" data-type={spec.type} className="m-0 flex min-w-0 flex-col gap-2">
      {title ? (
        <figcaption className="text-sm font-medium [overflow-wrap:anywhere]">{title}</figcaption>
      ) : null}
      <svg
        role="img"
        aria-label={label}
        viewBox={spec.type === "pie" ? "0 0 180 180" : `0 0 ${W} ${H}`}
        className="block h-auto w-full max-w-md overflow-visible"
        style={{ color: "var(--muted-foreground)" }}
      >
        {spec.type === "pie" ? (
          <Pie spec={spec} />
        ) : (
          <>
            {ticks.map((t) => (
              <g key={t}>
                <line
                  x1={PAD.left}
                  x2={W - PAD.right}
                  y1={y(t)}
                  y2={y(t)}
                  stroke="currentColor"
                  strokeOpacity={0.2}
                />
                <text
                  x={PAD.left - 6}
                  y={y(t)}
                  textAnchor="end"
                  dominantBaseline="middle"
                  fontSize={10}
                  fill="currentColor"
                >
                  {number(t)}
                </text>
              </g>
            ))}
            {spec.type === "bar" ? (
              <Bars spec={spec} lo={lo} hi={hi} />
            ) : (
              <Lines spec={spec} lo={lo} hi={hi} />
            )}
            {spec.labels.map((l, j) => (
              <text
                // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
                key={j}
                x={PAD.left + band * (j + 0.5)}
                y={H - PAD.bottom + 14}
                textAnchor="middle"
                fontSize={10}
                fill="currentColor"
              >
                {cut(l)}
              </text>
            ))}
          </>
        )}
      </svg>
      <ul
        aria-label="Legend"
        className="m-0 flex min-w-0 list-none flex-wrap gap-x-3 gap-y-1 p-0 text-xs"
      >
        {legend.map((item) => (
          <li key={item.name + item.color} className="inline-flex min-w-0 items-center gap-1.5">
            <span
              aria-hidden="true"
              className="inline-block size-2.5 shrink-0 rounded-sm"
              style={{ backgroundColor: COLOR[item.color % COLOR.length] }}
            />
            <span className="[overflow-wrap:anywhere]">{plainName(item.name, 60)}</span>
          </li>
        ))}
      </ul>
      {caption ? (
        <p className="text-xs whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]">
          {caption}
        </p>
      ) : null}
      <details className="text-xs">
        <summary className="cursor-pointer text-muted-foreground">Data</summary>
        <div
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
          tabIndex={0}
          className="mt-2 max-w-full overflow-x-auto"
        >
          <table className="w-full border-collapse">
            <caption className="sr-only">{label}</caption>
            <thead>
              <tr className="border-b">
                <th scope="col" className="px-2 py-1 text-left font-medium">
                  {spec.type === "pie" ? "Label" : "Point"}
                </th>
                {spec.series.map((s, i) => (
                  <th
                    // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
                    key={i}
                    scope="col"
                    className="px-2 py-1 text-right font-medium [overflow-wrap:anywhere]"
                  >
                    {plainName(s.name, 60)}
                    {unit ? ` (${unit})` : ""}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {spec.labels.map((l, j) => (
                <tr
                  // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
                  key={j}
                  className="border-b last:border-b-0"
                >
                  <th
                    scope="row"
                    className="px-2 py-1 text-left font-normal [overflow-wrap:anywhere]"
                  >
                    {l}
                  </th>
                  {spec.series.map((s, i) => (
                    <td
                      // biome-ignore lint/suspicious/noArrayIndexKey: drawn once, in order, never reordered
                      key={i}
                      className="px-2 py-1 text-right tabular-nums"
                    >
                      {number(s.values[j] ?? 0)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </details>
    </figure>
  );
}
