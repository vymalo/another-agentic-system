import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { duplicateIn, limitsIn, readChoices } from "../choices";
import { checkValues } from "../choices-kinds";
import lock from "./catalog.lock.json";
import { componentNames, OWN_CATALOG } from "./index";
import { compileCatalog } from "./validate";

/**
 * Catalog version 7: five more kinds of question in `Choices` (confirm, number, scale, date, rank).
 * The schema on its own, then the contract page itself: its examples are parsed and its
 * conformance vectors are run through the client's own check, so that the page cannot say what
 * the code does not.
 */

type Rec = Record<string, unknown>;
const check = (instance: unknown) => compileCatalog(OWN_CATALOG.catalog).check("Choices", instance);
const choices = (...questions: Rec[]) => ({ id: "ask", component: "Choices", questions });
const ask = (q: Rec) => check(choices({ id: "q", question: "Q?", ...q }));

describe("version 7", () => {
  it("is catalog version 7 or later and still has Choices", () => {
    expect(OWN_CATALOG.version).toBeGreaterThanOrEqual(7);
    expect(lock.version).toBeGreaterThanOrEqual(7);
    expect(componentNames(OWN_CATALOG.catalog)).toContain("Choices");
  });

  it("every type of version 6 is still a type, and the five join them", () => {
    const schema = OWN_CATALOG.catalog.components.Choices as {
      properties: { questions: { items: { properties: { type: { enum: string[] } } } } };
    };
    expect(schema.properties.questions.items.properties.type.enum).toEqual([
      "single",
      "multiple",
      "select",
      "images",
      "text",
      "longText",
      "confirm",
      "number",
      "scale",
      "date",
      "rank",
    ]);
  });

  it("tells an agent the kinds, their members and that a receiver checks again", () => {
    const d = (OWN_CATALOG.catalog.components.Choices as { description: string }).description;
    for (const word of ["confirm", "number", "scale", "date", "rank", "version 7"]) {
      expect(d, word).toContain(word);
    }
    expect(d).toMatch(/refused/);
  });
});

describe("the schema of the five kinds", () => {
  it("accepts one of each", () => {
    expect(ask({ type: "confirm", yesLabel: "Delete it", noLabel: "Keep it", default: "no" })).toBe(
      undefined,
    );
    expect(
      ask({ type: "number", unit: "EUR", minValue: "0", maxValue: "1000", step: "0.5" }),
    ).toBeUndefined();
    expect(
      ask({ type: "scale", from: 0, to: 10, fromLabel: "Not at all", toLabel: "Certainly" }),
    ).toBeUndefined();
    expect(ask({ type: "date", minDate: "2026-12-01", maxDate: "2026-12-31" })).toBeUndefined();
    expect(
      ask({
        type: "rank",
        options: [
          { value: "a", label: "A" },
          { value: "b", label: "B" },
        ],
        default: ["b", "a"],
      }),
    ).toBeUndefined();
  });

  it("a decimal is a string of the grammar, never a JSON number", () => {
    for (const member of ["minValue", "maxValue", "step", "default"]) {
      for (const ok of ["0", "-1", "12.5", "2.50", "123456789012345.123456"]) {
        expect(ask({ type: "number", [member]: ok }), `${member} ${ok}`).toBeUndefined();
      }
    }
    for (const member of ["minValue", "maxValue", "step"]) {
      for (const bad of [
        "1e3",
        "+1",
        ".5",
        "1.",
        "01",
        "1,5",
        " 1",
        "1234567890123456",
        "0.1234567",
      ]) {
        expect(ask({ type: "number", [member]: bad }), `${member} ${bad}`).toMatch(
          new RegExp(`^questions\\.0\\.${member}: `),
        );
      }
      expect(ask({ type: "number", [member]: 5 }), `${member} as a number`).toMatch(
        new RegExp(`^questions\\.0\\.${member}: `),
      );
    }
  });

  it("the bounds of every member are exact", () => {
    const ok = (q: Rec) => ask(q) === undefined;
    expect(ok({ type: "confirm", yesLabel: "y".repeat(40) })).toBe(true);
    expect(ask({ type: "confirm", yesLabel: "y".repeat(41) })).toMatch(/^questions\.0\.yesLabel: /);
    expect(ask({ type: "confirm", noLabel: "" })).toMatch(/^questions\.0\.noLabel: /);
    expect(ok({ type: "number", unit: "u".repeat(20) })).toBe(true);
    expect(ask({ type: "number", unit: "u".repeat(21) })).toMatch(/unit/);
    expect(ok({ type: "scale", from: 0, to: 2 })).toBe(true);
    expect(ok({ type: "scale", from: 1, to: 10 })).toBe(true);
    expect(ask({ type: "scale", from: 2, to: 5 })).toMatch(/from/);
    expect(ask({ type: "scale", from: -1, to: 5 })).toMatch(/from/);
    expect(ask({ type: "scale", to: 11 })).toMatch(/to/);
    expect(ask({ type: "scale", to: 1 })).toMatch(/to/);
    expect(ask({ type: "scale", to: 2.5 })).toMatch(/to/);
    expect(ask({ type: "scale", to: 5, fromLabel: "f".repeat(61) })).toMatch(/fromLabel/);
    expect(ask({ type: "scale", to: 5, toLabel: "" })).toMatch(/toLabel/);
    expect(ok({ type: "date", minDate: "0001-01-01", maxDate: "9999-12-31" })).toBe(true);
    for (const bad of ["2026-1-1", "26-01-01", "2026/01/01", "2026-01-01T00:00", "20260101"]) {
      expect(ask({ type: "date", minDate: bad }), bad).toMatch(/minDate/);
    }
  });

  it("refuses a type that is not one of the eleven, and a member nobody has", () => {
    expect(ask({ type: "slider" })).toMatch(/type/);
    expect(ask({ type: "number", precision: 2 })).toMatch(/precision/);
    expect(ask({ type: "number", minValue: { path: "/min" } })).toMatch(/minValue/);
  });
});

