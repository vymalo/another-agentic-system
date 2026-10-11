import { describe, expect, it } from "vitest";
import { type Prepared, prepareSurface } from "./prepare";
import { BASIC_CATALOG, OWN_CATALOG_ID, surface } from "./testing";

/**
 * Catalog version 6, group 3 (data), through the validator: the rules JSON Schema cannot say (a column's key used
 * once; a series with a value per label; a pie of one series of values, none negative, one above zero) and the
 * names the renderer keeps. The schemas are in catalog/v6-data.test.ts.
 */

type Rec = Record<string, unknown>;
const refused = (p: Prepared) => {
  if (p.kind !== "refused") throw new Error(`expected a refusal, got ${p.kind}`);
  return p;
};
const ours = (components: Rec[], catalogId = OWN_CATALOG_ID) =>
  surface(components, undefined, "v0.9.1", catalogId);
const root = (children: string[]): Rec => ({ id: "root", component: "Column", children });

const table = (columns: Rec[] = [{ key: "name", label: "Name" }]): Rec => ({
  id: "t",
  component: "Table",
  columns,
  rows: [{ name: "api" }],
});
const chart = (over: Rec = {}): Rec => ({
  id: "c",
  component: "Chart",
  type: "bar",
  labels: ["Mon", "Tue"],
  series: [{ name: "Runs", values: [3, 5] }],
  ...over,
});

describe("Table, KeyValue, Stats and Chart, in a surface of our catalog", () => {
  it("are drawn, and lowered to the names the renderer keeps", () => {
    const p = prepareSurface(
      ours([
        root(["t", "kv", "s", "c"]),
        table(),
        { id: "kv", component: "KeyValue", pairs: [{ key: "Region", value: "eu-west" }] },
        { id: "s", component: "Stats", tiles: [{ label: "Users", value: "1,204", trend: "up" }] },
        chart(),
      ]),
    );
    expect(p.kind).toBe("surface");
    const spec = JSON.stringify(p.kind === "surface" ? p.spec : {});
    for (const name of ["Table", "KeyValue", "Stats", "Chart"]) {
      expect(spec, name).toContain(`"vymalo.${name}"`);
    }
  });

  it("a Table's column keys are used once: a key twice is refused (rule schema)", () => {
    const bad = refused(
      prepareSurface(
        ours([
          root(["t"]),
          table([
            { key: "name", label: "Name" },
            { key: "name", label: "Again" },
          ]),
        ]),
      ),
    );
    expect(bad.rule).toBe("schema");
    expect(bad.reason).toMatch(/column "name" twice/);
  });

  it("a Table's rows reach the converter as lists of cells in the order of its columns: a column named path or call is a cell", () => {
    const p = prepareSurface(
      ours([
        root(["t"]),
        {
          id: "t",
          component: "Table",
          columns: [
            { key: "path", label: "Path" },
            { key: "call", label: "Call" },
          ],
          rows: [{ path: "src/a.ts", call: "Alice" }, { path: "/" }],
        },
      ]),
    );
    expect(p.kind, p.kind === "refused" ? p.reason : "").toBe("surface");
    const spec = JSON.stringify(p.kind === "surface" ? p.spec : {});
    expect(spec).toContain('["src/a.ts","Alice"]');
    expect(spec).toContain('["/",null]');
  });

  it("a Chart's series each have a value for each label (rule schema)", () => {
    const bad = refused(
      prepareSurface(ours([root(["c"]), chart({ series: [{ name: "Runs", values: [3] }] })])),
    );
    expect(bad.rule).toBe("schema");
    expect(bad.reason).toMatch(/1 values for 2 labels/);
  });

  it("a Chart's value is 0 or at least 1e-6 in size, for each type: the smallest number is refused (rule schema)", () => {
    for (const type of ["bar", "line", "pie"]) {
      const bad = refused(
        prepareSurface(
          ours([root(["c"]), chart({ type, series: [{ name: "x", values: [5e-324, 1] }] })]),
        ),
      );
      expect(bad.rule, type).toBe("schema");
      expect(bad.reason, type).toMatch(/1e-6/);
    }
    expect(
      refused(
        prepareSurface(
          ours([root(["c"]), chart({ type: "pie", series: [{ name: "x", values: [5e-324, 0] }] })]),
        ),
      ).rule,
    ).toBe("schema");
    expect(
      prepareSurface(ours([root(["c"]), chart({ series: [{ name: "x", values: [0, 1e-6] }] })]))
        .kind,
    ).toBe("surface");
  });

  it("a pie has at most six slices, each with its own colour (rule schema)", () => {
    const pie = (n: number) => {
      const labels = Array.from({ length: n }, (_, i) => `s${i}`);
      const values = labels.map(() => 1);
      return prepareSurface(
        ours([root(["c"]), chart({ type: "pie", labels, series: [{ name: "x", values }] })]),
      );
    };
    expect(pie(6).kind).toBe("surface");
    const seven = refused(pie(7));
    expect(seven.rule).toBe("schema");
    expect(seven.reason).toMatch(/at most six/);
  });

  it("a pie is one series, none of whose values is negative, and one of which is above zero", () => {
    const pie = (series: Rec[], labels = ["a", "b"]) =>
      prepareSurface(ours([root(["c"]), chart({ type: "pie", labels, series })]));
    expect(pie([{ name: "Share", values: [1, 2] }]).kind).toBe("surface");
    expect(
      refused(
        pie([
          { name: "x", values: [1, 1] },
          { name: "y", values: [1, 1] },
        ]),
      ).reason,
    ).toMatch(/pie of 2 series/);
    expect(refused(pie([{ name: "x", values: [-1, 2] }])).reason).toMatch(/negative/);
    expect(refused(pie([{ name: "x", values: [0, 0] }])).reason).toMatch(/above zero/);
  });

  it("in the basic catalog none of them is drawn: they are not in its vocabulary", () => {
    const p = refused(
      prepareSurface(surface([root(["t"]), table()], undefined, "v0.9.1", BASIC_CATALOG)),
    );
    expect(p.rule).toBe("catalog");
  });
});
