/**
 * What the data components of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 3) show: a table's
 * columns and rows (and how they sort), the pairs of a key-value list, and the tiles of a stats row. Pure: no
 * React, no I/O. What is read here has passed the schema; the readers are defensive all the same, because a
 * surface is untrusted input (ADR 0013).
 */

export type Cell = string | number | boolean | null;
export type SortDir = "asc" | "desc";

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const isCell = (v: unknown): v is Cell =>
  v === null || typeof v === "string" || typeof v === "number" || typeof v === "boolean";

export type Column = { key: string; label: string; align: "start" | "end" };
export type TableRow = Record<string, Cell>;
export type TableSpec = { title?: string; columns: Column[]; rows: TableRow[] };

/** The table a component says, or undefined when it is not one. */
export function readTable(c: Rec): TableSpec | undefined {
  if (!Array.isArray(c.columns) || !Array.isArray(c.rows)) return undefined;
  const columns: Column[] = [];
  for (const v of c.columns) {
    if (!isRecord(v)) return undefined;
    const key = str(v.key);
    const label = str(v.label);
    if (key === undefined || label === undefined) return undefined;
    columns.push({ key, label, align: v.align === "end" ? "end" : "start" });
  }
  // a row is a list of cells in the order of the columns (`prepare.ts` lowers the object the agent sent to this)
  const rows: TableRow[] = [];
  for (const v of c.rows) {
    if (!Array.isArray(v)) return undefined;
    const row: TableRow = {};
    columns.forEach((column, i) => {
      const x = v[i];
      if (isCell(x)) Object.defineProperty(row, column.key, { value: x, enumerable: true });
    });
    rows.push(row);
  }
  const title = str(c.title);
  return { ...(title ? { title } : {}), columns, rows };
}

/** Two cells in order: numbers by value, text (and yes or no) by its words, a cell that is missing last. */
export function compareCells(a: Cell | undefined, b: Cell | undefined, dir: SortDir): number {
  if (a === undefined || b === undefined)
    return a === undefined && b === undefined ? 0 : a === undefined ? 1 : -1;
  const sign = dir === "asc" ? 1 : -1;
  if (typeof a === "number" && typeof b === "number") return (a - b) * sign;
  return (
    String(a).localeCompare(String(b), undefined, { numeric: true, sensitivity: "base" }) * sign
  );
}

/** A row's own cell under a column key (never an inherited one: a key may be `constructor`). */
export const cellOf = (row: TableRow, key: string): Cell | undefined =>
  Object.hasOwn(row, key) ? row[key] : undefined;

/** The rows in the order of the column's cells; a missing cell stays last in either direction. */
export function sortRows(
  rows: readonly TableRow[],
  key: string | undefined,
  dir: SortDir | undefined,
): TableRow[] {
  if (key === undefined || dir === undefined) return [...rows];
  return [...rows].sort((a, b) => compareCells(cellOf(a, key), cellOf(b, key), dir));
}

/** What a cell says, in words: text as it is, a number as written, yes or no, and nothing for no value. */
export function cellText(v: Cell | undefined): string {
  if (v === undefined || v === null) return "";
  if (typeof v === "boolean") return v ? "Yes" : "No";
  return String(v);
}

export type KeyValuePair = { key: string; value: Cell };

/** The pairs of a key-value list that passed the schema, in order. */
export function readPairs(v: unknown): KeyValuePair[] {
  if (!Array.isArray(v)) return [];
  const out: KeyValuePair[] = [];
  for (const pair of v) {
    if (!isRecord(pair)) continue;
    const key = str(pair.key);
    if (key === undefined || !isCell(pair.value)) continue;
    out.push({ key, value: pair.value });
  }
  return out;
}

export type Trend = "up" | "down" | "flat";
export type StatTile = { label: string; value: string; delta?: string; trend?: Trend };

/** The tiles of a stats row that passed the schema. */
export function readTiles(v: unknown): StatTile[] {
  if (!Array.isArray(v)) return [];
  const out: StatTile[] = [];
  for (const tile of v) {
    if (!isRecord(tile)) continue;
    const label = str(tile.label);
    const value = str(tile.value);
    if (label === undefined || value === undefined) continue;
    const delta = str(tile.delta);
    const trend =
      tile.trend === "up" || tile.trend === "down" || tile.trend === "flat"
        ? tile.trend
        : undefined;
    out.push({ label, value, ...(delta ? { delta } : {}), ...(trend ? { trend } : {}) });
  }
  return out;
}

/** What a trend says in words, for a screen reader beside its arrow. */
export const TREND_WORDS: Record<Trend, string> = { up: "Up", down: "Down", flat: "Unchanged" };
