import { describe, expect, it } from "vitest";
import { applyA2uiOperations } from "./reducer";

type Rec = Record<string, unknown>;

const OWN_CATALOG = "https://agents.vymalo.com/a2ui/catalogs/chat";

/** The four operations, as an agent writes them (A2UI 1.0, docs/api/ui-catalog-v2.md). */
const create = (surfaceId: string, rest: Rec = {}, version = "1.0"): Rec => ({
  version,
  createSurface: { surfaceId, catalogId: OWN_CATALOG, ...rest },
});
const update = (surfaceId: string, components: unknown, version = "1.0"): Rec => ({
  version,
  updateComponents: { surfaceId, components },
});
const data = (surfaceId: string, rest: Rec, version = "1.0"): Rec => ({
  version,
  updateDataModel: { surfaceId, ...rest },
});
const drop = (surfaceId: string, version = "1.0"): Rec => ({
  version,
  deleteSurface: { surfaceId },
});

const root = (children: string[]) => ({ id: "root", component: "Column", children });
const text = (id: string, value: string) => ({ id, component: "Text", text: value });

/** The components of a surface, as a plain object, for a readable expectation. */
const componentsOf = (result: ReturnType<typeof applyA2uiOperations>, surfaceId: string) =>
  Object.fromEntries(result.state.get(surfaceId)?.components ?? []);

describe("the four operations", () => {
  it("createSurface, updateComponents and updateDataModel build a surface; deleteSurface ends it", () => {
    const built = applyA2uiOperations(new Map(), [
      create("s1"),
      update("s1", [root(["t"]), text("t", "hello")]),
      data("s1", { path: "/name", value: "one" }),
    ]);
    expect(built.warnings).toEqual([]);
    const surface = built.state.get("s1");
    expect(surface?.catalogId).toBe(OWN_CATALOG);
    expect(componentsOf(built, "s1")).toMatchObject({
      root: { component: "Column", children: ["t"] },
      t: { component: "Text", text: "hello" },
    });
    expect(surface?.dataModel).toEqual({ name: "one" });

    const ended = applyA2uiOperations(built.state, [drop("s1")]);
    expect(ended.warnings).toEqual([]);
    expect(ended.state.size).toBe(0);
  });

  it("an inline createSurface is read whole: its components and its data model at once", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      create("s1", {
        components: [root(["t"]), { id: "t", component: "Text", text: { path: "/x" } }],
        dataModel: { x: "hello" },
      }),
    ]);
    expect(warnings).toEqual([]);
    expect(componentsOf({ state, warnings }, "s1").t).toMatchObject({ text: { path: "/x" } });
    expect(state.get("s1")?.dataModel).toEqual({ x: "hello" });
  });

  it("updateComponents upserts by id: a later component replaces the one before, the others stay", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      create("s1"),
      update("s1", [root(["a", "b"]), text("a", "first"), text("b", "kept")]),
      update("s1", [text("a", "second")]),
    ]);
    expect(warnings).toEqual([]);
    expect(componentsOf({ state, warnings }, "s1")).toMatchObject({
      root: { children: ["a", "b"] },
      a: { text: "second" },
      b: { text: "kept" },
    });
    // the root's own children list is what it was: an upsert never edits it
    expect(componentsOf({ state, warnings }, "s1").root).toMatchObject({ children: ["a", "b"] });
  });

  it("updateDataModel writes the whole model with no path, a member with one, and null deletes", () => {
    const created = applyA2uiOperations(new Map(), [
      create("s1"),
      data("s1", { value: { name: "one", agree: false } }),
    ]);
    expect(created.state.get("s1")?.dataModel).toEqual({ name: "one", agree: false });

    const member = applyA2uiOperations(created.state, [
      data("s1", { path: "/name", value: "two" }),
    ]);
    expect(member.warnings).toEqual([]);
    expect(member.state.get("s1")?.dataModel).toEqual({ name: "two", agree: false });

    const deleted = applyA2uiOperations(member.state, [
      data("s1", { path: "/agree", value: null }),
    ]);
    expect(deleted.warnings).toEqual([]);
    expect(deleted.state.get("s1")?.dataModel).toEqual({ name: "two" });
  });

  it("a path writes into a list and appends to it", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      create("s1"),
      data("s1", { value: { items: ["a"] } }),
      data("s1", { path: "/items/1", value: "b" }),
      data("s1", { path: "/items/-", value: "c" }),
    ]);
    expect(warnings).toEqual([]);
    expect(state.get("s1")?.dataModel).toEqual({ items: ["a", "b", "c"] });
  });
});

