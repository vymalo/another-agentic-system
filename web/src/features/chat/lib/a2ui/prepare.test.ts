import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { OWN_CATALOG, type OwnCatalog } from "./catalog";
import {
  BASIC_CATALOG_IDS,
  MAX_BYTES,
  MAX_COMPONENTS,
  MAX_DEPTH,
  MAX_NODES,
  MAX_TEMPLATE_ITEMS,
  OPEN_URL,
  UNSUPPORTED,
  USER_MESSAGE,
  VOCABULARY,
} from "./limits";
import { type Prepared, prepareSurface } from "./prepare";
import {
  BAD_URLS,
  BASIC_CATALOG,
  button,
  column,
  event,
  GOOD_URLS,
  labelled,
  NOT_TEXT,
  OWN_CATALOG_ID,
  openUrl,
  SURFACE,
  sizeOf,
  surface,
  text,
  withRoot,
} from "./testing";

type Rec = Record<string, unknown>;
const refused = (p: Prepared) => {
  expect(p.kind).toBe("refused");
  return p as Extract<Prepared, { kind: "refused" }>;
};
const drawn = (p: Prepared) => {
  expect(p, JSON.stringify(p)).toMatchObject({ kind: "surface" });
  return p as Extract<Prepared, { kind: "surface" }>;
};
/** The first node of the converted spec that has this `$type`. */
const find = (node: unknown, type: string): Rec | undefined => {
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = find(n, type);
      if (hit) return hit;
    }
    return undefined;
  }
  if (typeof node !== "object" || node === null) return undefined;
  const rec = node as Rec;
  if (rec.$type === type) return rec;
  return find(rec.children, type);
};

/** The A2UI story of docs/api/examples/agui/a2ui.agui.json: the last snapshot of the surface. */
function goldenOperations(): unknown[] {
  const file = path.resolve(
    import.meta.dirname,
    "../../../../../../docs/api/examples/agui/a2ui.agui.json",
  );
  const frames = JSON.parse(readFileSync(file, "utf8")) as { event: Rec }[];
  const snapshots = frames.filter((f) => f.event.activityType === "a2ui-surface");
  const last = snapshots[snapshots.length - 1] as {
    event: { content: { a2ui_operations: unknown[] } };
  };
  return last.event.content.a2ui_operations;
}

describe("a valid surface", () => {
  it("the golden, whose operations say v0.9.1 (the pinned runtime would drop them), is drawn", () => {
    const p = drawn(prepareSurface(goldenOperations()));
    expect(p.surfaceId).toBe("s1");
    expect(p.eventActions).toBe(1);
    expect(p.spec).toMatchObject({
      $type: "Col",
      children: [
        { $type: "Markdown", value: "Pick one" },
        {
          $type: "Button",
          label: "Go",
          buttonStyle: "primary",
          $action: {
            type: "a2ui:action",
            name: "go",
            surfaceId: "s1",
            sourceComponentId: "go",
            context: { choice: "a" },
          },
        },
      ],
    });
  });

  it("v0.9 and v1.0 (with inline components and data model) read the same", () => {
    const v09 = drawn(prepareSurface(surface([column("root", ["t"]), text("t", "hi")])));
    const v10 = drawn(
      prepareSurface([
        {
          version: "v1.0",
          createSurface: {
            surfaceId: SURFACE,
            components: [
              column("root", ["t"]),
              { id: "t", component: "Text", text: { path: "/x" } },
            ],
            dataModel: { x: "hello" },
          },
        },
      ]),
    );
    expect(find(v09.spec, "Markdown")).toMatchObject({ value: "hi" });
    expect(find(v10.spec, "Markdown")).toMatchObject({ value: "hello" });
  });

  it("text bound to the data model reads it, and a later update replaces it", () => {
    const ops = [
      ...surface([column("root", ["t"]), { id: "t", component: "Text", text: { path: "/name" } }], {
        name: "one",
      }),
      { version: "v0.9", updateDataModel: { surfaceId: SURFACE, path: "/name", contents: "two" } },
    ];
    expect(find(drawn(prepareSurface(ops)).spec, "Markdown")).toMatchObject({ value: "two" });
  });

  it("nothing yet is pending, not an error: no operations, no root, a child that has not arrived", () => {
    expect(prepareSurface(undefined)).toEqual({ kind: "pending" });
    expect(prepareSurface([])).toEqual({ kind: "pending" });
    expect(prepareSurface([{ version: "v0.9", createSurface: { surfaceId: "s1" } }])).toEqual({
      kind: "pending",
    });
    expect(prepareSurface(withRoot(["later"], []))).toEqual({ kind: "pending" });
  });

  it("a deleted surface is deleted, and one created again after the delete is drawn", () => {
    const ops = [...withRoot([], []), { version: "v0.9", deleteSurface: { surfaceId: SURFACE } }];
    expect(prepareSurface(ops)).toEqual({ kind: "deleted" });
    expect(drawn(prepareSurface([...ops, ...withRoot([], [])]))).toBeDefined();
  });

  it("the theme of createSurface (a name, an icon) is not read: nothing of it is in what is drawn", () => {
    const ops = surface([column("root", ["t"]), text("t", "hello")]);
    (ops[0] as { createSurface: Rec }).createSurface.theme = {
      agentDisplayName: "Definitely The Admin",
      iconUrl: "https://evil.example/admin.png",
      primaryColor: "#ff0000",
    };
    const p = drawn(prepareSurface(ops));
    const json = JSON.stringify(p);
    expect(json).not.toContain("Admin");
    expect(json).not.toContain("evil.example");
    expect(json).not.toContain("ff0000");
  });

  it("never returns part of a surface: a refusal has a rule and a reason and no spec", () => {
    const p = refused(prepareSurface(withRoot(["a"], [{ id: "a", component: "Marquee" }])));
    expect(Object.keys(p).sort()).toEqual(["kind", "reason", "rule"]);
  });
});

