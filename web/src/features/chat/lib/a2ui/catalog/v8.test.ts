import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";
import { aliveOperations, prepareSurface } from "../prepare";
import { progressOf, progressWords, readItems } from "../status";
import { answeredAfter, latestPlan, latestSurface, placeOf, planDataOf } from "../surfaces";
import lock from "./catalog.lock.json";
import { catalogDigest } from "./digest";
import { asVersion7, V7_DIGEST, VERSION_7 } from "./earlier";
import { OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 8: a question that shows components of its surface (`show`), a checklist that can drop an
 * item and note it, a surface that is sent again, and the plan. The schema on its own, then the contract page
 * itself: its vectors are run through the validator and the functions the web uses, so that the page cannot
 * say what the code does not; and version 8 is version 7 and additions, digest for digest.
 */

type Rec = Record<string, unknown>;
const check = (component: string, instance: unknown) =>
  compileCatalog(OWN_CATALOG.catalog).check(component, instance);
const ask = (q: Rec) =>
  check("Choices", {
    id: "ask",
    component: "Choices",
    questions: [{ id: "q", question: "Q?", ...q }],
  });
const item = (over: Rec) => check("Checklist", { id: "c", component: "Checklist", items: [over] });

const doc = readFileSync(
  path.resolve(import.meta.dirname, "../../../../../../../docs/api/ui-catalog-v1.md"),
  "utf8",
);
const block = <T>(kind: string): T => {
  const found = [...doc.matchAll(new RegExp(`\`\`\`json ${kind}\\n([\\s\\S]*?)\`\`\``, "g"))];
  expect(found.length, `one \`\`\`json ${kind} block in the contract`).toBe(1);
  return JSON.parse(found[0]?.[1] ?? "") as T;
};
const said = (name: string): number => {
  const m = new RegExp(`\\(\`${name}\`:[^]*?the block holds\\s+\\*\\*(\\d+) cases\\*\\*`).exec(doc);
  expect(m, `the page states how many cases ${name} holds`).not.toBeNull();
  return Number(m?.[1]);
};

describe("version 8", () => {
  it("is catalog version 8 or later", () => {
    expect(OWN_CATALOG.version).toBeGreaterThanOrEqual(8);
    expect(lock.version).toBeGreaterThanOrEqual(8);
  });

  it("tells an agent what show is, what it may name and that a Checklist can be dropped and noted", () => {
    const choices = (OWN_CATALOG.catalog.components.Choices as { description: string }).description;
    for (const word of ["show", "version 8", "Image", "Mermaid", "Chart", "alt"]) {
      expect(choices, word).toContain(word);
    }
    const list = (OWN_CATALOG.catalog.components.Checklist as { description: string }).description;
    for (const word of ["dropped", "version 8", "plan-", "surfaceId"]) {
      expect(list, word).toContain(word);
    }
  });
});

describe("the schema of show", () => {
  it("takes 1 to 4 ids of 1 to 256 characters, on a question of any type", () => {
    expect(ask({ show: ["a"] })).toBeUndefined();
    expect(ask({ show: ["a", "b", "c", "d"] })).toBeUndefined();
    expect(ask({ show: ["x".repeat(256)] })).toBeUndefined();
    expect(ask({ type: "confirm", show: ["a"] })).toBeUndefined();
    expect(ask({ type: "number", show: ["a"] })).toBeUndefined();
    expect(ask({ type: "text", show: ["a"] })).toBeUndefined();
  });

  it("refuses none, five, an id of nothing or of 257 characters, and what is not a list of strings", () => {
    for (const bad of [
      [],
      ["a", "b", "c", "d", "e"],
      [""],
      ["x".repeat(257)],
      "a",
      [1],
      [null],
      {},
    ]) {
      expect(ask({ show: bad }), JSON.stringify(bad)).toMatch(/^questions\.0\.show/);
    }
  });
});

describe("the schema of the checklist", () => {
  it("takes the state dropped and a note of up to 200 characters", () => {
    expect(item({ label: "A", state: "dropped" })).toBeUndefined();
    expect(item({ label: "A", state: "dropped", note: "x".repeat(200) })).toBeUndefined();
    expect(item({ label: "A", state: "done", note: "" })).toBeUndefined();
    expect(item({ label: "A", state: "todo", note: "x".repeat(201) })).toMatch(/note/);
    expect(item({ label: "A", state: "Dropped" })).toMatch(/state/);
    expect(item({ label: "A", state: "skipped" })).toMatch(/state/);
  });
});

describe("the contract page (docs/api/ui-catalog-v1.md, Version 8): a checklist", () => {
  type Case = { because: string; items: Rec[]; progress: [number, number] | null; words?: string };
  const cases = block<Case[]>("checklists");

  it("each case is judged by the schema and counted as the page says", () => {
    for (const c of cases) {
      const refused = check("Checklist", { id: "c", component: "Checklist", items: c.items });
      if (c.progress === null) {
        expect(refused, c.because).toBeDefined();
        continue;
      }
      expect(refused, c.because).toBeUndefined();
      const p = progressOf(readItems(c.items));
      expect([p.done, p.total], c.because).toEqual(c.progress);
      expect(progressWords(p), c.because).toBe(c.words);
    }
  });

  it("states how many cases there are, and every one of them is there", () => {
    expect(cases.length).toBe(said("checklists"));
    expect(cases.filter((c) => c.progress === null).length).toBe(8);
    expect(cases.length).toBe(16);
  });
});

describe("the contract page: what a surface may be and hold", () => {
  type Surface = {
    because: string;
    surfaceId?: string;
    catalog?: "basic";
    components: Rec[];
    operationsAfter?: Rec[];
    noCreate?: boolean;
    result: string;
  };
  const { files, cases } = block<{
    files: { sha256: string; kind: "image" | "other" }[];
    cases: Surface[];
  }>("v8-surfaces");
  const kept = files.map((f) => ({
    sha256: f.sha256,
    preview: f.kind === "image" ? ("image" as const) : null,
    mimeType: f.kind === "image" ? "image/png" : "text/plain",
  }));
  const BASIC = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
  /**
   * The page's vectors are written in v1's spelling, because `ui-catalog-v1.md` is the record of v1. Read under
   * v2 they say `"version": "1.0"` and name the v1.0 basic catalog, and nothing else in them changes
   * (`ui-catalog-v2.md`, section 1).
   */
  const atV1 = (ops: Rec[] = []): Rec[] =>
    ops.map((op) => ({
      ...op,
      version: "1.0",
      ...(typeof op.createSurface === "object" &&
      op.createSurface !== null &&
      (op.createSurface as Rec).catalogId !== undefined
        ? {
            createSurface: {
              ...(op.createSurface as Rec),
              catalogId: BASIC,
            },
          }
        : {}),
    }));
  const operations = (c: Surface) =>
    c.noCreate
      ? atV1(c.operationsAfter ?? [])
      : [...operationsFull(c), ...atV1(c.operationsAfter ?? [])];
  const operationsFull = (c: Surface) => [
    {
      version: "1.0",
      createSurface: {
        surfaceId: c.surfaceId ?? "s1",
        catalogId: c.catalog === "basic" ? BASIC : OWN_CATALOG.catalogId,
      },
    },
    {
      version: "1.0",
      updateComponents: { surfaceId: c.surfaceId ?? "s1", components: c.components },
    },
  ];
  const resultOf = (c: Surface): string => {
    const got = prepareSurface(operations(c), {
      files: kept,
      threadVersion: 8,
    });
    return got.kind === "surface"
      ? "drawn"
      : got.kind === "refused"
        ? `refused:${got.rule}`
        : got.kind;
  };

  it("each case is drawn, left pending or refused as the page says", () => {
    for (const c of cases) expect(resultOf(c), c.because).toBe(c.result);
  });

  it("states how many cases there are, and how many of each result: deleting one fails", () => {
    expect(cases.length).toBe(said("v8-surfaces"));
    const per: Record<string, number> = {};
    for (const c of cases) per[c.result] = (per[c.result] ?? 0) + 1;
    expect(per).toEqual({
      drawn: 14,
      pending: 3,
      deleted: 1,
      "refused:schema": 17,
      "refused:expansion": 2,
      "refused:artifact": 2,
      "refused:action": 5,
      "refused:shape": 2,
    });
  });

  it("every component a question may show has a case that draws it, and the closed list is seven", () => {
    const drawn = new Set<string>();
    for (const c of cases.filter((x) => x.result === "drawn")) {
      for (const comp of c.components) drawn.add(String(comp.component));
    }
    for (const name of ["Text", "Image", "Mermaid", "Code", "Cards", "Callout", "Chart"]) {
      expect(drawn.has(name), name).toBe(true);
    }
    expect(doc).toContain("a closed list of seven components");
    // the page says that a plan refuses an input with no action, and a Button of an event, and allows a link
    const plans = cases.filter((c) => c.surfaceId === "plan-1" && c.catalog === "basic");
    expect(plans.map((c) => c.result)).toEqual([
      "refused:action",
      "refused:action",
      "refused:action",
      "drawn",
    ]);
  });

  it("a plan that holds a form is refused whole, and a form is drawn under every id that is not a plan", () => {
    const forms = cases.filter((c) => c.components.some((x) => x.component === "Choices"));
    const byId = Object.fromEntries(
      forms
        .filter((c) => c.surfaceId?.toLowerCase().startsWith("plan"))
        .map((c) => [c.surfaceId, c.result]),
    );
    expect(byId).toEqual({
      "plan-1": "refused:action",
      "plan-": "drawn",
      plan1: "drawn",
      "Plan-1": "drawn",
    });
  });
});

describe("the contract page: a surface that is started over", () => {
  const cases = block<{ because: string; operations: unknown[]; from: number }[]>("restarts");

  it("only what is alive is read: the operations from the last createSurface of the surface", () => {
    for (const c of cases) {
      expect(aliveOperations(c.operations).length, c.because).toBe(c.operations.length - c.from);
    }
    expect(cases.length).toBe(said("restarts"));
    expect(cases.length).toBe(6);
  });

  it("a surface sent again whole many times counts only its last copy against the size limit", () => {
    const create = (s: string) => ({
      version: "1.0",
      createSurface: { surfaceId: s, catalogId: OWN_CATALOG.catalogId },
    });
    const update = (s: string, n: number) => ({
      version: "1.0",
      updateComponents: {
        surfaceId: s,
        components: [
          { id: "root", component: "Column", children: ["list"] },
          {
            id: "list",
            component: "Checklist",
            items: [{ label: `${"x".repeat(190)} ${n}`, state: "doing" }],
          },
        ],
      },
    });
    const sent = Array.from({ length: 500 }, (_, i) => [
      create("plan-1"),
      update("plan-1", i),
    ]).flat();
    expect(JSON.stringify(sent).length).toBeGreaterThan(64 * 1024);
    const got = prepareSurface(sent, { threadVersion: 8 });
    expect(got.kind).toBe("surface");
    // the same copies one surface that never started over are over the limit: the rule is what saves them
    const accumulated = Array.from({ length: 500 }, (_, i) => update("s1", i));
    expect(prepareSurface([create("s1"), ...accumulated], { threadVersion: 8 })).toMatchObject({
      kind: "refused",
      rule: "size",
    });
  });
});

describe("the contract page: a surface sent again, and the plan", () => {
  type Copy = { key: string; surfaceId: string; marker: string; deleted?: boolean };
  type Item = Copy | { person: true };
  const cases =
    block<{ because: string; copies: Item[]; result: string[]; pinned: string | null }[]>("copies");
  const isCopy = (i: Item): i is Copy => !("person" in i);
  const transcript = (items: Item[]) =>
    items.map((c, i) =>
      isCopy(c)
        ? {
            id: `m${i}`,
            role: "assistant",
            content: [
              {
                type: "data",
                name: activityPartName(ACTIVITY.surface),
                data: {
                  surface: c.key,
                  marker: c.marker,
                  a2ui_operations: [
                    c.deleted
                      ? { version: "1.0", deleteSurface: { surfaceId: c.surfaceId } }
                      : {
                          version: "1.0",
                          createSurface: { surfaceId: c.surfaceId, catalogId: "x" },
                        },
                  ],
                },
              },
            ],
          }
        : { id: `m${i}`, role: "user", content: [{ type: "text", text: "ok go ahead" }] },
    );
  const markerOf = (data: unknown) => String((data as Rec).marker);
  const deletedOf = (data: unknown) =>
    Boolean(((data as Rec).a2ui_operations as Rec[]).at(-1)?.deleteSurface);
  const says = (data: unknown, label: string) =>
    deletedOf(data) ? "gone" : `${label}:${markerOf(data)}`;

  it("each copy draws what the page says, and the pinned plan is the one it names", () => {
    for (const c of cases) {
      const messages = transcript(c.copies);
      const got: string[] = [];
      c.copies.forEach((copy, i) => {
        if (!isCopy(copy)) return;
        const place = placeOf(messages, `m${i}`, copy.key, copy.surfaceId);
        if (place === "own") got.push(says(latestSurface(messages, copy.key)?.data, "draws"));
        else if (place === "older") {
          got.push(deletedOf(latestSurface(messages, copy.key)?.data) ? "gone" : "older");
        } else if (place === "newest") {
          got.push(says(planDataOf(messages, `m${i}`, copy.key, copy.surfaceId), "draws"));
        } else if (place === "frozen") {
          got.push(says(planDataOf(messages, `m${i}`, copy.key, copy.surfaceId), "older-plan"));
        } else got.push(place);
      });
      expect(got, c.because).toEqual(c.result);
      const pinned = latestPlan(messages);
      expect(pinned === undefined ? null : markerOf(pinned.data), c.because).toBe(c.pinned);
    }
    expect(cases.length).toBe(said("copies"));
    expect(cases.length).toBe(17);
  });

  it("an answered copy and an open one: answered belongs to the copy (answered)", () => {
    const answered = block<{ because: string; items: string[]; result: string[] }[]>("answered");
    for (const c of answered) {
      const messages = c.items.map((it, i) =>
        it === "person"
          ? { id: `m${i}`, role: "user", content: [{ type: "text", text: "ok" }] }
          : {
              id: `m${i}`,
              role: "assistant",
              content: [{ type: "data", name: activityPartName(ACTIVITY.surface), data: {} }],
            },
      );
      const got = c.items.flatMap((it, i) =>
        it === "person" ? [] : [answeredAfter(messages, `m${i}`) ? "answered" : "open"],
      );
      expect(got, c.because).toEqual(c.result);
    }
    expect(answered.length).toBe(said("answered"));
    expect(answered.length).toBe(4);
  });

  it("the progress of a plan is the sum over its checklists, a dropped item in neither number (plan-progress)", () => {
    const cases =
      block<{ because: string; checklists: Rec[][]; progress: [number, number] | null }[]>(
        "plan-progress",
      );
    for (const c of cases) {
      const ids = c.checklists.map((_, i) => `l${i}`);
      const components = [
        { id: "root", component: "Column", children: ids.length ? ids : ["t"] },
        ...(ids.length ? [] : [{ id: "t", component: "Text", text: "Nothing to count" }]),
        ...c.checklists.map((items, i) => ({ id: `l${i}`, component: "Checklist", items })),
      ];
      const got = prepareSurface(
        [
          {
            version: "1.0",
            createSurface: { surfaceId: "plan-1", catalogId: OWN_CATALOG.catalogId },
          },
          { version: "1.0", updateComponents: { surfaceId: "plan-1", components } },
        ],
        { threadVersion: 8 },
      );
      expect(got.kind, c.because).toBe("surface");
      const progress = got.kind === "surface" ? got.progress : undefined;
      expect(progress ? [progress.done, progress.total] : null, c.because).toEqual(c.progress);
    }
    expect(cases.length).toBe(said("plan-progress"));
    expect(cases.length).toBe(4);
  });

  it("the id of a plan is plan- and one more character, in lower case", () => {
    const planned = (surfaceId: string) =>
      latestPlan(transcript([{ key: "a2ui-1", surfaceId, marker: "A" }])) !== undefined;
    expect([planned("plan-1"), planned("plan-x"), planned("plan-run-7")]).toEqual([
      true,
      true,
      true,
    ]);
    expect([planned("plan-"), planned("plan1"), planned("Plan-1"), planned("s1")]).toEqual([
      false,
      false,
      false,
      false,
    ]);
  });
});

/**
 * Version 8 only adds: take `show`, `dropped`, `note` and the new words of two descriptions away and what is
 * left is the catalog of version 7, digest for digest. So an agent that knows only version 7 keeps working
 * (every Choices and Checklist it can write is still valid), and a client of version 7 reads the same
 * components and refuses a member it does not know, at its place.
 */
describe("version 8 is version 7 and additions", () => {
  it("takes the additions away and the digest is the one version 7 shipped", async () => {
    expect(await catalogDigest(asVersion7(OWN_CATALOG.catalog))).toBe(V7_DIGEST);
  });

  it("the checked-in catalog of version 7 is the released one, and what is left of version 8 is equal to it, descriptions too", async () => {
    expect(await catalogDigest(VERSION_7)).toBe(V7_DIGEST);
    expect(asVersion7(OWN_CATALOG.catalog)).toEqual(VERSION_7);
  });

  it("a client of version 7 refuses show, a dropped item and a note, at the place of each", () => {
    const v7 = compileCatalog(asVersion7(OWN_CATALOG.catalog));
    const form = (q: Rec) => ({
      id: "ask",
      component: "Choices",
      questions: [
        { id: "a", question: "A?", type: "text" },
        { id: "q", question: "Q?", ...q },
      ],
    });
    expect(v7.check("Choices", form({ type: "text", show: ["t1"] }))).toMatch(
      /^questions\.1\.show: /,
    );
    const list = (over: Rec) => ({
      id: "c",
      component: "Checklist",
      items: [{ label: "A", state: "todo", ...over }],
    });
    expect(v7.check("Checklist", list({ state: "dropped" }))).toMatch(/^items\.0\.state: /);
    expect(v7.check("Checklist", list({ note: "x" }))).toMatch(/^items\.0\.note: /);
    // and what it knows it still takes
    expect(v7.check("Choices", form({ type: "text" }))).toBeUndefined();
    expect(v7.check("Checklist", list({ state: "done" }))).toBeUndefined();
  });

  it("a Choices and a Checklist of version 7 are valid in version 8", () => {
    const form = {
      id: "ask",
      component: "Choices",
      questions: [{ id: "a", question: "A?", type: "confirm" }],
    };
    const list = { id: "c", component: "Checklist", items: [{ label: "A", state: "failed" }] };
    const v7 = compileCatalog(asVersion7(OWN_CATALOG.catalog));
    for (const [name, value] of [
      ["Choices", form],
      ["Checklist", list],
    ] as const) {
      expect(v7.check(name, value)).toBeUndefined();
      expect(check(name, value)).toBeUndefined();
    }
  });
});