describe("the surface map", () => {
  it("two surfaces stand side by side, and an update to one leaves the other alone", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      create("s1"),
      create("s2"),
      update("s1", [root([])]),
    ]);
    expect(warnings).toEqual([]);
    expect([...state.keys()]).toEqual(["s1", "s2"]);
    expect(state.get("s1")?.components.size).toBe(1);
    expect(state.get("s2")?.components.size).toBe(0);
  });

  it("never changes the state it was given: an update builds a new surface", () => {
    const before = applyA2uiOperations(new Map(), [create("s1"), update("s1", [text("t", "one")])]);
    const after = applyA2uiOperations(before.state, [update("s1", [text("t", "two")])]);
    expect(before.state.get("s1")?.components.get("t")).toMatchObject({ text: "one" });
    expect(after.state.get("s1")?.components.get("t")).toMatchObject({ text: "two" });
    expect(after.state).not.toBe(before.state);
  });
});

describe("refusals: nothing is guessed at, and a bad operation changes nothing", () => {
  const built = () => applyA2uiOperations(new Map(), [create("s1"), update("s1", [root([])])]);

  it("an operation that is not one of the four is refused, and the surface is kept", () => {
    const start = built();
    const { state, warnings } = applyA2uiOperations(start.state, [
      { version: "1.0", callRendererFunction: { surfaceId: "s1", name: "x" } },
    ]);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain("callRendererFunction");
    expect(state.get("s1")).toBe(start.state.get("s1"));
  });

  it("an operation with no surfaceId, or more than one operation, is refused", () => {
    const noId = applyA2uiOperations(new Map(), [{ version: "1.0", createSurface: {} }]);
    expect(noId.warnings[0]).toContain("surfaceId");
    const two = applyA2uiOperations(new Map(), [
      { version: "1.0", createSurface: { surfaceId: "s1" }, deleteSurface: { surfaceId: "s1" } },
    ]);
    expect(two.warnings[0]).toContain("exactly one operation");
    expect(two.state.size).toBe(0);
  });

  it("a second createSurface for a live surface is refused, and the first one is kept", () => {
    const start = built();
    const { state, warnings } = applyA2uiOperations(start.state, [
      create("s1", { components: [text("t", "again")] }),
    ]);
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toContain('"s1"');
    expect(componentsOf({ state, warnings }, "s1")).not.toHaveProperty("t");
  });

  it("updateComponents before any createSurface is refused", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [update("s1", [root([])])]);
    expect(warnings[0]).toContain("missing surface");
    expect(state.size).toBe(0);
  });

  it("updateDataModel without value is refused: contents and data are not a value", () => {
    const start = built();
    for (const rest of [{ contents: "x" }, { data: "x" }, { path: "/x" }]) {
      const { warnings } = applyA2uiOperations(start.state, [data("s1", rest)]);
      expect(warnings[0], JSON.stringify(rest)).toContain("no data model value");
    }
  });

  it("a createSurface that names no catalog is refused", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      { version: "1.0", createSurface: { surfaceId: "s1" } },
    ]);
    expect(warnings[0]).toContain("catalogId");
    expect(state.size).toBe(0);
  });

  it("an operation of an earlier version is refused, and the reason names it", () => {
    for (const version of ["v0.9", "v0.9.1", "v1.0"]) {
      const { state, warnings } = applyA2uiOperations(new Map(), [create("s1", {}, version)]);
      expect(state.size, version).toBe(0);
      expect(warnings[0], version).toContain(`"${version}"`);
      expect(warnings[0], version).toContain('"1.0"');
    }
    const unknown = applyA2uiOperations(new Map(), [create("s1", {}, "2.0")]);
    expect(unknown.warnings[0]).toContain('"2.0"');
  });

  it("a path that is not a JSON Pointer is refused, and the model is kept", () => {
    const start = applyA2uiOperations(new Map(), [create("s1"), data("s1", { value: { a: 1 } })]);
    const { state, warnings } = applyA2uiOperations(start.state, [
      data("s1", { path: "a", value: 2 }),
    ]);
    expect(warnings[0]).toContain("JSON Pointer");
    expect(state.get("s1")?.dataModel).toEqual({ a: 1 });
  });

  it("operations that are not a list, or an entry that is not an object, are refused", () => {
    expect(applyA2uiOperations(new Map(), "createSurface").warnings[0]).toContain(
      "must be an array",
    );
    expect(applyA2uiOperations(new Map(), ["createSurface"]).warnings[0]).toContain("malformed");
  });

  it("a component that is not {id, component} is skipped, and the rest of the surface stands", () => {
    const { state, warnings } = applyA2uiOperations(new Map(), [
      create("s1"),
      update("s1", [root([]), { id: "x" }, { component: "Text" }, text("t", "ok")]),
    ]);
    expect(warnings).toHaveLength(2);
    expect(componentsOf({ state, warnings }, "s1")).toMatchObject({
      root: { component: "Column" },
      t: { text: "ok" },
    });
  });
});
