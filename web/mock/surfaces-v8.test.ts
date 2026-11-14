import { readFileSync } from "node:fs";
import path from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import { describe, expect, it } from "vitest";
import {
  aliveFrom,
  answeredCopies,
  type Files,
  type Item,
  placeCopies,
  planProgress,
  progress,
  type SurfaceCase,
  surfaceResult,
} from "./surfaces-v8";

/**
 * The second implementation of version 8 (`surfaces-v8.ts`, written from the contract page and the catalog document
 * alone) against the page's own conformance vectors: the same blocks the web's tests read (`lib/a2ui/catalog/
 * v8.test.ts`). The component schemas are compiled here, by Ajv, from `catalog.json`: nothing of the web's validator
 * is imported. Two implementations that agree on every case are the proof that the page says enough for a client that
 * is not the web.
 */

type Rec = Record<string, unknown>;
const doc = readFileSync(
  path.resolve(import.meta.dirname, "../../docs/api/ui-catalog-v1.md"),
  "utf8",
);
const block = <T>(kind: string): T => {
  const found = [...doc.matchAll(new RegExp(`\`\`\`json ${kind}\\n([\\s\\S]*?)\`\`\``, "g"))];
  expect(found.length, `one ${kind} block`).toBe(1);
  return JSON.parse(found[0]?.[1] ?? "") as T;
};

// CJS interop: depending on the loader, the default import is the class or the module object.
const Ajv = ((Ajv2020 as unknown as { default?: typeof Ajv2020 }).default ??
  Ajv2020) as typeof Ajv2020;
const catalog = JSON.parse(
  readFileSync(
    path.resolve(import.meta.dirname, "../src/features/chat/lib/a2ui/catalog/catalog.json"),
    "utf8",
  ),
) as { components: Record<string, Rec> };
const ajv = new Ajv({ strict: false, allErrors: false });
const validators = new Map(Object.entries(catalog.components).map(([n, s]) => [n, ajv.compile(s)]));
const check = (c: Rec): string | undefined => {
  const v = validators.get(String(c.component));
  if (!v) return "a component this catalog does not have";
  return v(c) ? undefined : (ajv.errorsText(v.errors) ?? "invalid");
};

describe("the page's vectors, judged by the second implementation", () => {
  it("what a surface may be and hold (v8-surfaces)", () => {
    const { files, cases } = block<{
      files: Files;
      cases: (SurfaceCase & { because: string; result: string })[];
    }>("v8-surfaces");
    for (const c of cases) expect(surfaceResult(c, files, check), c.because).toBe(c.result);
    expect(cases.length).toBe(46);
  });

  it("a checklist's progress and its schema (checklists)", () => {
    const cases =
      block<{ because: string; items: { state: string }[]; progress: [number, number] | null }[]>(
        "checklists",
      );
    for (const c of cases) {
      const refused = check({ id: "c", component: "Checklist", items: c.items });
      if (c.progress === null) expect(refused, c.because).toBeDefined();
      else {
        expect(refused, c.because).toBeUndefined();
        expect(progress(c.items), c.because).toEqual(c.progress);
      }
    }
  });

  it("the sums of a plan's checklists (plan-progress)", () => {
    const cases =
      block<
        { because: string; checklists: { state: string }[][]; progress: [number, number] | null }[]
      >("plan-progress");
    for (const c of cases) expect(planProgress(c.checklists), c.because).toEqual(c.progress);
  });

  it("the operations that are alive (restarts)", () => {
    const cases = block<{ because: string; operations: Rec[]; from: number }[]>("restarts");
    for (const c of cases) expect(aliveFrom(c.operations), c.because).toBe(c.from);
  });

  it("what each copy of a surface draws, and the pinned plan (copies)", () => {
    const cases =
      block<{ because: string; copies: Item[]; result: string[]; pinned: string | null }[]>(
        "copies",
      );
    for (const c of cases) {
      const got = placeCopies(c.copies);
      expect(got.result, c.because).toEqual(c.result);
      expect(got.pinned, c.because).toBe(c.pinned);
    }
  });

  it("an answered copy and an open one (answered)", () => {
    const cases = block<{ because: string; items: string[]; result: string[] }[]>("answered");
    for (const c of cases) expect(answeredCopies(c.items), c.because).toEqual(c.result);
  });
});
