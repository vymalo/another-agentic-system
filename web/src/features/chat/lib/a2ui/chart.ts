/**
 * The chart of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3): what a Chart says, the ticks of its
 * value axis, and the geometry of a pie. Pure: no React, no I/O. The SVG is drawn by `components/surface/chart.tsx`
 * from these numbers; the numbers are the agent's and are never markup.
 */

export type ChartType = "bar" | "line" | "pie";
export type Series = { name: string; values: number[] };
export type ChartSpec = {
  type: ChartType;
  title?: string;
  caption?: string;
  unit?: string;
  labels: string[];
  series: Series[];
};

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const isType = (v: unknown): v is ChartType => v === "bar" || v === "line" || v === "pie";

/** The chart a component says, or undefined when it is not one (or its series do not match its labels). */
export function readChart(c: Rec): ChartSpec | undefined {
  if (!isType(c.type) || !Array.isArray(c.labels) || !Array.isArray(c.series)) return undefined;
  const labels = c.labels.map((l) => (typeof l === "string" ? l : undefined));
  if (labels.some((l) => l === undefined)) return undefined;
  const series: Series[] = [];
  for (const s of c.series) {
    if (!isRecord(s)) return undefined;
    const name = str(s.name);
    if (name === undefined || !Array.isArray(s.values)) return undefined;
    const values = s.values.map((v) =>
      typeof v === "number" && Number.isFinite(v) ? v : undefined,
    );
    if (values.some((v) => v === undefined) || values.length !== labels.length) return undefined;
    series.push({ name, values: values as number[] });
  }
  const title = str(c.title);
  const caption = str(c.caption);
  const unit = str(c.unit);
  return {
    type: c.type,
    ...(title ? { title } : {}),
    ...(caption ? { caption } : {}),
    ...(unit ? { unit } : {}),
    labels: labels as string[],
    series,
  };
}

/**
 * The ticks of a value axis from at most `min` to at least `max`, at a step of 1, 2, 2.5, 5 or 10 times a power of
 * ten (the first that gives about `count` steps). Zero is always a tick when the range holds it.
 */
export function niceTicks(min: number, max: number, count = 4): number[] {
  const lo = Math.min(min, 0);
  const hi = Math.max(max, 0);
  const span = hi - lo || 1;
  const raw = span / count;
  const pow = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * pow).find((s) => s >= raw) ?? 10 * pow;
  const first = Math.floor(lo / step) * step;
  const ticks: number[] = [];
  for (let t = first; ; t += step) {
    const v = Number(t.toPrecision(12));
    ticks.push(v);
    if (v >= hi) break;
  }
  return ticks;
}

export type Slice = { start: number; end: number; fraction: number };

/**
 * The slices of a pie, clockwise from twelve o'clock, as fractions of a turn: their angles in degrees from the top.
 * Values below zero and a total of zero draw nothing.
 */
export function pieSlices(values: readonly number[]): Slice[] {
  const parts = values.map((v) => (v > 0 ? v : 0));
  const total = parts.reduce((a, b) => a + b, 0);
  if (total <= 0) return [];
  let angle = 0;
  return parts.map((v) => {
    const fraction = v / total;
    const slice = { start: angle, end: angle + fraction * 360, fraction };
    angle = slice.end;
    return slice;
  });
}

/** The SVG path of a pie slice of radius `r` around (cx, cy), between two angles in degrees from the top. */
export function arcPath(cx: number, cy: number, r: number, start: number, end: number): string {
  const point = (deg: number) => {
    const rad = ((deg - 90) * Math.PI) / 180;
    return [cx + r * Math.cos(rad), cy + r * Math.sin(rad)].map((n) => n.toFixed(2)).join(" ");
  };
  if (end - start >= 360) {
    // a whole circle is two half arcs: one arc cannot start where it ends
    const mid = start + 180;
    return `M ${point(start)} A ${r} ${r} 0 1 1 ${point(mid)} A ${r} ${r} 0 1 1 ${point(start)} Z`;
  }
  const large = end - start > 180 ? 1 : 0;
  return `M ${cx} ${cy} L ${point(start)} A ${r} ${r} 0 ${large} 1 ${point(end)} Z`;
}