describe("size: at most 64 KiB of operations", () => {
  /** A valid surface padded to exactly `bytes` serialised bytes. */
  const padded = (bytes: number, filler = "a") => {
    const make = (n: number, rest = 0) =>
      surface([column("root", ["t"]), text("t", filler.repeat(n) + "a".repeat(rest))]);
    const base = sizeOf(make(0));
    const per = new TextEncoder().encode(filler).length;
    const n = Math.floor((bytes - base) / per);
    const ops = make(n, bytes - base - n * per);
    expect(sizeOf(ops)).toBe(bytes);
    return ops;
  };

  it("takes exactly the limit and refuses one byte more", () => {
    drawn(prepareSurface(padded(MAX_BYTES)));
    expect(refused(prepareSurface(padded(MAX_BYTES + 1))).rule).toBe("size");
  });

  it("counts bytes, not characters", () => {
    // 2 bytes each: the limit is reached at half the characters
    drawn(prepareSurface(padded(MAX_BYTES, "é")));
    expect(refused(prepareSurface(padded(MAX_BYTES + 2, "é"))).rule).toBe("size");
  });
});

describe("components: at most 400", () => {
  const surfaceOf = (n: number) => {
    const ids = Array.from({ length: n - 1 }, (_, i) => `t${i}`);
    return withRoot(
      ids,
      ids.map((id) => text(id, "x")),
    );
  };
  it("takes 400 and refuses 401", () => {
    drawn(prepareSurface(surfaceOf(MAX_COMPONENTS)));
    expect(refused(prepareSurface(surfaceOf(MAX_COMPONENTS + 1))).rule).toBe("components");
  });

  it("counts components nobody references too", () => {
    const ops = surface([
      column("root", []),
      ...Array.from({ length: MAX_COMPONENTS }, (_, i) => text(`orphan${i}`, "x")),
    ]);
    expect(refused(prepareSurface(ops)).rule).toBe("components");
  });
});

describe("expansion: at most 2000 nodes after references and templates are expanded", () => {
  /** root + M `mid`s of 10 leaves each + `extra` leaves: 1 + 11M + extra nodes, in 3 components. */
  const nodes = (extra: number) =>
    surface([
      column("root", [
        ...Array.from({ length: 181 }, () => "mid"),
        ...Array.from({ length: extra }, () => "leaf"),
      ]),
      column(
        "mid",
        Array.from({ length: 10 }, () => "leaf"),
      ),
      text("leaf", "x"),
    ]);

  it("takes exactly 2000 nodes and refuses 2001", () => {
    drawn(prepareSurface(nodes(8)));
    const p = refused(prepareSurface(nodes(9)));
    expect(p.rule).toBe("expansion");
    expect(p.reason).toContain(String(MAX_NODES));
  });

  it("a template counts its items (and the wrapper of each) as nodes", () => {
    // root 1 + per item: wrapper 1 + Text 1 = 1 + 2 * 100
    const items = Array.from({ length: 100 }, (_, i) => `item ${i}`);
    const ops = surface(
      [
        { id: "root", component: "List", children: { componentId: "row", path: "/items" } },
        { id: "row", component: "Text", text: { path: "/" } },
      ],
      { items },
    );
    const p = drawn(prepareSurface(ops));
    expect(p.spec).toMatchObject({ $type: "ListView" });
    expect((p.spec.children as unknown[]).length).toBe(100);
  });

  it("an expansion bomb (100 x 100 x ...) is refused, quickly, before anything converts it", () => {
    const inner = Array.from({ length: 100 }, (_, i) => i);
    const data = { items: Array.from({ length: 100 }, () => ({ items: inner })) };
    const ops = surface(
      [
        { id: "root", component: "Column", children: { componentId: "outer", path: "/items" } },
        { id: "outer", component: "Column", children: { componentId: "cell", path: "/items" } },
        { id: "cell", component: "Text", text: "x" },
      ],
      data,
    );
    const started = performance.now();
    const p = refused(prepareSurface(ops));
    expect(p.rule).toBe("expansion");
    expect(performance.now() - started).toBeLessThan(500);
  });

  it("a reference bomb (every level names the next twice, 2^23 nodes in 24 components) is refused, quickly", () => {
    const comps = Array.from({ length: 23 }, (_, i) =>
      column(i === 0 ? "root" : `c${i}`, [`c${i + 1}`, `c${i + 1}`]),
    );
    comps.push(text("c23", "x"));
    const started = performance.now();
    const p = refused(prepareSurface(surface(comps)));
    expect(p.rule).toBe("expansion");
    expect(performance.now() - started).toBeLessThan(500);
  });
});

