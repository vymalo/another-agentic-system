import { describe, expect, it } from "vitest";
import {
  type AnswerState,
  answerContext,
  answerLines,
  buildAnswers,
  type ChoiceQuestion,
  chosenCount,
  EMPTY,
  initialState,
  isAnswered,
  isSatisfied,
  limitsIn,
  missing,
  readChoices,
} from "./choices";
import {
  checkValues,
  compareDecimals,
  isCalendarDate,
  longDate,
  parseDecimal,
} from "./choices-kinds";

/**
 * The five kinds of question of catalog version 7 (docs/api/ui-catalog-v1.md, "Version 7"): the
 * grammar of a decimal and of a date, what a receiver (and the client before it) checks, what JSON
 * Schema cannot say about the members, and how an answer is built and read back. The conformance
 * vectors of the document are run in catalog/v7-kinds.test.ts.
 */

type Rec = Record<string, unknown>;

/** The one question of a Choices, read as the web reads it. */
const question = (q: Rec): ChoiceQuestion => {
  const spec = readChoices({ id: "pick", component: "Choices", questions: [q] });
  const read = spec?.questions[0];
  if (!read) throw new Error("not a question");
  return read;
};
const spec = (...qs: Rec[]) => {
  const s = readChoices({ id: "pick", component: "Choices", questions: qs });
  if (!s) throw new Error("not a Choices");
  return s;
};
const refused = (...qs: Rec[]) => limitsIn(spec(...qs));

const NUMBER: Rec = {
  id: "n",
  question: "How much?",
  type: "number",
  unit: "EUR",
  minValue: "0",
  maxValue: "1000",
  step: "0.5",
};
const SCALE: Rec = { id: "s", question: "How likely?", type: "scale", from: 0, to: 10 };
const DATE: Rec = {
  id: "d",
  question: "Which day?",
  type: "date",
  minDate: "2026-12-01",
  maxDate: "2026-12-31",
};
const CONFIRM: Rec = {
  id: "c",
  question: "Delete it?",
  type: "confirm",
  yesLabel: "Delete it",
  noLabel: "Keep it",
};
const RANK: Rec = {
  id: "r",
  question: "Order",
  type: "rank",
  options: [
    { value: "a", label: "Alpha" },
    { value: "b", label: "Beta" },
    { value: "c", label: "Gamma" },
  ],
};

describe("a decimal is exact", () => {
  it("reads the grammar and nothing else", () => {
    for (const ok of ["0", "7", "-7", "0.5", "-0.000001", "2.50", "123456789012345.123456"]) {
      expect(parseDecimal(ok), ok).not.toBeUndefined();
    }
    for (const bad of [
      "",
      " 5",
      "5 ",
      "05",
      "-05",
      "+5",
      ".5",
      "5.",
      "1e3",
      "1E3",
      "1,5",
      "1_000",
      "0x10",
      "NaN",
      "Infinity",
      "-0",
      "-0.0",
      "-0.000000",
      "1234567890123456",
      "0.1234567",
      "٣",
    ]) {
      expect(parseDecimal(bad), JSON.stringify(bad)).toBeUndefined();
    }
  });

  it("compares as decimals, never through floating point", () => {
    const cmp = (a: string, b: string) => {
      const x = parseDecimal(a);
      const y = parseDecimal(b);
      if (x === undefined || y === undefined) throw new Error("bad");
      return compareDecimals(x, y);
    };
    expect(cmp("1000.5", "1000")).toBe(1);
    expect(cmp("2.50", "2.5")).toBe(0);
    expect(cmp("-0.000001", "0")).toBe(-1);
    expect(cmp("0.1", "0.3")).toBe(-1);
    expect(cmp("999999999999999.999999", "999999999999999.999998")).toBe(1);
  });
});