/** The fenced blocks of the contract page that carry a second word after `json`. */
const doc = readFileSync(
  path.resolve(import.meta.dirname, "../../../../../../../docs/api/ui-catalog-v1.md"),
  "utf8",
);
const block = (kind: string): unknown => {
  const found = [...doc.matchAll(new RegExp(`\`\`\`json ${kind}\\n([\\s\\S]*?)\`\`\``, "g"))];
  expect(found.length, `one \`\`\`json ${kind} block in the contract`).toBe(1);
  return JSON.parse(found[0]?.[1] ?? "");
};
const asQuestion = (q: Rec) => {
  const spec = readChoices({ id: "ask", component: "Choices", questions: [q] });
  const read = spec?.questions[0];
  if (!read) throw new Error(`not a question: ${JSON.stringify(q)}`);
  return read;
};

describe("the contract page (docs/api/ui-catalog-v1.md, Version 7)", () => {
  it("its questions are one valid form of the five kinds", () => {
    const questions = block("questions") as Rec[];
    expect(questions.map((q) => q.type)).toEqual(["confirm", "number", "scale", "date", "rank"]);
    const form = choices(...questions);
    expect(check(form)).toBeUndefined();
    const spec = readChoices(form);
    expect(spec && duplicateIn(spec)).toBeUndefined();
    expect(spec && limitsIn(spec)).toBeUndefined();
  });

  it("its answer is an answer to those questions, and a receiver accepts every entry", () => {
    const questions = block("questions") as Rec[];
    const { context } = block("answers") as {
      context: { answers: { id: string; values: string[] }[] };
    };
    expect(context.answers.map((a) => a.id)).toEqual(questions.map((q) => q.id));
    for (const [i, a] of context.answers.entries()) {
      const q = questions[i] as Rec;
      expect(checkValues(asQuestion(q), a.values), `${a.id}`).toBeUndefined();
      expect(Object.keys(a).sort()).toEqual(["id", "values"]);
    }
  });

  it("each of its vectors is judged as the page says", () => {
    const vectors = block("vectors") as { question: Rec; cases: [string[], string | null][] }[];
    expect(vectors.length).toBeGreaterThanOrEqual(9);
    let cases = 0;
    for (const { question, cases: list } of vectors) {
      // the question of a vector is itself one the catalog and the rules accept
      const form = choices(question);
      expect(check(form), `the question ${String(question.id)}`).toBeUndefined();
      const spec = readChoices(form);
      expect(spec && limitsIn(spec), `the question ${String(question.id)}`).toBeUndefined();
      for (const [values, expected] of list) {
        const got = checkValues(asQuestion(question), values)?.rule ?? null;
        expect(got, `${String(question.id)} ${JSON.stringify(values)}`).toBe(expected);
        cases += 1;
      }
    }
    expect(cases).toBeGreaterThanOrEqual(60);
  });

  it("every rule the page names has a case, and every case names a rule of the page", () => {
    const vectors = block("vectors") as { cases: [string[], string | null][] }[];
    const used = new Set(vectors.flatMap((v) => v.cases.map(([, rule]) => rule)).filter(Boolean));
    const table = [...doc.matchAll(/^\| `(\w+)` \| /gm)].map((m) => m[1] as string);
    const rules = [
      "required",
      "too_many",
      "not_offered",
      "grammar",
      "below_min",
      "above_max",
      "off_step",
      "not_a_permutation",
    ];
    for (const rule of rules) {
      expect(table, rule).toContain(rule);
      expect(used.has(rule), `a vector for ${rule}`).toBe(true);
    }
    for (const rule of used) expect(rules, String(rule)).toContain(rule);
    // `unknown` and `unexpected` are about the whole answer and not a question's values
    expect(table).toContain("unknown");
    expect(table).toContain("unexpected");
  });

  it("holds the schema of Choices once, and it is the build's", () => {
    const blocks = [...doc.matchAll(/```json\n([\s\S]*?)```/g)].flatMap(([, body]) => {
      try {
        return [JSON.parse(body ?? "") as Rec];
      } catch {
        return [];
      }
    });
    const written = blocks.filter((b) => !Array.isArray(b) && "Choices" in b);
    expect(written.length).toBe(1);
    expect(written[0]?.Choices).toEqual(OWN_CATALOG.catalog.components.Choices);
  });
});