describe("templates: at most 100 items", () => {
  const listOf = (n: number) =>
    surface(
      [
        { id: "root", component: "Column", children: { componentId: "row", path: "/items" } },
        { id: "row", component: "Text", text: { path: "/" } },
      ],
      { items: Array.from({ length: n }, (_, i) => `item ${i}`) },
    );
  it("takes 100 and refuses 101 (nothing is cut off silently)", () => {
    drawn(prepareSurface(listOf(MAX_TEMPLATE_ITEMS)));
    const p = refused(prepareSurface(listOf(MAX_TEMPLATE_ITEMS + 1)));
    expect(p.rule).toBe("template");
  });

  it("reads both spellings of a template, {componentId, path} and {template: {componentId, path}}", () => {
    const wrapped = surface(
      [
        {
          id: "root",
          component: "Column",
          children: { template: { componentId: "row", path: "/items" } },
        },
        { id: "row", component: "Text", text: { path: "/" } },
      ],
      { items: ["a", "b"] },
    );
    expect(drawn(prepareSurface(wrapped)).spec.children).toHaveLength(2);
    expect(drawn(prepareSurface(listOf(2))).spec.children).toHaveLength(2);
  });

  it("a template that does not read a list is refused", () => {
    const ops = surface(
      [
        { id: "root", component: "Column", children: { componentId: "row", path: "/missing" } },
        { id: "row", component: "Text", text: "x" },
      ],
      { items: [] },
    );
    expect(refused(prepareSurface(ops)).rule).toBe("template");
  });

  it("children that are neither a list nor a template are refused", () => {
    expect(refused(prepareSurface(surface([column("root", { nope: 1 })]))).rule).toBe("shape");
    expect(refused(prepareSurface(surface([column("root", [1 as never])]))).rule).toBe("shape");
  });
});

describe("depth: at most 24 levels", () => {
  /** A chain of `levels` components: `levels - 1` columns and a text. */
  const chain = (levels: number) =>
    surface(
      Array.from({ length: levels }, (_, i) => {
        const id = i === 0 ? "root" : `c${i}`;
        return i === levels - 1 ? text(id, "deep") : column(id, [`c${i + 1}`]);
      }),
    );
  it("takes 24 levels and refuses 25", () => {
    drawn(prepareSurface(chain(MAX_DEPTH)));
    const p = refused(prepareSurface(chain(MAX_DEPTH + 1)));
    expect(p.rule).toBe("depth");
    expect(p.reason).toContain(String(MAX_DEPTH));
  });

  it("a template item is one level below its container", () => {
    const at = (levels: number) =>
      surface(
        [
          ...Array.from({ length: levels - 1 }, (_, i) =>
            column(
              i === 0 ? "root" : `c${i}`,
              i === levels - 2 ? { componentId: "row", path: "/items" } : [`c${i + 1}`],
            ),
          ),
          text("row", "x"),
        ],
        { items: ["a"] },
      );
    drawn(prepareSurface(at(MAX_DEPTH)));
    expect(refused(prepareSurface(at(MAX_DEPTH + 1))).rule).toBe("depth");
  });

  it("a cycle is refused (a component that contains itself, directly or not)", () => {
    expect(refused(prepareSurface(surface([column("root", ["root"])]))).rule).toBe("cycle");
    expect(
      refused(
        prepareSurface(surface([column("root", ["a"]), column("a", ["b"]), column("b", ["a"])])),
      ).rule,
    ).toBe("cycle");
  });
});

