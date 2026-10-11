import { describe, expect, it } from "vitest";
import {
  type Cell,
  cellOf,
  cellText,
  compareCells,
  readPairs,
  readTable,
  readTiles,
  sortRows,
  type TableRow,
  TREND_WORDS,
} from "./data";

const ROWS: TableRow[] = [
  { name: "beta", count: 10, ok: true },
  { name: "Alpha", count: 2, ok: false },
  { name: "gamma", count: 33, ok: null },
  { name: "item 10", count: 9 },
  { name: "item 9", count: 9 },
];

describe("readTable", () => {
  it("reads the columns and the rows, each cell a text, a number, a yes or no, or nothing", () => {
    const t = readTable({
      title: "Services",
      columns: [
        { key: "name", label: "Name" },
        { key: "count", label: "Count", align: "end" },
      ],
      rows: [{ name: "api", count: 3, extra: null, flag: true }],
    });
    expect(t).toEqual({
      title: "Services",
      columns: [
        { key: "name", label: "Name", align: "start" },
        { key: "count", label: "Count", align: "end" },
      ],
      rows: [{ name: "api", count: 3, extra: null, flag: true }],
    });
  });

  it("refuses what the schema would have refused: a column without a label, a row that is not an object", () => {
    expect(readTable({ columns: [{ key: "a" }], rows: [] })).toBeUndefined();
    expect(readTable({ columns: [{ key: "a", label: "A" }], rows: ["x"] })).toBeUndefined();
    expect(readTable({ columns: "nope", rows: [] })).toBeUndefined();
  });

  it("keeps a row's key named like an inherited property as an own one", () => {
    const t = readTable({
      columns: [{ key: "constructor", label: "C" }],
      rows: [{ constructor: "x" }, {}],
    });
    expect(t?.rows[1]).toEqual({});
    expect(cellOf(t?.rows[1] ?? {}, "constructor")).toBeUndefined();
  });
});

describe("sortRows", () => {
  it("keeps the order the rows came in when no column is sorted", () => {
    expect(sortRows(ROWS, undefined, undefined).map((r) => r.name)).toEqual(
      ROWS.map((r) => r.name),
    );
  });

  it("sorts numbers by value, both ways", () => {
    expect(sortRows(ROWS, "count", "asc").map((r) => r.count)).toEqual([2, 9, 9, 10, 33]);
    expect(sortRows(ROWS, "count", "desc").map((r) => r.count)).toEqual([33, 10, 9, 9, 2]);
  });

  it("sorts text by its words, with numbers inside it in their order, and without regard to case", () => {
    expect(sortRows(ROWS, "name", "asc").map((r) => r.name)).toEqual([
      "Alpha",
      "beta",
      "gamma",
      "item 9",
      "item 10",
    ]);
  });

  it("keeps a missing cell last in either direction", () => {
    const withGap: TableRow[] = [{ v: 2 }, {}, { v: 1 }];
    expect(sortRows(withGap, "v", "asc").map((r) => r.v)).toEqual([1, 2, undefined]);
    expect(sortRows(withGap, "v", "desc").map((r) => r.v)).toEqual([2, 1, undefined]);
  });

  it("does not change the rows it was given", () => {
    const before = ROWS.map((r) => r.name);
    sortRows(ROWS, "count", "desc");
    expect(ROWS.map((r) => r.name)).toEqual(before);
  });
});

describe("compareCells", () => {
  it("puts a missing cell after every other whichever way the column is sorted", () => {
    expect(compareCells(undefined, 1, "asc")).toBeGreaterThan(0);
    expect(compareCells(undefined, 1, "desc")).toBeGreaterThan(0);
    expect(compareCells(1, undefined, "desc")).toBeLessThan(0);
    expect(compareCells(undefined, undefined, "asc")).toBe(0);
  });
});

describe("cellText", () => {
  it("says a cell in words: yes or no for a boolean, the number as written, nothing for no value", () => {
    const cases: [Cell | undefined, string][] = [
      ["text", "text"],
      [42, "42"],
      [true, "Yes"],
      [false, "No"],
      [null, ""],
      [undefined, ""],
    ];
    for (const [value, said] of cases) expect(cellText(value), String(value)).toBe(said);
  });
});

describe("readPairs and readTiles", () => {
  it("reads each pair's key and value, in order, and skips what does not fit", () => {
    expect(
      readPairs([
        { key: "Region", value: "eu-west" },
        { key: "Replicas", value: 3 },
        { key: 1, value: 2 },
      ]),
    ).toEqual([
      { key: "Region", value: "eu-west" },
      { key: "Replicas", value: 3 },
    ]);
    expect(readPairs("nope")).toEqual([]);
  });

  it("reads each tile's label, value, change and trend, and a trend that is not one of the three is none", () => {
    expect(
      readTiles([
        { label: "Users", value: "1,204", delta: "+12%", trend: "up" },
        { label: "Errors", value: "3", trend: "sideways" },
        { value: "no label" },
      ]),
    ).toEqual([
      { label: "Users", value: "1,204", delta: "+12%", trend: "up" },
      { label: "Errors", value: "3" },
    ]);
  });

  it("says each trend in words", () => {
    expect(TREND_WORDS).toEqual({ up: "Up", down: "Down", flat: "Unchanged" });
  });
});
