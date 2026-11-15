import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  blankQuestion,
  dropsHiddenMembers,
  type EditableQuestion,
  fromForm,
  hasHiddenMembers,
  idFromWords,
  type KeptQuestion,
  toForm,
  valueFromLabel,
} from "./editor";

const ROOT = path.resolve(import.meta.dirname, "../../../../..");
type Json = unknown;
type Rec = Record<string, Json>;

/** `{"$concat": [piece, ...]}` is the pieces joined (a piece: a string or `{"$repeat": text, "times": n}`), anywhere in a vector. */
function expand(v: Json): Json {
  if (Array.isArray(v)) return v.map(expand);
  if (v !== null && typeof v === "object") {
    const o = v as Rec;
    if ("$concat" in o) {
      return (o.$concat as Json[])
        .map((p) =>
          typeof p === "string"
            ? p
            : String((p as Rec).$repeat).repeat((p as { times: number }).times),
        )
        .join("");
    }
    if ("$repeat" in o) return String(o.$repeat).repeat((o as { times: number }).times);
    return Object.fromEntries(Object.entries(o).map(([k, x]) => [k, expand(x)]));
  }
  return v;
}

const vectors = JSON.parse(
  readFileSync(path.join(ROOT, "docs/api/examples/prompts/prompts.json"), "utf8"),
) as { cases: { name: string; create: Rec; expect: Json }[] };
const accepted = vectors.cases.filter((c) => c.expect === "ok");

const editable = (q: unknown): EditableQuestion => {
  const out = q as EditableQuestion | KeptQuestion;
  if (out.kept) throw new Error("not editable");
  return out;
};