describe("vocabulary: only the components the renderer draws", () => {
  it("the vocabulary is small", () => {
    expect([...VOCABULARY].sort()).toEqual(
      [
        "Button",
        "Card",
        "CheckBox",
        "Column",
        "Divider",
        "Image",
        "List",
        "Row",
        "Text",
        "TextField",
      ].sort(),
    );
  });

  it.each([
    "Icon",
    "Tabs",
    "Modal",
    "Slider",
    "DateTimeInput",
    "ChoicePicker",
    "AudioPlayer",
    "Video",
    "Script",
    "Iframe",
    "text",
    "TEXT",
    "Text ",
    "toString",
    "constructor",
    "__proto__",
    "hasOwnProperty",
    "vymalo.TextField",
    "vymalo.CheckBox",
    "",
  ])("refuses the whole surface for a %j component", (name) => {
    const p = refused(
      prepareSurface(withRoot(["ok", "x"], [text("ok", "fine"), { id: "x", component: name }])),
    );
    // "" fails as a malformed component in the reducer; the rest as vocabulary
    expect(["vocabulary", "shape"]).toContain(p.rule);
  });

  it("refuses a component nobody references: an unknown one is refused wherever it is", () => {
    expect(
      refused(prepareSurface(withRoot([], [{ id: "hidden", component: "Icon", name: "check" }])))
        .rule,
    ).toBe("vocabulary");
  });

  it("every component of the vocabulary is accepted", () => {
    const ops = surface(
      [
        column("root", ["card", "list", "row", "img", "hr", "field", "box", "go", "go_label"]),
        { id: "card", component: "Card", child: "t" },
        text("t", "in a card"),
        { id: "list", component: "List", children: ["t"] },
        { id: "row", component: "Row", children: ["t"] },
        { id: "img", component: "Image", url: "https://example.com/a.png" },
        { id: "hr", component: "Divider" },
        { id: "field", component: "TextField", label: "Name", value: { path: "/name" } },
        { id: "box", component: "CheckBox", label: "Agree", value: { path: "/agree" } },
        button("go", event("go")),
        text("go_label", "Go"),
      ],
      { name: "n", agree: false },
    );
    drawn(prepareSurface(ops));
  });

  it("an unknown operation, a version it does not read, or two operations in one message refuse the surface", () => {
    const good = surface([column("root", [])]);
    const cases: [string, unknown[]][] = [
      [
        "unknown operation",
        [...good, { version: "v0.9", callRendererFunction: { surfaceId: SURFACE } }],
      ],
      ["version", [{ version: "v2", createSurface: { surfaceId: SURFACE } }]],
      ["no version", [{ createSurface: { surfaceId: SURFACE } }]],
      [
        "two operations",
        [
          {
            version: "v0.9",
            createSurface: { surfaceId: SURFACE },
            deleteSurface: { surfaceId: SURFACE },
          },
        ],
      ],
      ["not an object", [...good, "createSurface"]],
      ["no surface id", [{ version: "v0.9", createSurface: {} }]],
      ["surface id not text", [{ version: "v0.9", createSurface: { surfaceId: 7 } }]],
      [
        "update of a surface that was never created",
        [{ version: "v0.9", updateComponents: { surfaceId: "x", components: [] } }],
      ],
      [
        "components not a list",
        [...good, { version: "v0.9", updateComponents: { surfaceId: SURFACE, components: {} } }],
      ],
      [
        "component without id",
        [
          ...good,
          {
            version: "v0.9",
            updateComponents: { surfaceId: SURFACE, components: [{ component: "Text" }] },
          },
        ],
      ],
      ["operations not a list", { version: "v0.9" } as unknown as unknown[]],
      ["a string", "createSurface" as unknown as unknown[]],
    ];
    for (const [label, ops] of cases) {
      expect(prepareSurface(ops).kind, label).toBe("refused");
    }
  });

  it("two surfaces in one activity are refused", () => {
    const ops = [
      ...surface([column("root", [])]),
      { version: "v0.9", createSurface: { surfaceId: "other" } },
    ];
    expect(refused(prepareSurface(ops)).rule).toBe("surfaces");
  });

  it("a function value (formatString, ...) is refused: the converter cannot run it", () => {
    const ops = surface([
      column("root", ["t"]),
      {
        id: "t",
        component: "Text",
        text: { call: "formatString", args: { value: "hi $" + "{/name}" } },
      },
    ]);
    expect(refused(prepareSurface(ops)).rule).toBe("function");
  });

  it("properties nested absurdly deep are refused, not walked", () => {
    let deep: unknown = "x";
    for (let i = 0; i < 5000; i++) deep = [deep];
    const p = prepareSurface(
      surface([column("root", ["t"]), { id: "t", component: "Text", text: "x", extra: deep }]),
    );
    expect(p.kind).toBe("refused");
  });
});

