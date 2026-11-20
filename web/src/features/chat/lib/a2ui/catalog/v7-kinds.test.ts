import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { duplicateIn, limitsIn, readChoices } from "../choices";
import { checkValues } from "../choices-kinds";
import { prepareSurface } from "../prepare";
import lock from "./catalog.lock.json";
import { catalogDigest } from "./digest";
import { asVersion7 } from "./earlier";
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

  it("pins the vectors: deleting a case fails, whatever else is left", () => {
    const vectors = block("vectors") as { cases: [unknown[], string | null][] }[];
    const all = vectors.flatMap((v) => v.cases);
    // the page says how many there are
    const said = /the block holds \*\*(\d+) cases\*\*/.exec(doc);
    expect(said, "the page states the count").not.toBeNull();
    expect(all.length).toBe(Number(said?.[1]));
    // and how many of each rule: a deleted case drops one of these
    const per: Record<string, number> = {};
    for (const [, rule] of all) per[String(rule)] = (per[String(rule)] ?? 0) + 1;
    expect(per).toEqual({
      null: 45,
      not_offered: 10,
      too_many: 6,
      required: 6,
      off_step: 4,
      above_max: 7,
      below_min: 7,
      grammar: 44,
      not_a_permutation: 4,
    });
    expect(vectors.length).toBe(15);
  });

  it("every question the page says is invalid is refused: by the schema or by the rules", () => {
    const invalid = block("invalid-questions") as { because: string; question: Rec }[];
    expect(invalid.length).toBe(34);
    for (const { because, question } of invalid) {
      const form = choices(question);
      const spec = readChoices(form);
      const refused =
        check(form) !== undefined || spec === undefined || limitsIn(spec) !== undefined;
      expect(refused, because).toBe(true);
    }
  });

  it("an unknown type is a schema refusal whatever the thread's version; a kind is drawn at any version", () => {
    const cases = block("surfaces") as { threadVersion: number; question: Rec; result: string }[];
    expect(cases.length).toBe(5);
    for (const { threadVersion, question, result } of cases) {
      const surface = {
        version: "1.0",
        createSurface: { surfaceId: "s1", catalogId: OWN_CATALOG.catalogId },
      };
      const update = {
        version: "1.0",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Column", children: ["pick"] },
            { id: "pick", component: "Choices", questions: [question] },
          ],
        },
      };
      const prepared = prepareSurface([surface, update], { threadVersion });
      if (result === "drawn") expect(prepared.kind, JSON.stringify(question)).toBe("surface");
      else {
        expect(prepared.kind, JSON.stringify(question)).toBe("refused");
        expect(`refused:${(prepared as { rule: string }).rule}`).toBe(result);
      }
    }
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

/**
 * Version 7 only adds: take the five types, the members that belong to them and the new words of the
 * description away and what is left is the catalog of version 6, digest for digest. So an agent that
 * knows only version 6 keeps working (every Choices it can write is still valid), and a client of
 * version 6 reads the same Choices and refuses a kind it does not know.
 */
describe("version 7 is version 6 and additions", () => {
  const V6_DESCRIPTION =
    'ask_user only: use it in the question you ask the person, never in a surface you show with show (nothing waits for its answer, so its form is dead). A form of up to 8 questions answered with one submit: a title, a description of why you ask, and per question a help line and a type: single (radio buttons, the default), multiple (checkboxes), select (a searchable list, up to 50 options), images (a deck of picture tiles, up to 16, each option\'s image an image file shared in this conversation), text or longText (the person types). An option has a label, a description, an optional image {sha256, alt}, an icon and a recommended badge. freeInput adds an anything-else box. One submit sends one action: name = action.event.name (default "answer"), context = {answers:[{id, values:[option value…], other?, text?}], note?} in question order.';
  const V6_DIGEST = "sha256:f0baf255994b3c80144a5f3db634f06bce37253eb43efb4a85b33c47628c105d";
  const NEW_MEMBERS = [
    "yesLabel",
    "noLabel",
    "minValue",
    "maxValue",
    "step",
    "unit",
    "from",
    "to",
    "fromLabel",
    "toLabel",
    "minDate",
    "maxDate",
  ];
  const asVersion6 = () => {
    // version 8 only added too: take its additions away first, and what is left is version 7
    const catalog = structuredClone(asVersion7(OWN_CATALOG.catalog)) as unknown as {
      components: { Choices: Rec & { description: string; properties: Rec } };
    } & Rec;
    const choices = catalog.components.Choices;
    choices.description = V6_DESCRIPTION;
    const question = (choices.properties.questions as Rec).items as { properties: Rec };
    for (const member of NEW_MEMBERS) delete question.properties[member];
    (question.properties.type as { enum: string[] }).enum = [
      "single",
      "multiple",
      "select",
      "images",
      "text",
      "longText",
    ];
    return catalog as unknown as typeof OWN_CATALOG.catalog;
  };

  it("takes the additions away and the digest is the one version 6 shipped", async () => {
    expect(await catalogDigest(asVersion6())).toBe(V6_DIGEST);
  });

  it("a client of version 6 refuses a kind it does not know, at the place of its type", () => {
    const v6 = compileCatalog(asVersion6());
    for (const type of ["confirm", "number", "scale", "date", "rank"]) {
      const bad = choices(
        { id: "a", question: "A?", options: [] },
        { id: "q", question: "Q?", type },
      );
      expect(v6.check("Choices", bad), type).toMatch(/^questions\.1\.type: /);
    }
    // and what it knows it still takes
    const known = choices({ id: "a", question: "A?", type: "text" });
    expect(v6.check("Choices", known)).toBeUndefined();
  });
});