describe("a date is a day that exists", () => {
  it("accepts the days of the proleptic Gregorian calendar, years 0001 to 9999", () => {
    for (const ok of ["2026-12-15", "2028-02-29", "2000-02-29", "0001-01-01", "9999-12-31"]) {
      expect(isCalendarDate(ok), ok).toBe(true);
    }
  });

  it("refuses the shapes and the days that do not exist", () => {
    for (const bad of [
      "2027-02-29",
      "1900-02-29",
      "2026-04-31",
      "2026-13-01",
      "2026-00-10",
      "2026-12-00",
      "0000-01-01",
      "2026-12-1",
      "26-12-15",
      "2026/12/15",
      "12/15/2026",
      "2026-12-15T00:00:00Z",
      " 2026-12-15",
      "2026-12-15\n",
      "２０２６-12-15",
      "",
    ]) {
      expect(isCalendarDate(bad), JSON.stringify(bad)).toBe(false);
    }
  });
});

describe("what a question of a kind must hold (what JSON Schema cannot say)", () => {
  it("accepts the five, with and without their optional members", () => {
    expect(refused(NUMBER, SCALE, DATE, CONFIRM, RANK)).toBeUndefined();
    expect(
      refused(
        { id: "a", question: "A?", type: "confirm" },
        { id: "b", question: "B?", type: "number" },
        { id: "c", question: "C?", type: "scale", to: 5 },
        { id: "d", question: "D?", type: "date" },
      ),
    ).toBeUndefined();
  });

  it("a kind takes no options, except a rank (2 to 12, no pictures)", () => {
    const opts = [
      { value: "a", label: "A" },
      { value: "b", label: "B" },
    ];
    expect(refused({ ...CONFIRM, options: opts })).toMatch(/takes no options/);
    expect(refused({ ...NUMBER, options: opts })).toMatch(/takes no options/);
    expect(refused({ id: "r", question: "R?", type: "rank", options: opts.slice(0, 1) })).toMatch(
      /2 to 12 options/,
    );
    const thirteen = Array.from({ length: 13 }, (_, i) => ({ value: `v${i}`, label: `V${i}` }));
    expect(refused({ id: "r", question: "R?", type: "rank", options: thirteen })).toMatch(
      /2 to 12 options/,
    );
    expect(refused({ id: "r", question: "R?", type: "rank" })).toMatch(/2 to 12 options/);
    const sha = "a1".repeat(32);
    const pictured = opts.map((o) => ({ ...o, image: { sha256: sha, alt: "x" } }));
    expect(refused({ id: "r", question: "R?", type: "rank", options: pictured })).toMatch(/image/);
  });

  it("none takes multiple, allowOther, min or max", () => {
    for (const q of [CONFIRM, NUMBER, SCALE, DATE, RANK]) {
      expect(refused({ ...q, multiple: true }), `${q.type} multiple`).toMatch(/multiple/);
      expect(refused({ ...q, allowOther: true }), `${q.type} allowOther`).toMatch(/allowOther/);
      expect(refused({ ...q, min: 1 }), `${q.type} min`).toMatch(/min and max/);
      expect(refused({ ...q, max: 2 }), `${q.type} max`).toMatch(/min and max/);
    }
  });

  it("a member belongs to its kind", () => {
    expect(refused({ ...NUMBER, yesLabel: "Go" })).toMatch(/yesLabel/);
    expect(refused({ ...CONFIRM, minValue: "1" })).toMatch(/minValue/);
    expect(refused({ ...DATE, step: "1" })).toMatch(/step/);
    expect(refused({ ...SCALE, unit: "pt" })).toMatch(/unit/);
    expect(refused({ ...NUMBER, from: 1 })).toMatch(/from/);
    expect(refused({ ...RANK, minDate: "2026-01-01" })).toMatch(/minDate/);
    expect(refused({ id: "t", question: "T?", type: "text", toLabel: "x" })).toMatch(/toLabel/);
  });

  it("number: min at most max, step above zero, a default that is an answer", () => {
    expect(refused({ ...NUMBER, minValue: "10", maxValue: "5" })).toMatch(/above its maxValue/);
    expect(refused({ ...NUMBER, step: "0" })).toMatch(/step/);
    expect(refused({ ...NUMBER, step: "-1" })).toMatch(/step/);
    expect(refused({ ...NUMBER, default: "5000" })).toMatch(/default/);
    expect(refused({ ...NUMBER, default: "250.25" })).toMatch(/default/);
    expect(refused({ ...NUMBER, default: "250" })).toBeUndefined();
    expect(refused({ ...NUMBER, minValue: "-0" })).toMatch(/minValue/);
  });

  it("scale: `to` is required and above `from`, a default is a point of the scale", () => {
    expect(refused({ id: "s", question: "S?", type: "scale" })).toMatch(/needs a "to"/);
    expect(refused({ ...SCALE, from: 1, to: 1 })).toMatch(/above its "from"/);
    expect(refused({ ...SCALE, from: 1, to: 2 })).toBeUndefined();
    expect(refused({ ...SCALE, default: "11" })).toMatch(/default/);
    expect(refused({ ...SCALE, default: "4" })).toBeUndefined();
    expect(refused({ ...SCALE, from: 1, default: "0" })).toMatch(/default/);
  });

  it("date: real days, min at most max, a default in the bounds", () => {
    expect(refused({ ...DATE, minDate: "2026-02-30" })).toMatch(/minDate/);
    expect(refused({ ...DATE, minDate: "2027-01-01" })).toMatch(/above its maxDate/);
    expect(refused({ ...DATE, default: "2027-01-01" })).toMatch(/default/);
    expect(refused({ ...DATE, default: "2026-12-15" })).toBeUndefined();
  });

  it("confirm: the default is yes or no; rank: the default is every option once", () => {
    expect(refused({ ...CONFIRM, default: "maybe" })).toMatch(/default/);
    expect(refused({ ...CONFIRM, default: "no" })).toBeUndefined();
    expect(refused({ ...RANK, default: ["a", "b"] })).toMatch(/default/);
    expect(refused({ ...RANK, default: ["a", "a", "b"] })).toMatch(/default/);
    expect(refused({ ...RANK, default: ["c", "b", "a"] })).toBeUndefined();
  });
});

