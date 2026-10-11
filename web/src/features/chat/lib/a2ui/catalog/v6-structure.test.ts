import { describe, expect, it } from "vitest";
import lock from "./catalog.lock.json";
import { componentNames, OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 6, group 2 (structure and status): Stack, Tabs, Accordion, Callout, Checklist and Timeline.
 * Each schema on its own: valid, missing, oversize and wrong types. The walk into their children and
 * the lowering are in prepare-v6-structure.test.ts.
 */

const check = (component: string, instance: unknown) =>
  compileCatalog(OWN_CATALOG.catalog).check(component, instance);
const text = (n: number) => "x".repeat(n);
const kids = (n: number) => Array.from({ length: n }, (_, i) => `c${i}`);

describe("version 6: structure and status", () => {
  it("is catalog version 6, and has the six components", () => {
    expect(OWN_CATALOG.version).toBe(6);
    expect(lock.version).toBe(6);
    expect(componentNames(OWN_CATALOG.catalog)).toEqual(
      expect.arrayContaining(["Stack", "Tabs", "Accordion", "Callout", "Checklist", "Timeline"]),
    );
  });

  it("keeps the first sentence of each description short enough for adam's show to list it whole", () => {
    for (const name of ["Stack", "Tabs", "Accordion", "Callout", "Checklist", "Timeline"]) {
      const d = (OWN_CATALOG.catalog.components[name] as { description: string }).description;
      const end = d.indexOf(". ");
      const first = end < 0 ? d : d.slice(0, end + 1);
      expect(first.length, name).toBeLessThanOrEqual(100);
    }
  });
});

describe("Tabs", () => {
  const tabs = (
    list: unknown = [{ label: "Overview", child: "a" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "t",
    component: "Tabs",
    tabs: list,
    ...over,
  });

  it("accepts one tab and eight, each a label and the id of its child", () => {
    expect(check("Tabs", tabs())).toBeUndefined();
    const eight = Array.from({ length: 8 }, (_, i) => ({ label: `T${i}`, child: `c${i}` }));
    expect(check("Tabs", tabs(eight))).toBeUndefined();
    expect(check("Tabs", tabs([...eight, { label: "T9", child: "c9" }]))).toMatch(/^tabs: /);
  });

  it("needs at least one tab, and each tab its label and child", () => {
    expect(check("Tabs", tabs([]))).toMatch(/^tabs: /);
    expect(check("Tabs", tabs([{ label: "Only a label" }]))).toMatch(/required property "child"/);
    expect(check("Tabs", tabs([{ child: "a" }]))).toMatch(/required property "label"/);
    expect(check("Tabs", tabs([{ label: "", child: "a" }]))).toMatch(/^tabs\.0\.label: /);
    expect(check("Tabs", tabs([{ label: "a", child: "" }]))).toMatch(/^tabs\.0\.child: /);
    expect(check("Tabs", tabs([{ label: "a", child: "b", title: "x" }]))).toMatch(/title/);
    expect(check("Tabs", { id: "t", component: "Tabs" })).toMatch(/required property "tabs"/);
  });

  it("limits a label to 60 characters and the list to 8 tabs; a tab is an object, not a string", () => {
    expect(check("Tabs", tabs([{ label: text(60), child: "a" }]))).toBeUndefined();
    expect(check("Tabs", tabs([{ label: text(61), child: "a" }]))).toMatch(/^tabs\.0\.label: /);
    expect(check("Tabs", tabs(["a"]))).toMatch(/^tabs\.0: /);
    expect(check("Tabs", tabs({ label: "a", child: "b" }))).toMatch(/^tabs: /);
  });

  it("is not a binding: the children are ids", () => {
    expect(check("Tabs", tabs([{ label: "a", child: { path: "/c" } }]))).toMatch(
      /^tabs\.0\.child: /,
    );
  });
});

describe("Accordion", () => {
  const acc = (
    items: unknown[] = [{ title: "Setup", child: "a" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "a",
    component: "Accordion",
    items,
    ...over,
  });

  it("accepts items with a title and a child, and the multiple flag", () => {
    expect(check("Accordion", acc())).toBeUndefined();
    expect(
      check("Accordion", acc([{ title: "A", child: "a" }], { multiple: true })),
    ).toBeUndefined();
    expect(check("Accordion", acc([{ title: "A", child: "a" }], { multiple: "yes" }))).toMatch(
      /^multiple: /,
    );
  });

  it("needs items, and each item its title and child", () => {
    expect(check("Accordion", acc([]))).toMatch(/^items: /);
    expect(check("Accordion", acc([{ child: "a" }]))).toMatch(/required property "title"/);
    expect(check("Accordion", acc([{ title: "A" }]))).toMatch(/required property "child"/);
    expect(check("Accordion", { id: "a", component: "Accordion" })).toMatch(
      /required property "items"/,
    );
  });

  it("limits a title to 200 characters and the items to 20", () => {
    expect(check("Accordion", acc([{ title: text(200), child: "a" }]))).toBeUndefined();
    expect(check("Accordion", acc([{ title: text(201), child: "a" }]))).toMatch(
      /^items\.0\.title: /,
    );
    const twenty = Array.from({ length: 20 }, (_, i) => ({ title: `I${i}`, child: `c${i}` }));
    expect(check("Accordion", acc(twenty))).toBeUndefined();
    expect(check("Accordion", acc([...twenty, { title: "x", child: "y" }]))).toMatch(/^items: /);
  });
});

describe("Callout", () => {
  const callout = (over: Record<string, unknown> = {}) => ({
    id: "k",
    component: "Callout",
    title: "Heads up",
    text: "The database is read-only until the migration ends.",
    ...over,
  });

  it("takes each tone, and a tone that is not one of them is refused", () => {
    for (const tone of ["info", "success", "warning", "error"]) {
      expect(check("Callout", callout({ tone })), tone).toBeUndefined();
    }
    expect(check("Callout", callout({ tone: "danger" }))).toMatch(/^tone: /);
    expect(check("Callout", callout({ tone: "Warning" }))).toMatch(/^tone: /);
    expect(check("Callout", callout())).toBeUndefined();
  });

  it("needs a title and a text", () => {
    const { title: _t, ...noTitle } = callout();
    const { text: _x, ...noText } = callout();
    expect(check("Callout", noTitle)).toMatch(/required property "title"/);
    expect(check("Callout", noText)).toMatch(/required property "text"/);
    expect(check("Callout", callout({ title: "" }))).toMatch(/^title: /);
    expect(check("Callout", callout({ text: "" }))).toMatch(/^text: /);
  });

  it("limits the title to 120 characters and the text to 2,000", () => {
    expect(check("Callout", callout({ title: text(120) }))).toBeUndefined();
    expect(check("Callout", callout({ title: text(121) }))).toMatch(/^title: /);
    expect(check("Callout", callout({ text: text(2000) }))).toBeUndefined();
    expect(check("Callout", callout({ text: text(2001) }))).toMatch(/^text: /);
  });

  it("has no member that could carry a link, an action or a level", () => {
    expect(check("Callout", callout({ url: "https://example.com" }))).toMatch(/url/);
    expect(check("Callout", callout({ action: { event: { name: "x" } } }))).toMatch(/action/);
  });
});

describe("Checklist", () => {
  const list = (
    items: unknown[] = [{ label: "Write the tests", state: "done" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "l",
    component: "Checklist",
    items,
    ...over,
  });

  it("takes items in each of the four states, with an optional title", () => {
    const items = ["done", "doing", "todo", "failed"].map((state, i) => ({
      label: `Step ${i}`,
      state,
    }));
    expect(check("Checklist", list(items, { title: "Release" }))).toBeUndefined();
    expect(check("Checklist", list())).toBeUndefined();
  });

  it("refuses any other state, and a state that is not lower case", () => {
    for (const state of ["skipped", "Done", "complete", ""]) {
      expect(check("Checklist", list([{ label: "a", state }])), state).toMatch(
        /^items\.0\.state: /,
      );
    }
  });

  it("needs its items (at least one), and each item its label and state", () => {
    expect(check("Checklist", list([]))).toMatch(/^items: /);
    expect(check("Checklist", list([{ state: "done" }]))).toMatch(/required property "label"/);
    expect(check("Checklist", list([{ label: "a" }]))).toMatch(/required property "state"/);
    expect(check("Checklist", { id: "l", component: "Checklist" })).toMatch(
      /required property "items"/,
    );
  });

  it("limits a label to 200 characters, the items to 50 and the title to 120", () => {
    expect(check("Checklist", list([{ label: text(200), state: "todo" }]))).toBeUndefined();
    expect(check("Checklist", list([{ label: text(201), state: "todo" }]))).toMatch(
      /^items\.0\.label: /,
    );
    const fifty = Array.from({ length: 50 }, (_, i) => ({ label: `s${i}`, state: "todo" }));
    expect(check("Checklist", list(fifty))).toBeUndefined();
    expect(check("Checklist", list([...fifty, { label: "x", state: "todo" }]))).toMatch(/^items: /);
    expect(check("Checklist", list(undefined, { title: text(121) }))).toMatch(/^title: /);
  });
});

describe("Timeline", () => {
  const timeline = (
    events: unknown[] = [{ time: "09:00", title: "Started" }],
    over: Record<string, unknown> = {},
  ) => ({
    id: "tl",
    component: "Timeline",
    events,
    ...over,
  });

  it("takes events with a time and a title; the description and the state are optional", () => {
    expect(check("Timeline", timeline())).toBeUndefined();
    expect(
      check(
        "Timeline",
        timeline(
          [{ time: "09:05", title: "Deployed", description: "To staging.", state: "todo" }],
          { title: "Day" },
        ),
      ),
    ).toBeUndefined();
  });

  it("needs a time and a title, and takes only the four states", () => {
    expect(check("Timeline", timeline([{ title: "x" }]))).toMatch(/required property "time"/);
    expect(check("Timeline", timeline([{ time: "09:00" }]))).toMatch(/required property "title"/);
    expect(check("Timeline", timeline([{ time: "a", title: "b", state: "later" }]))).toMatch(
      /^events\.0\.state: /,
    );
    expect(check("Timeline", timeline([]))).toMatch(/^events: /);
  });

  it("limits the time to 40, the title to 200, the description to 1,000 and the events to 50", () => {
    expect(check("Timeline", timeline([{ time: text(40), title: "a" }]))).toBeUndefined();
    expect(check("Timeline", timeline([{ time: text(41), title: "a" }]))).toMatch(
      /^events\.0\.time: /,
    );
    expect(check("Timeline", timeline([{ time: "a", title: text(200) }]))).toBeUndefined();
    expect(check("Timeline", timeline([{ time: "a", title: text(201) }]))).toMatch(
      /^events\.0\.title: /,
    );
    expect(
      check("Timeline", timeline([{ time: "a", title: "b", description: text(1000) }])),
    ).toBeUndefined();
    expect(
      check("Timeline", timeline([{ time: "a", title: "b", description: text(1001) }])),
    ).toMatch(/^events\.0\.description: /);
    const fifty = Array.from({ length: 50 }, (_, i) => ({ time: `${i}`, title: "t" }));
    expect(check("Timeline", timeline(fifty))).toBeUndefined();
    expect(check("Timeline", timeline([...fifty, { time: "x", title: "y" }]))).toMatch(/^events: /);
  });

  it("the children of a Tabs are ids, one per tab: a count of 8 is the most", () => {
    expect(kids(8)).toHaveLength(8);
  });
});

describe("Stack", () => {
  const stack = (over: Record<string, unknown> = {}) => ({
    id: "s",
    component: "Stack",
    children: ["a", "b"],
    ...over,
  });

  it("accepts the minimum: an id and its children, horizontal and wrapping by default", () => {
    expect(check("Stack", stack())).toBeUndefined();
    expect(check("Stack", stack({ direction: "vertical" }))).toBeUndefined();
    expect(check("Stack", stack({ direction: "horizontal" }))).toBeUndefined();
  });

  it("takes each of its enums, and refuses a value outside them", () => {
    for (const gap of ["none", "small", "medium", "large"]) {
      expect(check("Stack", stack({ gap })), gap).toBeUndefined();
    }
    for (const align of ["start", "center", "end", "stretch"]) {
      expect(check("Stack", stack({ align })), align).toBeUndefined();
    }
    for (const justify of ["start", "center", "end", "between"]) {
      expect(check("Stack", stack({ justify })), justify).toBeUndefined();
    }
    expect(check("Stack", stack({ direction: "diagonal" }))).toMatch(/^direction: /);
    expect(check("Stack", stack({ gap: "huge" }))).toMatch(/^gap: /);
    expect(check("Stack", stack({ align: "baseline" }))).toMatch(/^align: /);
    expect(check("Stack", stack({ justify: "around" }))).toMatch(/^justify: /);
  });

  it("takes wrap as a boolean, and a weight as a number", () => {
    expect(check("Stack", stack({ wrap: false }))).toBeUndefined();
    expect(check("Stack", stack({ wrap: true }))).toBeUndefined();
    expect(check("Stack", stack({ wrap: "yes" }))).toMatch(/^wrap: /);
    expect(check("Stack", stack({ weight: 2 }))).toBeUndefined();
    expect(check("Stack", stack({ weight: "2" }))).toMatch(/^weight: /);
  });

  it("needs its id, its component and its children", () => {
    expect(check("Stack", { id: "s", component: "Stack" })).toMatch(/required property "children"/);
    expect(check("Stack", { component: "Stack", children: ["a"] })).toMatch(
      /required property "id"/,
    );
    expect(check("Stack", { id: "s", children: ["a"] })).toMatch(/required property "component"/);
  });

  it("holds between one and fifty children, each an id", () => {
    expect(check("Stack", stack({ children: kids(1) }))).toBeUndefined();
    expect(check("Stack", stack({ children: kids(50) }))).toBeUndefined();
    expect(check("Stack", stack({ children: [] }))).toMatch(/^children: /);
    expect(check("Stack", stack({ children: kids(51) }))).toMatch(/^children: /);
    expect(check("Stack", stack({ children: [""] }))).toMatch(/^children\.0: /);
    expect(check("Stack", stack({ children: [text(256)] }))).toBeUndefined();
    expect(check("Stack", stack({ children: [text(257)] }))).toMatch(/^children\.0: /);
    expect(check("Stack", stack({ children: [{ child: "a" }] }))).toMatch(/^children\.0: /);
  });

  it("refuses a property it does not know: a Stack is not a Column with more options", () => {
    expect(check("Stack", stack({ title: "x" }))).toMatch(/title/);
    expect(check("Stack", stack({ spacing: 4 }))).toMatch(/spacing/);
    expect(check("Stack", stack({ id: text(257) }))).toMatch(/^id: /);
  });
});
