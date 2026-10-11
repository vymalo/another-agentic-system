import { describe, expect, it } from "vitest";
import { type Prepared, prepareSurface } from "./prepare";
import { BASIC_CATALOG, OWN_CATALOG_ID, surface } from "./testing";

/**
 * Catalog version 6, group 2 (structure and status), through the validator: a Tabs and an Accordion hold
 * components of the surface (their children are walked like any reference), a child not sent yet is
 * pending, and a child that holds its parent is a cycle.
 */

type Rec = Record<string, unknown>;
const refused = (p: Prepared) => {
  if (p.kind !== "refused") throw new Error(`expected a refusal, got ${p.kind}`);
  return p;
};
const ours = (components: Rec[]) => surface(components, undefined, "v0.9.1", OWN_CATALOG_ID);
const root = (children: string[]): Rec => ({ id: "root", component: "Column", children });

describe("Tabs and Accordion, in a surface of our catalog", () => {
  it("are drawn with their children, lowered so that the converter draws each of them", () => {
    const p = prepareSurface(
      ours([
        root(["tabs", "acc", "k", "list", "line"]),
        {
          id: "tabs",
          component: "Tabs",
          tabs: [
            { label: "First", child: "a" },
            { label: "Second", child: "b" },
          ],
        },
        { id: "a", component: "Text", text: "First body" },
        { id: "b", component: "Text", text: "Second body" },
        { id: "acc", component: "Accordion", items: [{ title: "Setup", child: "s" }] },
        { id: "s", component: "Text", text: "Install the packages" },
        {
          id: "k",
          component: "Callout",
          tone: "warning",
          title: "Careful",
          text: "It is not reversible.",
        },
        {
          id: "list",
          component: "Checklist",
          items: [{ label: "Write the tests", state: "done" }],
        },
        {
          id: "line",
          component: "Timeline",
          events: [{ time: "09:00", title: "Started" }],
        },
      ]),
    );
    expect(p.kind).toBe("surface");
    const spec = JSON.stringify(p.kind === "surface" ? p.spec : {});
    for (const name of ["Tabs", "Accordion", "Callout", "Checklist", "Timeline"]) {
      expect(spec, name).toContain(`"vymalo.${name}"`);
    }
    for (const words of ["First body", "Second body", "Install the packages"]) {
      expect(spec, words).toContain(words);
    }
  });

  it("a child that is not in the surface yet makes it pending, as any reference does", () => {
    const p = prepareSurface(
      ours([root(["t"]), { id: "t", component: "Tabs", tabs: [{ label: "A", child: "a" }] }]),
    );
    expect(p.kind).toBe("pending");
  });

  it("a tab whose child holds the tabs is a cycle, refused", () => {
    const bad = refused(
      prepareSurface(
        ours([root(["t"]), { id: "t", component: "Tabs", tabs: [{ label: "A", child: "t" }] }]),
      ),
    );
    expect(bad.rule).toBe("cycle");
  });

  it("an accordion item whose child holds the accordion is a cycle, refused", () => {
    const bad = refused(
      prepareSurface(
        ours([
          root(["acc"]),
          { id: "acc", component: "Accordion", items: [{ title: "x", child: "c" }] },
          { id: "c", component: "Column", children: ["acc"] },
        ]),
      ),
    );
    expect(bad.rule).toBe("cycle");
  });

  it("a tab names a child of the wrong shape: refused by the schema, not drawn", () => {
    expect(
      refused(
        prepareSurface(
          ours([root(["t"]), { id: "t", component: "Tabs", tabs: [{ label: "A", child: 4 }] }]),
        ),
      ).rule,
    ).toBe("schema");
  });

  it("in the basic catalog they are not drawn: they are not in its vocabulary", () => {
    const p = refused(
      prepareSurface(
        surface(
          [root(["t"]), { id: "t", component: "Tabs", tabs: [{ label: "A", child: "a" }] }],
          undefined,
          "v0.9.1",
          BASIC_CATALOG,
        ),
      ),
    );
    expect(p.rule).toBe("catalog");
  });
});