describe("the editor model", () => {
  it("every ok vector of prompts.json survives toForm then fromForm deep-equal, byte for byte", () => {
    expect(accepted.length).toBe(35);
    for (const c of accepted) {
      const variables = expand(c.create.variables ?? []) as Rec[];
      const back = fromForm(toForm(variables));
      expect(back, c.name).toStrictEqual(variables);
      expect(JSON.stringify(back), c.name).toBe(JSON.stringify(variables));
    }
  });

  it("sorts the seven editable kinds from the rest", () => {
    const form = toForm([
      { id: "a", question: "A", type: "text" },
      { id: "b", question: "B", type: "longText" },
      { id: "c", question: "C", options: [{ value: "x", label: "X" }] },
      { id: "d", question: "D", type: "multiple", options: [] },
      { id: "e", question: "E", type: "confirm" },
      { id: "f", question: "F", type: "number" },
      { id: "g", question: "G", type: "date" },
      { id: "h", question: "H", type: "scale", to: 5 },
      { id: "i", question: "I", type: "rank", options: [] },
      { id: "j", question: "J", type: "select", options: [] },
      { id: "k", question: "K", type: "something-new" },
    ]);
    expect(form.map((q) => (q.kept ? "kept" : q.fields.kind))).toEqual([
      "text",
      "longText",
      "single",
      "multiple",
      "confirm",
      "number",
      "date",
      "kept",
      "kept",
      "kept",
      "kept",
    ]);
  });

  it("a kept question is never rewritten: the same object comes back whatever else changes", () => {
    const scale = { id: "depth", question: "Depth?", type: "scale", to: 5, fromLabel: "Low" };
    const form = toForm([{ id: "t", question: "Topic?", type: "text" }, scale]);
    editable(form[0]).fields.question = "Topic of the review?";
    const back = fromForm(form);
    expect(back[1]).toBe(scale);
    expect(back[0]).toStrictEqual({ id: "t", question: "Topic of the review?", type: "text" });
  });

  it("an unreadable variable (not an object) is kept as it is", () => {
    const form = toForm(["oops", 7, null]);
    expect(form.every((q) => q.kept)).toBe(true);
    expect(fromForm(form)).toStrictEqual(["oops", 7, null]);
  });

  it("an edit changes only the member edited: settings the page does not show stay", () => {
    const was = {
      id: "focus",
      question: "Focus?",
      options: [
        { value: "sec", label: "Security", description: "Threats", icon: "git", recommended: true },
        { value: "speed", label: "Speed" },
      ],
      default: "sec",
      unknownFuture: { a: 1 },
    };
    const form = toForm([was]);
    const q = editable(form[0]);
    expect(hasHiddenMembers(q)).toBe(true);
    q.fields.options[0] = {
      ...(q.fields.options[0] as (typeof q.fields.options)[number]),
      label: "Safety",
    };
    expect(fromForm(form)).toStrictEqual([
      {
        id: "focus",
        question: "Focus?",
        options: [
          { value: "sec", label: "Safety", description: "Threats", icon: "git", recommended: true },
          { value: "speed", label: "Speed" },
        ],
        default: "sec",
        unknownFuture: { a: 1 },
      },
    ]);
  });

  it("an edited label keeps its value; a new option takes a value from its label", () => {
    const form = toForm([
      {
        id: "q",
        question: "Q?",
        options: [
          { value: "a", label: "A" },
          { value: "b", label: "B" },
        ],
      },
    ]);
    const q = editable(form[0]);
    q.fields.options = [
      { value: "a", label: "Alpha", raw: { value: "a", label: "A" } },
      ...q.fields.options.slice(1),
      { value: valueFromLabel("Gamma ray", ["a", "b"]), label: "Gamma ray", raw: null },
    ];
    const out = fromForm(form)[0] as Rec;
    expect(out.options).toStrictEqual([
      { value: "a", label: "Alpha" },
      { value: "b", label: "B" },
      { value: "gamma-ray", label: "Gamma ray" },
    ]);
  });

  it("option values are unique and match the grammar", () => {
    const taken: string[] = [];
    for (const label of ["Yes!", "Yes", "yes", "Ünï", "", "   ", "a".repeat(80), "Yes?"]) {
      const value = valueFromLabel(label, taken);
      expect(value).toMatch(/^[A-Za-z0-9_.:-]{1,64}$/);
      expect(taken).not.toContain(value);
      taken.push(value);
    }
  });

  it("unchecking Required writes required false, and checking it again writes nothing new", () => {
    const form = toForm([{ id: "t", question: "T?", type: "text" }]);
    const q = editable(form[0]);
    q.fields.required = false;
    expect(fromForm(form)[0]).toStrictEqual({
      id: "t",
      question: "T?",
      type: "text",
      required: false,
    });
    q.fields.required = true;
    expect(fromForm(form)[0]).toStrictEqual({ id: "t", question: "T?", type: "text" });
  });

  it("an explicit required true that was not touched stays", () => {
    const v = [{ id: "t", question: "T?", type: "text", required: true }];
    expect(fromForm(toForm(v))).toStrictEqual(v);
  });

  it("changing the kind drops the members the new kind does not take and keeps the options of a choice", () => {
    const form = toForm([
      {
        id: "q",
        question: "Q?",
        help: "h",
        options: [
          { value: "a", label: "A" },
          { value: "b", label: "B" },
        ],
        allowOther: true,
        default: "a",
      },
    ]);
    const q = editable(form[0]);
    q.fields.kind = "multiple";
    expect(fromForm(form)[0]).toStrictEqual({
      id: "q",
      question: "Q?",
      help: "h",
      type: "multiple",
      options: [
        { value: "a", label: "A" },
        { value: "b", label: "B" },
      ],
      allowOther: true,
    });
    q.fields.kind = "number";
    q.fields.minValue = "1";
    expect(fromForm(form)[0]).toStrictEqual({
      id: "q",
      question: "Q?",
      help: "h",
      type: "number",
      minValue: "1",
    });
  });

  it("each kind writes its own members and no others", () => {
    const q = (kind: EditableQuestion["fields"]["kind"]): Rec => {
      const added = blankQuestion([]);
      added.fields.id = "x";
      added.fields.question = "X?";
      added.fields.kind = kind;
      added.fields.options = [
        { value: "a", label: "A", raw: null },
        { value: "b", label: "B", raw: null },
      ];
      added.fields.allowOther = true;
      added.fields.yesLabel = "Sure";
      added.fields.noLabel = "No way";
      added.fields.minValue = "0.5";
      added.fields.maxValue = "9";
      added.fields.step = "0.5";
      added.fields.unit = "kg";
      added.fields.minDate = "2026-01-01";
      added.fields.maxDate = "2026-12-31";
      added.fields.start = "hello";
      return fromForm([added])[0] as Rec;
    };
    const keys = (kind: EditableQuestion["fields"]["kind"]) => Object.keys(q(kind)).sort();
    expect(keys("single")).toEqual(["allowOther", "id", "options", "question", "type"]);
    expect(keys("multiple")).toEqual(["allowOther", "id", "options", "question", "type"]);
    expect(keys("confirm")).toEqual(["id", "noLabel", "question", "type", "yesLabel"]);
    expect(keys("number")).toEqual([
      "id",
      "maxValue",
      "minValue",
      "question",
      "step",
      "type",
      "unit",
    ]);
    expect(keys("date")).toEqual(["id", "maxDate", "minDate", "question", "type"]);
    expect(keys("text")).toEqual(["default", "id", "question", "type"]);
    expect(keys("longText")).toEqual(["default", "id", "question", "type"]);
  });

  it("a blank question asks for nothing it cannot say: single, required, two empty options", () => {
    const added = blankQuestion([]);
    expect(added.fields.kind).toBe("single");
    expect(added.fields.required).toBe(true);
    expect(added.fields.options.length).toBe(2);
    expect(added.raw).toBeNull();
  });
});