describe("links: only absolute http(s) URLs", () => {
  const withUrl = (url: unknown) =>
    surface([column("root", [...["go", "go_label"]]), ...labelled("go", "Open", openUrl(url))]);

  it.each(GOOD_URLS)("openUrl to %s becomes a link to %s", (url, href) => {
    const p = drawn(prepareSurface(withUrl(url)));
    expect(find(p.spec, "Button")).toMatchObject({
      $action: { name: OPEN_URL, context: { url: href } },
    });
    expect(p.eventActions).toBe(0);
  });

  it.each(BAD_URLS.map((u) => [JSON.stringify(u), u]))(
    "openUrl to %s refuses the surface",
    (_label, url) => {
      expect(refused(prepareSurface(withUrl(url))).rule).toBe("url");
    },
  );

  it.each(NOT_TEXT.map((v) => [JSON.stringify(v) ?? "undefined", v]))(
    "openUrl to a value that is not text (%s) refuses the surface",
    (_label, url) => {
      expect(refused(prepareSurface(withUrl(url))).rule).toBe("url");
    },
  );

  it("openUrl to a URL read from the data model is refused: only a literal URL is checked when it is sent", () => {
    const ops = surface(
      [column("root", ["go", "go_label"]), ...labelled("go", "Open", openUrl({ path: "/u" }))],
      { u: "https://example.com" },
    );
    expect(refused(prepareSurface(ops)).rule).toBe("url");
  });

  it.each(["url", "href", "src", "uri", "link", "iconUrl", "imageUrl"])(
    "a %s prop that is not http(s) refuses the surface, on any component",
    (key) => {
      for (const bad of [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "data:text/html,x",
        "//evil.example/x",
        " https://example.com",
      ]) {
        const ops = surface([
          column("root", ["img"]),
          { id: "img", component: "Image", [key]: bad },
        ]);
        expect(refused(prepareSurface(ops)).rule, `${key} ${bad}`).toBe("url");
      }
      const ok = surface([
        column("root", ["img"]),
        { id: "img", component: "Image", [key]: "https://example.com/a.png" },
      ]);
      drawn(prepareSurface(ok));
    },
  );

  it("a URL bound to the data model is checked where it is read, in every item of a template", () => {
    const at = (urls: string[]) =>
      surface(
        [
          { id: "root", component: "Column", children: { componentId: "img", path: "/items" } },
          { id: "img", component: "Image", url: { path: "/url" } },
        ],
        { items: urls.map((url) => ({ url })) },
      );
    drawn(prepareSurface(at(["https://a.example/1.png", "https://a.example/2.png"])));
    expect(
      refused(prepareSurface(at(["https://a.example/1.png", "javascript:alert(1)"]))).rule,
    ).toBe("url");
  });

  it("an openUrl URL is normalised once, in what is drawn", () => {
    const p = drawn(prepareSurface(withUrl("HTTPS://Example.COM/x")));
    expect(JSON.stringify(p.spec)).not.toContain("HTTPS://");
  });
});

