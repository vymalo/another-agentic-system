import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { OWN_COMPILED } from "../src/features/chat/lib/a2ui/catalog/validate";
import {
  aliveFrom,
  type Copy,
  type Files,
  placeCopies,
  progress,
  surfaceResult,
} from "./surfaces-v8";

/**
 * The second implementation of version 8 (`surfaces-v8.ts`, written from the contract page alone) against the page's
 * own conformance vectors: the same blocks the web's tests read (`lib/a2ui/catalog/v8.test.ts`). Two implementations
 * that agree on every case are the proof that the page says enough for a client that is not the web.
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

describe("the page's vectors, judged by the second implementation", () => {
  it("what a surface may be and hold (v8-surfaces)", () => {
    const { files, cases } = block<{
      files: Files;
      cases: { because: string; surfaceId?: string; components: Rec[]; result: string }[];
    }>("v8-surfaces");
    for (const c of cases) {
      const got = surfaceResult(c.surfaceId ?? "s1", c.components, files, (comp) =>
        OWN_COMPILED.check(String(comp.component), comp),
      );
      expect(got, c.because).toBe(c.result);
    }
    expect(cases.length).toBe(34);
  });

  it("a checklist's progress (checklists)", () => {
    const cases =
      block<{ because: string; items: { state: string }[]; progress: [number, number] | null }[]>(
        "checklists",
      );
    for (const c of cases.filter((x) => x.progress !== null)) {
      expect(progress(c.items), c.because).toEqual(c.progress);
    }
  });

  it("the operations that are alive (restarts)", () => {
    const cases = block<{ because: string; operations: Rec[]; from: number }[]>("restarts");
    for (const c of cases) expect(aliveFrom(c.operations), c.because).toBe(c.from);
  });

  it("what each copy of a surface draws, and the pinned plan (copies)", () => {
    const cases =
      block<{ because: string; copies: Copy[]; result: string[]; pinned: string | null }[]>(
        "copies",
      );
    for (const c of cases) {
      const got = placeCopies(c.copies);
      expect(got.result, c.because).toEqual(c.result);
      expect(got.pinned, c.because).toBe(c.pinned);
    }
  });
});