describe("ids from words", () => {
  it("takes the first words as a lower-case id, at most 32 characters, unique among the others", () => {
    expect(idFromWords("What should we focus on?", [])).toBe("what-should-we");
    expect(idFromWords("Depth", ["depth"])).toBe("depth-2");
    expect(idFromWords("", [], 3)).toBe("answer-3");
    expect(idFromWords("???", [], 1)).toBe("answer-1");
    expect(idFromWords("a".repeat(100), []).length).toBeLessThanOrEqual(32);
    expect(idFromWords("Café au lait", [])).toMatch(/^[A-Za-z0-9_.:-]{1,64}$/);
  });
});

describe("the older spelling of several choices, and a change of kind", () => {
  const legacy = {
    id: "q",
    question: "Which?",
    multiple: true,
    options: [
      { value: "a", label: "A" },
      { value: "b", label: "B" },
    ],
  };

  it("a question with multiple: true and no type is shown as several choices", () => {
    expect(editable(toForm([legacy])[0]).fields.kind).toBe("multiple");
  });

  it("untouched, it is sent back exactly as read", () => {
    const back = fromForm(toForm([legacy]));
    expect(back).toStrictEqual([legacy]);
    expect(JSON.stringify(back)).toBe(JSON.stringify([legacy]));
  });

  it("deliberately made one choice, it gets a type and loses multiple", () => {
    const form = toForm([legacy]);
    editable(form[0]).fields.kind = "single";
    const [out] = fromForm(form) as Record<string, unknown>[];
    expect(out?.type).toBe("single");
    expect("multiple" in (out ?? {})).toBe(false);
    expect(out?.options).toStrictEqual(legacy.options);
  });

  it("a short text with a starting answer keeps it as a long text", () => {
    const form = toForm([{ id: "a", question: "Q", type: "text", default: "x" }]);
    editable(form[0]).fields.kind = "longText";
    const [out] = fromForm(form) as Record<string, unknown>[];
    expect(out?.type).toBe("longText");
    expect(out?.default).toBe("x");
  });

  it("one choice to several keeps the options, with their own members, and the other choice", () => {
    const opts = [
      { value: "a", label: "A", description: "first" },
      { value: "b", label: "B" },
    ];
    const form = toForm([
      { id: "a", question: "Q", type: "single", options: opts, allowOther: true, default: "a" },
    ]);
    editable(form[0]).fields.kind = "multiple";
    const [out] = fromForm(form) as Record<string, unknown>[];
    expect(out?.type).toBe("multiple");
    expect(out?.options).toStrictEqual(opts);
    expect(out?.allowOther).toBe(true);
    // a member only the old kind knows is dropped
    expect("default" in (out ?? {})).toBe(false);
  });

  it("an option the person did not edit is sent back as read when a sibling changes", () => {
    const opts = [
      { value: "a", label: "A  ", description: "kept" },
      { value: "b", label: "B" },
    ];
    const form = toForm([{ id: "a", question: "Q", type: "single", options: opts }]);
    const q = editable(form[0]);
    q.fields.options = q.fields.options.map((o, k) => (k === 1 ? { ...o, label: " B2 " } : o));
    const [out] = fromForm(form) as { options: unknown[] }[];
    expect(out?.options[0]).toBe(opts[0]);
    expect(out?.options[1]).toStrictEqual({ value: "b", label: "B2" });
  });

  it("says a change of kind removes what the page does not show, only for a question that has such members", () => {
    const hidden = editable(
      toForm([{ id: "a", question: "Q", type: "number", default: 3, minValue: "1" }])[0],
    );
    expect(dropsHiddenMembers(hidden)).toBe(false);
    hidden.fields.kind = "date";
    expect(hasHiddenMembers(hidden)).toBe(false);
    expect(dropsHiddenMembers(hidden)).toBe(true);
    const plain = editable(toForm([{ id: "a", question: "Q", type: "text" }])[0]);
    plain.fields.kind = "longText";
    expect(dropsHiddenMembers(plain)).toBe(false);
  });

  it("names images, and says a type it does not know", () => {
    const [images, odd] = toForm([
      { id: "a", question: "Q", type: "images" },
      { id: "b", question: "Q", type: "hologram" },
    ]) as KeptQuestion[];
    expect(images?.phrase).toBe("an image question");
    expect(odd?.phrase).toBe("a kind of question this page does not know");
  });
});