describe("actions", () => {
  const withAction = (action: unknown, extra: Rec[] = []) =>
    surface([column("root", ["go", "go_label"]), ...labelled("go", "Go", action), ...extra]);
  const actionOf = (p: Prepared) => (find(drawn(p).spec, "Button")?.$action ?? {}) as Rec;

  it("an event becomes a2ui:action with the surface and the component, and counts as an event action", () => {
    const p = drawn(prepareSurface(withAction(event("pick", { choice: "a", n: 1 }))));
    expect(p.eventActions).toBe(1);
    expect(find(p.spec, "Button")?.$action).toEqual({
      type: "a2ui:action",
      name: "pick",
      surfaceId: SURFACE,
      sourceComponentId: "go",
      context: { choice: "a", n: 1 },
    });
  });

  it("a bare {name, context} is an event too", () => {
    expect(actionOf(prepareSurface(withAction({ name: "pick", context: { a: 1 } })))).toMatchObject(
      {
        name: "pick",
        context: { a: 1 },
      },
    );
  });

  it("context bound to the data model is read at conversion", () => {
    const ops = [
      ...withAction(event("pick", { who: { path: "/who" } })),
      { version: "v0.9", updateDataModel: { surfaceId: SURFACE, path: "/who", contents: "me" } },
    ];
    expect(actionOf(prepareSurface(ops))).toMatchObject({ context: { who: "me" } });
  });

  it("a userMessage is lowered to text for the message box, and is not an event action", () => {
    const p = drawn(
      prepareSurface(withAction({ event: { name: "x", userMessage: "Please review the plan" } })),
    );
    expect(p.eventActions).toBe(0);
    expect(find(p.spec, "Button")?.$action).toMatchObject({
      name: USER_MESSAGE,
      context: { text: "Please review the plan" },
    });
    for (const bad of [
      { userMessage: "" },
      { userMessage: 7 },
      { userMessage: "x".repeat(4001) },
      { userMessage: { path: "/m" } },
    ]) {
      expect(refused(prepareSurface(withAction({ event: { name: "x", ...bad } }))).rule).toBe(
        "action",
      );
    }
    drawn(prepareSurface(withAction({ event: { name: "x", userMessage: "x".repeat(4000) } })));
  });

  it("a function call other than openUrl is ignored: a button that does nothing", () => {
    const p = drawn(prepareSurface(withAction({ functionCall: { call: "closeModal", args: {} } })));
    expect(p.eventActions).toBe(0);
    expect(find(p.spec, "Button")?.$action).toMatchObject({ name: UNSUPPORTED });
  });

  it("refuses what the renderer cannot send", () => {
    const cases: [string, unknown][] = [
      ["no action", undefined],
      ["action is text", "go"],
      ["no name", { event: {} }],
      ["name is empty", event("")],
      ["name is a number", { event: { name: 1 } }],
      ["name over 256 bytes", event("x".repeat(257))],
      ["a name in the reserved space", event(OPEN_URL)],
      ["a name in the reserved space (another)", event("vymalo:anything")],
      ["context is a list", { event: { name: "go", context: [] } }],
      ["context is text", { event: { name: "go", context: "x" } }],
      ["a function call that is not {call}", { functionCall: "openUrl" }],
    ];
    for (const [label, action] of cases) {
      const ops = surface([column("root", ["go", "go_label"]), ...labelled("go", "Go", action)]);
      if (action === undefined)
        (ops[1] as { updateComponents: { components: Rec[] } }).updateComponents.components[1] = {
          id: "go",
          component: "Button",
          child: "go_label",
        };
      expect(refused(prepareSurface(ops)).rule, label).toBe("action");
    }
    // a name of exactly 256 bytes is fine
    drawn(prepareSurface(withAction(event("x".repeat(256)))));
  });

  it("a function value inside an action context is refused too", () => {
    expect(refused(prepareSurface(withAction(event("go", { at: { call: "now" } })))).rule).toBe(
      "function",
    );
  });
});

describe("inputs", () => {
  const form = (extra: Rec[] = [], context: Rec = { email: { path: "/email" } }) =>
    surface(
      [
        column("root", ["field", "box", "go", "go_label", ...extra.map((c) => c.id as string)]),
        { id: "field", component: "TextField", label: "Email", value: { path: "/email" } },
        { id: "box", component: "CheckBox", label: "Agree", value: { path: "/agree" } },
        ...labelled("go", "Send", event("submit", context)),
        ...extra,
      ],
      { email: "a@example.com", agree: false },
    );

  it("an input is kept apart from the converter, with its field and its first value", () => {
    const p = drawn(prepareSurface(form()));
    expect(p.fields).toEqual({ "/email": "a@example.com", "/agree": false });
    expect(find(p.spec, "vymalo.TextField")).toMatchObject({
      label: "Email",
      fieldKey: "/email",
      value: "a@example.com",
    });
    expect(find(p.spec, "vymalo.CheckBox")).toMatchObject({ label: "Agree", fieldKey: "/agree" });
  });

  it("a binding to an input in an action context becomes 'the value at the click'", () => {
    const p = drawn(prepareSurface(form()));
    expect(find(p.spec, "Button")?.$action).toMatchObject({
      context: { email: { $field: "/email" } },
    });
  });

  it("an input bound to a relative path, or inside a template, is refused", () => {
    const relative = surface([
      column("root", ["f"]),
      { id: "f", component: "TextField", value: { path: "email" } },
    ]);
    expect(refused(prepareSurface(relative)).rule).toBe("field");
    const inList = surface(
      [
        { id: "root", component: "Column", children: { componentId: "f", path: "/items" } },
        { id: "f", component: "TextField", value: { path: "/email" } },
      ],
      { items: [1, 2] },
    );
    expect(refused(prepareSurface(inList)).rule).toBe("field");
  });
});

