import { describe, expect, it } from "vitest";
import lock from "./catalog.lock.json";
import { componentNames, OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 6, group 3 (data): Table, KeyValue, Stats and Chart. Each schema on its own: valid, missing,
 * oversize and wrong types. The rules JSON Schema cannot say (matching series, unique column keys) are in
 * prepare-v6-data.test.ts.
 */

const check = (component: string, instance: unknown) =>
  compileCatalog(OWN_CATALOG.catalog).check(component, instance);
const text = (n: number) => "x".repeat(n);

describe("version 6: data", () => {
  it("is catalog version 6, and has the four components", () => {
    expect(OWN_CATALOG.version).toBe(6);
    expect(lock.version).toBe(6);
    expect(componentNames(OWN_CATALOG.catalog)).toEqual(
      expect.arrayContaining(["Table", "KeyValue", "Stats", "Chart"]),
    );
  });

  it("keeps the first sentence of each description short enough for adam's show to list it whole", () => {
    for (const name of ["Table", "KeyValue", "Stats", "Chart"]) {
      const d = (OWN_CATALOG.catalog.components[name] as { description: string }).description;
      const end = d.indexOf(". ");
      const first = end < 0 ? d : d.slice(0, end + 1);
      expect(first.length, name).toBeLessThanOrEqual(100);
    }
  });
});

describe("Table", () => {
  const table = (over: Record<string, unknown> = {}) => ({
    id: "t",
    component: "Table",
    columns: [{ key: "name", label: "Name" }],
    rows: [{ name: "api" }],
    ...over,
  });

  it("accepts a table with rows, one without, and every cell kind", () => {
    expect(check("Table", table())).toBeUndefined();
    expect(check("Table", table({ rows: [] }))).toBeUndefined();
    expect(
      check(
        "Table",
        table({
          title: "Services",
          columns: [{ key: "count", label: "Count", align: "end" }],
          rows: [{ count: 3 }, { count: -1.5 }, { count: true }, { count: null }, { count: "3" }],
        }),
      ),
    ).toBeUndefined();
  });

  it("needs its columns and its rows; a column needs a key and a label", () => {
    const { rows: _rows, ...noRows } = table();
    expect(check("Table", noRows)).toMatch(/required property "rows"/);
    expect(check("Table", table({ columns: [] }))).toMatch(/^columns: /);
    expect(check("Table", table({ columns: [{ label: "Name" }] }))).toMatch(
      /required property "key"/,
    );
    expect(check("Table", table({ columns: [{ key: "name" }] }))).toMatch(
      /required property "label"/,
    );
  });

  it("a column key is 1 to 64 letters, digits or _.:-; the label 1 to 80; the alignment start or end", () => {
    expect(check("Table", table({ columns: [{ key: text(65), label: "a" }] }))).toMatch(
      /^columns\.0\.key: /,
    );
    expect(check("Table", table({ columns: [{ key: "a b", label: "a" }] }))).toMatch(
      /^columns\.0\.key: /,
    );
    expect(check("Table", table({ columns: [{ key: "a", label: text(81) }] }))).toMatch(
      /^columns\.0\.label: /,
    );
    expect(check("Table", table({ columns: [{ key: "a", label: "a", align: "center" }] }))).toMatch(
      /^columns\.0\.align: /,
    );
  });

  it("limits: 12 columns, 200 rows, 12 cells in a row, 64 characters in a cell's key, 500 in its text", () => {
    const cols = Array.from({ length: 12 }, (_, i) => ({ key: `c${i}`, label: `C${i}` }));
    expect(check("Table", table({ columns: cols }))).toBeUndefined();
    expect(check("Table", table({ columns: [...cols, { key: "x", label: "x" }] }))).toMatch(
      /^columns: /,
    );
    const rows = Array.from({ length: 200 }, () => ({ name: "a" }));
    expect(check("Table", table({ rows }))).toBeUndefined();
    expect(check("Table", table({ rows: [...rows, { name: "a" }] }))).toMatch(/^rows: /);
    const twelve = Object.fromEntries(Array.from({ length: 12 }, (_, i) => [`k${i}`, i]));
    expect(check("Table", table({ rows: [twelve] }))).toBeUndefined();
    expect(check("Table", table({ rows: [{ ...twelve, k12: 1 }] }))).toMatch(/^rows\.0: /);
    expect(check("Table", table({ rows: [{ [text(65)]: 1 }] }))).toMatch(/^rows\.0\./);
    expect(check("Table", table({ rows: [{ name: text(500) }] }))).toBeUndefined();
    expect(check("Table", table({ rows: [{ name: text(501) }] }))).toMatch(/^rows\.0\.name: /);
  });

  it("a cell is text, a number, a yes or no, or nothing: an object or a list is refused", () => {
    expect(check("Table", table({ rows: [{ name: { path: "/x" } }] }))).toMatch(/^rows\.0\.name: /);
    expect(check("Table", table({ rows: [{ name: ["a"] }] }))).toMatch(/^rows\.0\.name: /);
  });

  it("has no member that could carry a link or an action", () => {
    expect(check("Table", table({ url: "https://example.com" }))).toMatch(/url/);
  });
});

describe("KeyValue", () => {
  const kv = (
    pairs: unknown[] = [{ key: "Region", value: "eu-west" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "k",
    component: "KeyValue",
    pairs,
    ...over,
  });

  it("accepts pairs of a text, a number, a yes or no, or nothing, with an optional title", () => {
    expect(check("KeyValue", kv())).toBeUndefined();
    expect(
      check(
        "KeyValue",
        kv(
          [
            { key: "Replicas", value: 3 },
            { key: "Public", value: false },
            { key: "Owner", value: null },
          ],
          { title: "Deployment" },
        ),
      ),
    ).toBeUndefined();
  });

  it("needs at least one pair, and each pair its key and its value", () => {
    expect(check("KeyValue", kv([]))).toMatch(/^pairs: /);
    expect(check("KeyValue", kv([{ key: "a" }]))).toMatch(/required property "value"/);
    expect(check("KeyValue", kv([{ value: "a" }]))).toMatch(/required property "key"/);
    expect(check("KeyValue", { id: "k", component: "KeyValue" })).toMatch(
      /required property "pairs"/,
    );
  });

  it("limits: 50 pairs, a key of 120 characters, a text value of 500", () => {
    const fifty = Array.from({ length: 50 }, (_, i) => ({ key: `k${i}`, value: i }));
    expect(check("KeyValue", kv(fifty))).toBeUndefined();
    expect(check("KeyValue", kv([...fifty, { key: "x", value: 1 }]))).toMatch(/^pairs: /);
    expect(check("KeyValue", kv([{ key: text(120), value: "a" }]))).toBeUndefined();
    expect(check("KeyValue", kv([{ key: text(121), value: "a" }]))).toMatch(/^pairs\.0\.key: /);
    expect(check("KeyValue", kv([{ key: "a", value: text(501) }]))).toMatch(/^pairs\.0\.value: /);
  });

  it("a value is not a binding, an object or a list", () => {
    expect(check("KeyValue", kv([{ key: "a", value: { path: "/x" } }]))).toMatch(
      /^pairs\.0\.value: /,
    );
  });
});

describe("Stats", () => {
  const stats = (
    tiles: unknown[] = [{ label: "Users", value: "1,204" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "s",
    component: "Stats",
    tiles,
    ...over,
  });

  it("accepts tiles with a label and a value, and with a change and a trend", () => {
    expect(check("Stats", stats())).toBeUndefined();
    expect(
      check(
        "Stats",
        stats(
          [
            { label: "Users", value: "1,204", delta: "+12%", trend: "up" },
            { label: "Errors", value: "3", delta: "−1", trend: "down" },
            { label: "Uptime", value: "99.9%", trend: "flat" },
          ],
          { title: "This week" },
        ),
      ),
    ).toBeUndefined();
  });

  it("needs a label and a value, and a trend is up, down or flat", () => {
    expect(check("Stats", stats([{ value: "3" }]))).toMatch(/required property "label"/);
    expect(check("Stats", stats([{ label: "a" }]))).toMatch(/required property "value"/);
    expect(check("Stats", stats([{ label: "a", value: "1", trend: "sideways" }]))).toMatch(
      /^tiles\.0\.trend: /,
    );
    expect(check("Stats", stats([{ label: "a", value: "1", trend: "Up" }]))).toMatch(
      /^tiles\.0\.trend: /,
    );
  });

  it("the value is text of 1 to 40 characters, the change 1 to 40, the label 1 to 80; 12 tiles at most", () => {
    expect(check("Stats", stats([{ label: "a", value: text(40) }]))).toBeUndefined();
    expect(check("Stats", stats([{ label: "a", value: text(41) }]))).toMatch(/^tiles\.0\.value: /);
    expect(check("Stats", stats([{ label: "a", value: "1", delta: "" }]))).toMatch(
      /^tiles\.0\.delta: /,
    );
    expect(check("Stats", stats([{ label: text(81), value: "1" }]))).toMatch(/^tiles\.0\.label: /);
    expect(check("Stats", stats([{ label: "a", value: 3 }]))).toMatch(/^tiles\.0\.value: /);
    const twelve = Array.from({ length: 12 }, (_, i) => ({ label: `t${i}`, value: "1" }));
    expect(check("Stats", stats(twelve))).toBeUndefined();
    expect(check("Stats", stats([...twelve, { label: "x", value: "1" }]))).toMatch(/^tiles: /);
  });
});

describe("Chart", () => {
  const chart = (over: Record<string, unknown> = {}) => ({
    id: "c",
    component: "Chart",
    type: "bar",
    labels: ["Mon", "Tue", "Wed"],
    series: [{ name: "Runs", values: [3, 5, 2] }],
    ...over,
  });

  it("accepts a bar, a line or a pie with one to four series, and every optional member", () => {
    for (const type of ["bar", "line"])
      expect(check("Chart", chart({ type })), type).toBeUndefined();
    expect(
      check("Chart", chart({ type: "pie", series: [{ name: "Share", values: [1, 2, 3] }] })),
    ).toBeUndefined();
    expect(
      check(
        "Chart",
        chart({
          title: "Runs",
          caption: "Per day",
          unit: "runs",
          series: [1, 2, 3, 4].map((i) => ({ name: `s${i}`, values: [i, 1, 0] })),
        }),
      ),
    ).toBeUndefined();
  });

  it("needs its type, its labels and its series; the type is one of the three", () => {
    const { type: _type, ...noType } = chart();
    expect(check("Chart", noType)).toMatch(/required property "type"/);
    expect(check("Chart", chart({ type: "area" }))).toMatch(/^type: /);
    expect(check("Chart", chart({ labels: [] }))).toMatch(/^labels: /);
    expect(check("Chart", chart({ series: [] }))).toMatch(/^series: /);
    expect(check("Chart", chart({ series: [{ name: "s" }] }))).toMatch(
      /required property "values"/,
    );
  });

  it("limits: 24 labels of 40 characters, 4 series of 60, 24 values of at most a trillion, a unit of 20", () => {
    expect(check("Chart", chart({ labels: Array(24).fill("a") }))).toBeUndefined();
    expect(check("Chart", chart({ labels: Array(25).fill("a") }))).toMatch(/^labels: /);
    expect(check("Chart", chart({ labels: [text(41)] }))).toMatch(/^labels\.0: /);
    expect(check("Chart", chart({ series: [{ name: text(61), values: [1, 1, 1] }] }))).toMatch(
      /^series\.0\.name: /,
    );
    expect(check("Chart", chart({ series: [{ name: "s", values: Array(25).fill(1) }] }))).toMatch(
      /^series\.0\.values: /,
    );
    expect(
      check("Chart", chart({ series: [{ name: "s", values: [1e12, 1, 1] }] })),
    ).toBeUndefined();
    expect(check("Chart", chart({ series: [{ name: "s", values: [1e13, 1, 1] }] }))).toMatch(
      /^series\.0\.values\.0: /,
    );
    expect(check("Chart", chart({ unit: text(21) }))).toMatch(/^unit: /);
    expect(check("Chart", chart({ caption: text(501) }))).toMatch(/^caption: /);
  });

  it("values are numbers, not text, and never a binding", () => {
    expect(check("Chart", chart({ series: [{ name: "s", values: ["3", 1, 1] }] }))).toMatch(
      /^series\.0\.values\.0: /,
    );
    expect(
      check("Chart", chart({ series: [{ name: "s", values: [{ path: "/x" }, 1, 1] }] })),
    ).toMatch(/^series\.0\.values\.0: /);
  });
});