describe("what a receiver checks (the first rule that fails)", () => {
  const rule = (q: Rec, values: string[]) => checkValues(question(q), values)?.rule ?? null;

  it("names the rule, in a message a person can read", () => {
    const problem = checkValues(question(NUMBER), ["1500"]);
    expect(problem?.rule).toBe("above_max");
    expect(problem?.message).toMatch(/1000/);
    expect(problem?.message).toMatch(/EUR/);
    expect(checkValues(question(NUMBER), ["1e3"])?.message).toMatch(/digits and a point/);
    expect(checkValues(question(NUMBER), ["250.25"])?.message).toMatch(/0\.5/);
    expect(checkValues(question(DATE), ["2026-11-30"])?.message).toMatch(/2026-12-01/);
    expect(checkValues(question(SCALE), ["11"])?.message).toMatch(/10/);
  });

  it("an optional question may be left empty, a required one may not", () => {
    expect(rule(NUMBER, [])).toBe("required");
    expect(rule({ ...NUMBER, required: false }, [])).toBeNull();
    expect(rule({ ...RANK, required: false }, [])).toBeNull();
    expect(rule(CONFIRM, [])).toBe("required");
  });

  it("one value for every kind but a rank", () => {
    expect(rule(CONFIRM, ["yes", "no"])).toBe("too_many");
    expect(rule(SCALE, ["1", "2"])).toBe("too_many");
    expect(rule(RANK, ["a", "b", "c", "a"])).toBe("not_a_permutation");
  });

  it("checks the bound before the step, and the grammar before the bound", () => {
    expect(rule(NUMBER, ["1000.25"])).toBe("above_max");
    expect(rule(NUMBER, ["-0.25"])).toBe("below_min");
    expect(rule(NUMBER, ["abc"])).toBe("grammar");
  });

  it("the step starts at minValue, or at zero", () => {
    expect(rule({ ...NUMBER, minValue: "1", maxValue: "10", step: "2" }, ["3"])).toBeNull();
    expect(rule({ ...NUMBER, minValue: "1", maxValue: "10", step: "2" }, ["4"])).toBe("off_step");
    expect(rule({ id: "n", question: "N?", type: "number", step: "0.25" }, ["-0.75"])).toBeNull();
    expect(rule({ id: "n", question: "N?", type: "number", step: "0.25" }, ["0.1"])).toBe(
      "off_step",
    );
    expect(rule({ id: "n", question: "N?", type: "number", step: "0.1" }, ["0.3"])).toBeNull();
  });
});