describe("what the validator does not trust", () => {
  it("survives hostile keys and values without throwing", () => {
    const hostile = JSON.parse(
      '{"__proto__":{"polluted":true},"constructor":{"prototype":{"polluted":true}},"id":"root","component":"Column","children":["t"]}',
    );
    const ops = surface([hostile, { id: "t", component: "Text", text: "x", ["__proto__"]: 1 }]);
    const p = prepareSurface(ops);
    expect(({} as Rec).polluted).toBeUndefined();
    expect(["surface", "refused"]).toContain(p.kind);
  });

  it("a value of any type where text is expected is refused or drawn without it, never thrown", () => {
    for (const v of NOT_TEXT) {
      const p = prepareSurface(
        surface([column("root", ["t"]), { id: "t", component: "Text", text: v }]),
      );
      expect(["surface", "refused"]).toContain(p.kind);
    }
  });
});

/** The operations of a surface that names this app's catalog. */
const ours = (components: Rec[], catalogId = OWN_CATALOG_ID) =>
  surface(components, undefined, "v0.9", catalogId);

/** The surface of a build that has one more component than the real one, `Note`, and a version. */
const withNote = (version: number): OwnCatalog => ({
  ...OWN_CATALOG,
  version,
  catalog: {
    ...OWN_CATALOG.catalog,
    components: {
      ...OWN_CATALOG.catalog.components,
      Note: {
        type: "object",
        properties: {
          id: { type: "string", minLength: 1, maxLength: 256 },
          component: { const: "Note" },
          text: { type: "string", maxLength: 10 },
        },
        required: ["id", "component", "text"],
        additionalProperties: false,
      },
    },
  },
});

