import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { receive, refusalOf, refusalText } from "./receiver";

/**
 * The mock's receiver against the contract page's own conformance vectors (docs/api/ui-catalog-v1.md,
 * "Version 7"): the same cases the web's client check passes, by an implementation written from the
 * page alone.
 */

type Rec = Record<string, unknown>;
const doc = readFileSync(
  path.resolve(import.meta.dirname, "../../docs/api/ui-catalog-v1.md"),
  "utf8",
);
const block = (kind: string): unknown => {
  const found = [...doc.matchAll(new RegExp(`\`\`\`json ${kind}\\n([\\s\\S]*?)\`\`\``, "g"))];
  expect(found.length).toBe(1);
  return JSON.parse(found[0]?.[1] ?? "");
};

describe("the receiver judges the vectors of the contract", () => {
  const vectors = block("vectors") as { question: Rec; cases: [string[], string | null][] }[];

  it("gives the rule the page gives, for every case", () => {
    let n = 0;
    for (const { question, cases } of vectors) {
      for (const [values, rule] of cases) {
        expect(
          refusalOf(question, values)?.rule ?? null,
          `${String(question.id)} ${JSON.stringify(values)}`,
        ).toBe(rule);
        n += 1;
      }
    }
    expect(n).toBeGreaterThanOrEqual(60);
  });
});

describe("the answer as a whole", () => {
  const questions = block("questions") as Rec[];
  const good = (block("answers") as { context: { answers: Rec[] } }).context;

  it("accepts the page's own answer", () => {
    expect(receive(questions, good)).toEqual([]);
  });

  it("refuses answers that are not the questions, in order, once each", () => {
    const answers = good.answers;
    expect(receive(questions, { answers: answers.slice(1) })[0]?.rule).toBe("unknown");
    expect(receive(questions, { answers: [...answers].reverse() })[0]?.rule).toBe("unknown");
    expect(receive(questions, { answers: [...answers, ...answers] })[0]?.rule).toBe("unknown");
    expect(receive(questions, {})[0]?.rule).toBe("unknown");
    expect(receive(questions, null)[0]?.rule).toBe("unknown");
  });

  it("refuses a member that is not id and values, on a kind", () => {
    const answers = good.answers.map((a, i) => (i === 1 ? { ...a, text: "250.5" } : a));
    expect(receive(questions, { answers })).toEqual([
      { id: "budget", rule: "unexpected", reason: expect.any(String) },
    ]);
  });

  it("names every question that fails, in order, and says it in words", () => {
    const answers = good.answers.map((a) => {
      if (a.id === "budget") return { id: "budget", values: ["1500"] };
      if (a.id === "when") return { id: "when", values: ["2026-02-30"] };
      return a;
    });
    const refusals = receive(questions, { answers });
    expect(refusals.map((r) => [r.id, r.rule])).toEqual([
      ["budget", "above_max"],
      ["when", "grammar"],
    ]);
    expect(refusalText(refusals[0] as never)).toBe(
      'The answer to "budget" was not used (above_max): 1500 is above the most, 1000 EUR.',
    );
  });

  it("does not repair: an out-of-range number stays refused, however close", () => {
    expect(refusalOf({ type: "number", maxValue: "10" }, ["10.000001"])?.rule).toBe("above_max");
    expect(refusalOf({ type: "number", maxValue: "10" }, ["10"])).toBeUndefined();
  });
});

describe("the whole answers of the contract", () => {
  const cases = block("whole-answers") as {
    freeInput: boolean;
    questions: Rec[];
    context: unknown;
    refusal: [string, string] | null;
  }[];

  it("refuses first what the page says, by question and rule", () => {
    expect(cases.length).toBe(14);
    for (const { freeInput, questions, context, refusal } of cases) {
      const got = receive(questions, context, { freeInput })[0];
      expect(got ? [got.id, got.rule] : null, JSON.stringify(context)).toEqual(refusal);
    }
  });
});