describe("answering", () => {
  const form = spec(CONFIRM, NUMBER, SCALE, DATE, RANK);
  const state = (patch: Record<string, Partial<{ values: string[]; text: string }>>) =>
    new Map(Object.entries(patch).map(([id, p]) => [id, { ...EMPTY, ...p }])) as AnswerState;

  it("starts from the defaults of each kind", () => {
    const s = initialState(
      spec(
        { ...CONFIRM, default: "no" },
        { ...NUMBER, default: "250" },
        { ...SCALE, default: "4" },
        { ...DATE, default: "2026-12-15" },
        { ...RANK, default: ["c", "a", "b"] },
      ),
    );
    expect(s.get("c")?.values).toEqual(["no"]);
    expect(s.get("n")?.text).toBe("250");
    expect(s.get("s")?.values).toEqual(["4"]);
    expect(s.get("d")?.text).toBe("2026-12-15");
    // a rank's default is the order it starts in and is no answer until the person acts
    expect(s.get("r")).toBeUndefined();
    // nothing without a default: a rank untouched is not the person's ranking
    expect(initialState(form).size).toBe(0);
  });

  it("builds `values` as strings, in the person's order for a rank, with no other and no text", () => {
    const answers = buildAnswers(
      form,
      state({
        c: { values: ["yes"] },
        n: { text: " 250.5 " },
        s: { values: ["7"] },
        d: { text: "2026-12-15" },
        r: { values: ["c", "a", "b"] },
      }),
    );
    expect(answers).toEqual([
      { id: "c", values: ["yes"] },
      { id: "n", values: ["250.5"] },
      { id: "s", values: ["7"] },
      { id: "d", values: ["2026-12-15"] },
      { id: "r", values: ["c", "a", "b"] },
    ]);
    expect(JSON.stringify(answerContext(form, state({ n: { text: "12.50" } })))).toContain(
      '"values":["12.50"]',
    );
  });

  it("an empty kind is an empty `values`", () => {
    expect(buildAnswers(spec({ ...NUMBER, required: false }), new Map())).toEqual([
      { id: "n", values: [] },
    ]);
  });

  it("holds the form back while an answer is invalid, and says how many are", () => {
    const ok = state({
      c: { values: ["no"] },
      n: { text: "10" },
      s: { values: ["3"] },
      d: { text: "2026-12-02" },
      r: { values: ["a", "b", "c"] },
    });
    expect(missing(form, ok)).toBe(0);
    expect(missing(form, new Map())).toBe(5);
    expect(missing(form, new Map(ok).set("n", { ...EMPTY, text: "5000" }))).toBe(1);
    expect(missing(form, new Map(ok).set("d", { ...EMPTY, text: "2026-02-30" }))).toBe(1);
    expect(missing(form, new Map(ok).set("r", { ...EMPTY, values: ["a", "b"] }))).toBe(1);
  });

  it("counts an answer: one for a kind with a value, none without", () => {
    const n = question(NUMBER);
    expect(chosenCount(n, { ...EMPTY, text: "5" })).toBe(1);
    expect(chosenCount(n, EMPTY)).toBe(0);
    expect(isAnswered(n, { ...EMPTY, text: " " })).toBe(false);
    expect(isSatisfied({ ...n, required: false }, EMPTY)).toBe(true);
    expect(isSatisfied(n, { ...EMPTY, text: "5000" })).toBe(false);
  });

  it("reads an answer back with the words the person saw", () => {
    const lines = answerLines(
      [
        { id: "c", values: ["yes"] },
        { id: "n", values: ["250.5"] },
        { id: "s", values: ["10"] },
        { id: "d", values: ["2026-12-15"] },
        { id: "r", values: ["c", "a", "b"] },
        { id: "z", values: [] },
      ],
      form.questions,
    );
    expect(lines[0]).toMatchObject({ question: "Delete it?", chosen: ["Delete it"] });
    expect(lines[1]?.chosen).toEqual(["250.5 EUR"]);
    expect(lines[2]?.chosen).toEqual(["10"]);
    expect(lines[3]?.chosen).toEqual([longDate("2026-12-15")]);
    expect(lines[4]).toMatchObject({ ordered: true, chosen: ["Gamma", "Alpha", "Beta"] });
    expect(lines[5]).toMatchObject({ question: "z", chosen: [] });
  });

  it("reads the labels of the ends of a scale, and a yes or no with no labels", () => {
    const q = spec(
      { ...SCALE, fromLabel: "Not at all", toLabel: "Certainly" },
      { id: "c", question: "OK?", type: "confirm" },
    ).questions;
    expect(answerLines([{ id: "s", values: ["10"] }], q)[0]?.chosen).toEqual(["10 (Certainly)"]);
    expect(answerLines([{ id: "s", values: ["0"] }], q)[0]?.chosen).toEqual(["0 (Not at all)"]);
    expect(answerLines([{ id: "s", values: ["4"] }], q)[0]?.chosen).toEqual(["4"]);
    expect(answerLines([{ id: "c", values: ["no"] }], q)[0]?.chosen).toEqual(["No"]);
  });

  it("writes a day in the viewer's locale and never in another zone's day", () => {
    expect(longDate("2026-12-15", "en-GB")).toBe("15 December 2026");
    expect(longDate("2026-12-15", "en-US")).toBe("December 15, 2026");
    expect(longDate("2026-12-15", "de")).toBe("15. Dezember 2026");
    expect(longDate("0099-01-02", "en-GB")).toBe("2 January 99");
    expect(longDate("2026-02-30", "en-GB")).toBe("2026-02-30"); // not a day: shown as it was sent
  });

  for (const zone of ["Pacific/Kiritimati", "America/Los_Angeles", "UTC"]) {
    it(`shows a day as the same day in the zone ${zone}`, () => {
      const before = process.env.TZ;
      process.env.TZ = zone;
      try {
        const offset = new Date(2026, 0, 1).getTimezoneOffset();
        if (zone !== "UTC") expect(offset, "the zone took").not.toBe(0);
        expect(longDate("2026-01-01", "en-GB")).toBe("1 January 2026");
        expect(longDate("2026-12-31", "en-GB")).toBe("31 December 2026");
        expect(longDate("2026-03-29", "en-GB")).toBe("29 March 2026");
      } finally {
        if (before === undefined) delete process.env.TZ;
        else process.env.TZ = before;
      }
    });
  }

  it("shows a value the surface does not hold as it was sent", () => {
    expect(answerLines([{ id: "n", values: ["5"] }], undefined)[0]?.chosen).toEqual(["5"]);
  });
});