describe("catalogs (ADR 0023): the basic one, ours, and no other", () => {
  const hello = [column("root", ["t"]), text("t", "hello")];

  it("the three spellings of the basic catalog's id, and no id at all, are the basic catalog", () => {
    for (const id of BASIC_CATALOG_IDS) {
      expect(drawn(prepareSurface(surface(hello, undefined, "v0.9", id))).surfaceId).toBe(SURFACE);
    }
    const none = surface(hello).map((op) =>
      "createSurface" in op ? { version: op.version, createSurface: { surfaceId: SURFACE } } : op,
    );
    drawn(prepareSurface(none));
  });

  it("any other catalog is refused, and the reason names it (cut, without control characters)", () => {
    for (const id of [
      "basic",
      "https://example.com/catalog.json",
      "https://agents.vymalo.com/a2ui/catalogs/chat/v2",
    ]) {
      const r = refused(prepareSurface(ours(hello, id)));
      expect(r.rule).toBe("catalog");
      expect(r.reason).toContain("does not have");
    }
    const long = refused(
      prepareSurface(ours(hello, `https://example.com/\u0007${"x".repeat(200)}`)),
    );
    expect(long.reason).not.toContain("\u0007");
    expect(long.reason.length).toBeLessThan(160);
  });

  it("a catalog id that is not text is refused", () => {
    for (const id of [1, null, {}, ["a"]]) {
      const ops = ours(hello).map((op) =>
        "createSurface" in op
          ? { version: op.version, createSurface: { surfaceId: SURFACE, catalogId: id } }
          : op,
      );
      expect(refused(prepareSurface(ops)).rule).toBe("catalog");
    }
  });

  it("a surface that names two catalogs is refused: a later createSurface starts it over", () => {
    const ops = [
      ...ours(hello),
      { version: "v0.9", createSurface: { surfaceId: SURFACE, catalogId: "https://x.test/c" } },
    ];
    expect(refused(prepareSurface(ops)).rule).toBe("catalog");
  });

  it("ours: Text and Column are drawn like their basic namesakes", () => {
    const p = drawn(
      prepareSurface(
        ours([
          column("root", ["h", "t"], { align: "center" }),
          { id: "h", component: "Text", text: "Title", variant: "h2" },
          text("t", "<b>words</b>"),
        ]),
      ),
    );
    expect(find(p.spec, "Header")).toMatchObject({ text: "Title" });
    expect(find(p.spec, "Markdown")).toMatchObject({ value: "<b>words</b>" });
    expect(find(p.spec, "Col")).toMatchObject({ align: "center" });
  });

  it("ours: a basic component the catalog does not list is refused with the rule catalog", () => {
    const r = refused(
      prepareSurface(ours([column("root", ["b"]), ...labelled("b", "Go", event("go"))])),
    );
    expect(r.rule).toBe("catalog");
    expect(r.reason).toContain('"Button"');
    expect(r.reason).toContain(`version ${OWN_CATALOG.version}`);
  });

  it("ours: an instance that breaks its schema is refused with the rule schema, naming the component and the property", () => {
    const r = refused(
      prepareSurface(
        ours([column("root", ["t"]), { id: "t", component: "Text", text: "x".repeat(4001) }]),
      ),
    );
    expect(r.rule).toBe("schema");
    expect(r.reason).toMatch(/^component "t" \(Text\) text: String is too long/);
    // a property the schema does not list, a binding where a literal is required
    expect(
      refused(
        prepareSurface(
          ours([column("root", ["t"]), { id: "t", component: "Text", text: "x", color: "red" }]),
        ),
      ).rule,
    ).toBe("schema");
    expect(
      refused(
        prepareSurface(
          ours([column("root", ["t"]), { id: "t", component: "Text", text: { path: "/x" } }]),
        ),
      ).rule,
    ).toBe("schema");
    // a Column's children are a list of ids, never a template
    expect(
      refused(
        prepareSurface(
          ours([
            { id: "root", component: "Column", children: { componentId: "t", path: "/xs" } },
            text("t", "x"),
          ]),
        ),
      ).rule,
    ).toBe("schema");
  });

  it("ours: the limits of the schema are exact", () => {
    const col = (n: number) => [
      column(
        "root",
        Array.from({ length: n }, (_, i) => `t${i}`),
      ),
      ...Array.from({ length: Math.min(n, 51) }, (_, i) => text(`t${i}`, "x")),
    ];
    drawn(prepareSurface(ours(col(50))));
    expect(refused(prepareSurface(ours(col(51)))).rule).toBe("schema");
    drawn(prepareSurface(ours([column("root", ["t"]), text("t", "x".repeat(4000))])));
  });

  it("ours, a component of a newer catalog in a thread whose catalog is newer: 'newer', never drawn", () => {
    const ops = ours([
      column("root", ["t", "g"]),
      text("t", "before"),
      { id: "g", component: "Gizmo", n: 1 },
    ]);
    const p = prepareSurface(ops, { threadVersion: OWN_CATALOG.version + 1 });
    expect(p).toEqual({ kind: "newer", component: "Gizmo" });
  });

  it("ours, a component this build lacks in a thread that is not newer: the agent's fault, refused", () => {
    const ops = ours([column("root", ["g"]), { id: "g", component: "Gizmo" }]);
    for (const threadVersion of [undefined, OWN_CATALOG.version, OWN_CATALOG.version - 1]) {
      const r = refused(prepareSurface(ops, { threadVersion }));
      expect(r.rule).toBe("catalog");
      expect(r.reason).toContain('"Gizmo"');
    }
  });

  it("'newer' comes before the other defects of the surface: it is not the agent's fault", () => {
    const ops = ours([
      column("root", ["t", "g"]),
      { id: "t", component: "Text", text: "" },
      { id: "g", component: "Gizmo" },
    ]);
    expect(prepareSurface(ops, { threadVersion: OWN_CATALOG.version + 1 }).kind).toBe("newer");
    expect(refused(prepareSurface(ops, { threadVersion: OWN_CATALOG.version })).rule).toBe(
      "catalog",
    );
  });

  it("a newer thread does not excuse a surface of the basic catalog: its unknown component is refused", () => {
    const ops = surface([column("root", ["g"]), { id: "g", component: "Gizmo" }]);
    const r = refused(prepareSurface(ops, { threadVersion: 99 }));
    expect(r.rule).toBe("vocabulary");
  });

  it("a component of our catalog named under the basic catalog is refused: the surface did not ask for ours", () => {
    const build = withNote(3);
    const ops = surface([column("root", ["n"]), { id: "n", component: "Note", text: "hi" }]);
    const r = refused(prepareSurface(ops, { catalog: build }));
    expect(r.rule).toBe("catalog");
    expect(r.reason).toContain("does not name");
  });

  it("a build with another catalog validates against that one (the catalog is a parameter)", () => {
    const build = withNote(3);
    expect(
      prepareSurface(ours([column("root", ["n"]), { id: "n", component: "Note", text: "hi" }]), {
        catalog: build,
      }).kind,
    ).not.toBe("refused");
  });

  it("a thread with the same or an older version is not 'newer' than this build", () => {
    const ops = ours([column("root", ["g"]), { id: "g", component: "Gizmo" }]);
    expect(refused(prepareSurface(ops, { catalog: withNote(3), threadVersion: 3 })).rule).toBe(
      "catalog",
    );
    expect(prepareSurface(ops, { catalog: withNote(3), threadVersion: 4 }).kind).toBe("newer");
  });

  it("BASIC_CATALOG is one of the basic ids", () => {
    expect(BASIC_CATALOG_IDS as readonly string[]).toContain(BASIC_CATALOG);
  });
});