describe("what is refused that the older tests did not say", () => {
  it("a value that is not a string is refused as the kind's own rule refuses a wrong one", () => {
    const rule = (q: Rec, values: unknown[]) => checkValues(question(q), values)?.rule ?? null;
    expect(rule(CONFIRM, [true])).toBe("not_offered");
    expect(rule(NUMBER, [250.5])).toBe("grammar");
    expect(rule(SCALE, [7])).toBe("grammar");
    expect(rule(DATE, [20261215])).toBe("grammar");
    expect(rule(RANK, [1, 2, 3])).toBe("not_offered");
    expect(rule(NUMBER, [250.5, 300])).toBe("too_many");
  });

  it("a default has the shape of its kind: a string, and for a rank a list", () => {
    expect(refused({ ...NUMBER, default: ["5"] })).toMatch(/must be a string/);
    expect(refused({ ...CONFIRM, default: ["yes"] })).toMatch(/must be a string/);
    expect(refused({ ...SCALE, default: ["3"] })).toMatch(/must be a string/);
    expect(refused({ ...DATE, default: ["2026-12-15"] })).toMatch(/must be a string/);
    expect(refused({ ...RANK, default: "a" })).toMatch(/must be a list/);
  });

  it("a member of a kind is refused on the older types too", () => {
    const single = {
      id: "s",
      question: "S?",
      options: [
        { value: "a", label: "A" },
        { value: "b", label: "B" },
      ],
    };
    expect(refused({ ...single, step: "1" })).toMatch(/step/);
    expect(refused({ ...single, yesLabel: "Go" })).toMatch(/yesLabel/);
    expect(refused({ id: "t", question: "T?", type: "text", unit: "kg" })).toMatch(/unit/);
  });
});
