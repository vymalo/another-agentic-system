import { describe, expect, it } from "vitest";
import {
  ANSWER_ACTION,
  type AnswerState,
  answerContext,
  answerLines,
  buildAnswers,
  type ChoicesSpec,
  choicesOf,
  chosenCount,
  duplicateIn,
  EMPTY,
  FREE_INPUT_LABEL,
  initialState,
  isAnswered,
  isSatisfied,
  limitsIn,
  MAX_LONG_TEXT,
  MAX_NOTE,
  MAX_OTHER,
  MAX_TEXT,
  missing,
  type QuestionState,
  questionsOf,
  readAnswers,
  readChoices,
  readNote,
} from "./choices";

type Rec = Record<string, unknown>;

const component = (over: Rec = {}): Rec => ({
  id: "pick",
  component: "Choices",
  questions: [
    {
      id: "db",
      question: "Which database?",
      allowOther: true,
      options: [
        { value: "pg", label: "Postgres", description: "the default" },
        { value: "sqlite", label: "SQLite" },
      ],
    },
    {
      id: "auth",
      question: "Which login?",
      options: [
        { value: "keycloak", label: "Keycloak" },
        { value: "none", label: "No login" },
      ],
    },
    {
      id: "deploy",
      question: "Where?",
      multiple: true,
      required: false,
      allowOther: true,
      options: [
        { value: "k8s", label: "Kubernetes" },
        { value: "compose", label: "Compose" },
      ],
    },
  ],
  ...over,
});

const spec = (over: Rec = {}): ChoicesSpec => {
  const read = readChoices(component(over));
  if (!read) throw new Error("not a Choices");
  return read;
};

const st = (over: Partial<QuestionState> = {}): QuestionState => ({ ...EMPTY, ...over });
const state = (entries: Record<string, QuestionState>): AnswerState =>
  new Map(Object.entries(entries));

describe("reading a Choices", () => {
  it("reads the questions, with the defaults: required, single, no Other", () => {
    const s = spec();
    expect(s.componentId).toBe("pick");
    expect(s.actionName).toBe(ANSWER_ACTION);
    expect(s.questions.map((q) => [q.id, q.multiple, q.allowOther, q.required])).toEqual([
      ["db", false, true, true],
      ["auth", false, false, true],
      ["deploy", true, true, false],
    ]);
    expect(s.questions[0]?.options[0]).toEqual({
      value: "pg",
      label: "Postgres",
      description: "the default",
    });
    expect(s.questions[0]?.options[1]).toEqual({ value: "sqlite", label: "SQLite" });
  });

  it("the component's id comes from componentId (what the validator adds) or from its own id", () => {
    expect(
      readChoices({ ...component(), id: undefined, componentId: "lowered" })?.componentId,
    ).toBe("lowered");
    expect(spec().componentId).toBe("pick");
  });

  it("takes the title, the submit label and the action's name", () => {
    const s = spec({
      title: "Quick choices",
      submitLabel: "Go",
      action: { event: { name: "chosen" } },
    });
    expect([s.title, s.submitLabel, s.actionName]).toEqual(["Quick choices", "Go", "chosen"]);
  });

  it("is nothing for what is not a Choices", () => {
    for (const bad of [
      {},
      { questions: [] },
      { questions: "x" },
      { questions: [{ id: "a" }] },
      { questions: [{ id: "a", question: "q", options: [{ value: "x" }] }] },
      { questions: [{ id: "a", question: "q", options: "x" }] },
      { questions: [null] },
    ]) {
      expect(readChoices(bad as Rec), JSON.stringify(bad)).toBeUndefined();
    }
  });

  it("finds a question id used twice, and an option value used twice in a question", () => {
    expect(duplicateIn(spec())).toBeUndefined();
    const twice = spec({
      questions: [
        {
          id: "a",
          question: "A",
          options: [
            { value: "x", label: "X" },
            { value: "y", label: "Y" },
          ],
        },
        {
          id: "a",
          question: "B",
          options: [
            { value: "x", label: "X" },
            { value: "y", label: "Y" },
          ],
        },
      ],
    });
    expect(duplicateIn(twice)).toBe('the question id "a" is used twice');
    const values = spec({
      questions: [
        {
          id: "a",
          question: "A",
          options: [
            { value: "x", label: "X" },
            { value: "x", label: "Y" },
          ],
        },
      ],
    });
    expect(duplicateIn(values)).toBe('the option value "x" is used twice in the question "a"');
    // the same value in two different questions is fine
    const apart = spec({
      questions: [
        {
          id: "a",
          question: "A",
          options: [
            { value: "x", label: "X" },
            { value: "y", label: "Y" },
          ],
        },
        {
          id: "b",
          question: "B",
          options: [
            { value: "x", label: "X" },
            { value: "y", label: "Y" },
          ],
        },
      ],
    });
    expect(duplicateIn(apart)).toBeUndefined();
  });
});

describe("answering", () => {
  const s = spec();
  const [db, auth, deploy] = s.questions;
  if (!db || !auth || !deploy) throw new Error("fixture");

  it("a question is answered by an option, or by 'Other' once it says something", () => {
    expect(isAnswered(db)).toBe(false);
    expect(isAnswered(db, st({ values: ["pg"] }))).toBe(true);
    expect(isAnswered(db, st({ otherOn: true, other: "" }))).toBe(false);
    expect(isAnswered(db, st({ otherOn: true, other: "   " }))).toBe(false);
    expect(isAnswered(db, st({ otherOn: true, other: "Cockroach" }))).toBe(true);
    // text typed and then switched off does not count
    expect(isAnswered(db, st({ otherOn: false, other: "Cockroach" }))).toBe(false);
    // a question without allowOther has no Other
    expect(isAnswered(auth, st({ otherOn: true, other: "LDAP" }))).toBe(false);
  });

  it("counts the required questions that are not answered; an optional one never blocks", () => {
    expect(missing(s, state({}))).toBe(2);
    expect(missing(s, state({ db: st({ values: ["pg"] }) }))).toBe(1);
    expect(missing(s, state({ db: st({ values: ["pg"] }), auth: st({ values: ["none"] }) }))).toBe(
      0,
    );
  });

  it("the answers are in question order, one per question, values in the options' order, `other` trimmed", () => {
    const answers = buildAnswers(
      s,
      state({
        auth: st({ values: ["none"] }),
        db: st({ otherOn: true, other: "  Cockroach  " }),
        deploy: st({ values: ["compose", "k8s"] }),
      }),
    );
    expect(answers).toEqual([
      { id: "db", values: [], other: "Cockroach" },
      { id: "auth", values: ["none"] },
      { id: "deploy", values: ["k8s", "compose"] },
    ]);
  });

  it("a skipped optional question is sent as empty, and `other` only when it is on and not empty", () => {
    expect(buildAnswers(s, state({ db: st({ values: ["pg"], other: "left over" }) }))[0]).toEqual({
      id: "db",
      values: ["pg"],
    });
    expect(buildAnswers(s, state({}))[2]).toEqual({ id: "deploy", values: [] });
  });

  it("a value the question does not have is not sent", () => {
    expect(buildAnswers(s, state({ db: st({ values: ["mongo", "pg"] }) }))[0]).toEqual({
      id: "db",
      values: ["pg"],
    });
  });

  it("`other` is cut at 500 characters", () => {
    const long = "x".repeat(MAX_OTHER + 40);
    const [first] = buildAnswers(s, state({ db: st({ otherOn: true, other: long }) }));
    expect(first?.other).toHaveLength(MAX_OTHER);
  });

  it("a question id that is __proto__ or constructor is a key like another", () => {
    const hostile = spec({
      questions: [
        {
          id: "__proto__",
          question: "P",
          options: [
            { value: "a", label: "A" },
            { value: "b", label: "B" },
          ],
        },
        {
          id: "constructor",
          question: "C",
          options: [
            { value: "a", label: "A" },
            { value: "b", label: "B" },
          ],
        },
      ],
    });
    const picked = new Map<string, QuestionState>([["__proto__", st({ values: ["a"] })]]);
    expect(missing(hostile, picked)).toBe(1);
    expect(answerContext(hostile, picked).answers).toEqual([
      { id: "__proto__", values: ["a"] },
      { id: "constructor", values: [] },
    ]);
    expect(({} as Rec).values).toBeUndefined();
  });

  it("the context is {answers}", () => {
    expect(answerContext(s, state({ db: st({ values: ["pg"] }) }))).toEqual({
      answers: [
        { id: "db", values: ["pg"] },
        { id: "auth", values: [] },
        { id: "deploy", values: [] },
      ],
    });
  });

  it("the worst case, 8 questions of 8 values of 64 characters and 500 of 'other', is under 16 KiB", () => {
    const options = Array.from({ length: 8 }, (_, i) => ({
      value: `${String(i)}`.padEnd(64, "v"),
      label: "L",
    }));
    const big = spec({
      questions: Array.from({ length: 8 }, (_, i) => ({
        id: `q${i}`.padEnd(64, "i"),
        question: "Q",
        multiple: true,
        allowOther: true,
        options,
      })),
    });
    const all = new Map<string, QuestionState>(
      big.questions.map((q) => [
        q.id,
        st({ values: q.options.map((o) => o.value), otherOn: true, other: "o".repeat(MAX_OTHER) }),
      ]),
    );
    const bytes = new TextEncoder().encode(JSON.stringify(answerContext(big, all))).length;
    expect(bytes).toBeLessThan(16 * 1024);
  });
});

describe("reading an answer back", () => {
  it("is the context's `answers` when they have the shape of one", () => {
    expect(
      readAnswers({
        answers: [
          { id: "db", values: ["pg"] },
          { id: "auth", values: [], other: "Keycloak" },
        ],
        extra: 1,
      }),
    ).toEqual([
      { id: "db", values: ["pg"] },
      { id: "auth", values: [], other: "Keycloak" },
    ]);
  });

  it("is nothing for any other shape: a button's context, no values, a value that is not text", () => {
    for (const bad of [
      undefined,
      null,
      "x",
      [],
      {},
      { choice: "a" },
      { answers: [] },
      { answers: "x" },
      { answers: [null] },
      { answers: [{ values: ["a"] }] },
      { answers: [{ id: "a" }] },
      { answers: [{ id: "a", values: "a" }] },
      { answers: [{ id: "a", values: [1] }] },
      { answers: [{ id: "a", values: [], other: 3 }] },
      { answers: Array.from({ length: 51 }, (_, i) => ({ id: `q${i}`, values: [] })) },
    ]) {
      expect(readAnswers(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  const questions = spec().questions;

  it("shows the question and the labels of what was chosen", () => {
    expect(
      answerLines(
        [
          { id: "db", values: ["pg"] },
          { id: "auth", values: [], other: "Keycloak" },
          { id: "deploy", values: ["k8s", "compose"] },
        ],
        questions,
      ),
    ).toEqual([
      { id: "db", question: "Which database?", chosen: ["Postgres"] },
      { id: "auth", question: "Which login?", chosen: [], other: "Keycloak" },
      { id: "deploy", question: "Where?", chosen: ["Kubernetes", "Compose"] },
    ]);
  });

  it("falls back to the raw id and value for what the surface does not have, and without a surface", () => {
    expect(
      answerLines(
        [
          { id: "db", values: ["mongo"] },
          { id: "zzz", values: ["q"] },
        ],
        questions,
      ),
    ).toEqual([
      { id: "db", question: "Which database?", chosen: ["mongo"] },
      { id: "zzz", question: "zzz", chosen: ["q"] },
    ]);
    expect(answerLines([{ id: "db", values: ["pg"] }], undefined)).toEqual([
      { id: "db", question: "db", chosen: ["pg"] },
    ]);
  });
});

describe("the questions of a surface in the transcript", () => {
  const ops = (...components: Rec[]): Rec[] => [
    { version: "v0.9.1", createSurface: { surfaceId: "s1", catalogId: "https://x.test/c" } },
    { version: "v0.9.1", updateComponents: { surfaceId: "s1", components } },
  ];

  it("reads the Choices the action's source names", () => {
    const found = questionsOf(
      ops({ id: "root", component: "Column", children: ["pick"] }, component()),
      "s1",
      "pick",
    );
    expect(found?.map((q) => q.id)).toEqual(["db", "auth", "deploy"]);
  });

  it("without a source id, the first Choices of the surface", () => {
    expect(questionsOf(ops(component()), "s1")?.length).toBe(3);
    expect(questionsOf(ops(component()), "s1", "")?.length).toBe(3);
  });

  it("the last definition of a component wins, and a later createSurface starts the surface over", () => {
    const later = [
      ...ops(component()),
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            component({
              questions: [
                {
                  id: "x",
                  question: "X?",
                  options: [
                    { value: "a", label: "A" },
                    { value: "b", label: "B" },
                  ],
                },
              ],
            }),
          ],
        },
      },
    ];
    expect(questionsOf(later, "s1", "pick")?.map((q) => q.id)).toEqual(["x"]);
    const restarted = [...later, ...ops({ id: "root", component: "Text", text: "gone" })];
    expect(questionsOf(restarted, "s1", "pick")).toBeUndefined();
  });

  it("is nothing for another surface, another component, or operations that are not a list", () => {
    expect(questionsOf(ops(component()), "other", "pick")).toBeUndefined();
    expect(questionsOf(ops(component()), "s1", "nope")).toBeUndefined();
    expect(
      questionsOf(ops({ id: "pick", component: "Text", text: "t" }), "s1", "pick"),
    ).toBeUndefined();
    expect(questionsOf("x", "s1")).toBeUndefined();
    expect(questionsOf(undefined, "s1")).toBeUndefined();
    expect(questionsOf([null, 1, {}], "s1")).toBeUndefined();
  });
});

describe("version 5: a form, typed questions, defaults and a note", () => {
  const opt = (value: string, label = value, extra: Rec = {}) => ({ value, label, ...extra });
  const q = (over: Rec = {}): Rec => ({
    id: "q",
    question: "Which?",
    options: [opt("a", "Alpha"), opt("b", "Beta"), opt("c", "Gamma")],
    ...over,
  });
  const form = (questions: Rec[], over: Rec = {}): ChoicesSpec => {
    const read = readChoices({ id: "pick", component: "Choices", questions, ...over });
    if (!read) throw new Error("not a Choices");
    return read;
  };
  const only = (over: Rec = {}) => {
    const first = form([q(over)]).questions[0];
    if (!first) throw new Error("fixture");
    return first;
  };

  it("reads the title, the description and the free box (its label defaults)", () => {
    const s = form([q()], {
      title: "Before I start",
      description: "I need these to write the migration.",
      freeInput: { label: "Anything else?", placeholder: "A deadline…" },
    });
    expect([s.title, s.description, s.freeInput]).toEqual([
      "Before I start",
      "I need these to write the migration.",
      { label: "Anything else?", placeholder: "A deadline…" },
    ]);
    expect(form([q()], { freeInput: {} }).freeInput).toEqual({ label: FREE_INPUT_LABEL });
    expect(form([q()], { freeInput: "yes" }).freeInput).toBeUndefined();
    expect(form([q()]).freeInput).toBeUndefined();
    expect(form([q()]).description).toBeUndefined();
  });

  it("a v4 question reads as a single or multiple question with no help, no default and no limits", () => {
    const one = only();
    expect(one).toMatchObject({ type: "single", multiple: false, required: true, defaults: [] });
    expect(one.help).toBeUndefined();
    expect(one.min).toBeUndefined();
    expect(only({ multiple: true })).toMatchObject({ type: "multiple", multiple: true });
  });

  it("the type says how the question is answered; multiple: true is the type multiple", () => {
    expect(only({ type: "multiple" })).toMatchObject({ type: "multiple", multiple: true });
    expect(only({ type: "select" })).toMatchObject({ type: "select", multiple: false });
    expect(only({ type: "select", multiple: true })).toMatchObject({
      type: "select",
      multiple: true,
    });
    expect(only({ type: "images" })).toMatchObject({ type: "images", multiple: false });
    expect(only({ type: "text", options: undefined })).toMatchObject({
      type: "text",
      options: [],
      multiple: false,
    });
    // a type that is not one of the six is read as the old spelling says
    expect(only({ type: "slider" })).toMatchObject({ type: "single" });
  });

  it("reads help, defaults (a value or values), min and max, and the options' picture, icon and mark", () => {
    const one = only({
      help: "Pick the one you can run",
      type: "multiple",
      default: ["a", "c"],
      min: 1,
      max: 2,
      options: [
        opt("a", "Alpha", { description: "first", icon: "git", recommended: true }),
        opt("b", "Beta", { image: { sha256: "f".repeat(64), alt: "Beta, a diagram" } }),
        opt("c"),
      ],
    });
    expect(one.help).toBe("Pick the one you can run");
    expect(one.defaults).toEqual(["a", "c"]);
    expect([one.min, one.max]).toEqual([1, 2]);
    expect(one.options[0]).toEqual({
      value: "a",
      label: "Alpha",
      description: "first",
      icon: "git",
      recommended: true,
    });
    expect(one.options[1]?.image).toEqual({ sha256: "f".repeat(64), alt: "Beta, a diagram" });
    expect(only({ default: "b" }).defaults).toEqual(["b"]);
    expect(only({ type: "text", options: undefined, default: "Ada" }).defaultText).toBe("Ada");
  });

  it("is nothing for an option list that is not a list, and tolerates a missing one", () => {
    expect(readChoices({ questions: [{ id: "a", question: "q", options: "x" }] })).toBeUndefined();
    expect(readChoices({ questions: [{ id: "a", question: "q" }] })?.questions[0]?.options).toEqual(
      [],
    );
  });

  describe("limitsIn", () => {
    const problem = (questions: Rec[]) => limitsIn(form(questions));
    const opts = (n: number, extra: Rec = {}) =>
      Array.from({ length: n }, (_, i) => opt(`v${i}`, `Option ${i}`, extra));

    it("holds for what v4 allowed and for each type at its limits", () => {
      expect(problem([q(), q({ id: "r", multiple: true })])).toBeUndefined();
      expect(problem([q({ options: opts(12) })])).toBeUndefined();
      expect(problem([q({ type: "select", options: opts(50) })])).toBeUndefined();
      expect(
        problem([
          q({ type: "images", options: opts(16, { image: { sha256: "a".repeat(64), alt: "x" } }) }),
        ]),
      ).toBeUndefined();
      expect(problem([{ id: "t", question: "Name?", type: "text" }])).toBeUndefined();
    });

    it("counts options by type, and says which type to use for more", () => {
      expect(problem([q({ options: opts(13) })])).toMatch(/2 to 12.*"select"/);
      expect(problem([q({ type: "multiple", options: opts(1) })])).toMatch(/checkbox.*2 to 12/);
      expect(problem([q({ type: "select", options: opts(51) })])).toMatch(/2 to 50/);
      expect(
        problem([
          q({ type: "images", options: opts(17, { image: { sha256: "a".repeat(64), alt: "x" } }) }),
        ]),
      ).toMatch(/2 to 16/);
    });

    it("at most 8 questions", () => {
      const nine = Array.from({ length: 9 }, (_, i) => q({ id: `q${i}` }));
      expect(problem(nine)).toBe("a form has at most 8 questions, this has 9");
    });

    it("a deck needs a picture on every tile and no other type takes one", () => {
      const pic = { image: { sha256: "a".repeat(64), alt: "x" } };
      expect(problem([q({ type: "images", options: [opt("a", "A", pic), opt("b")] })])).toMatch(
        /"b".*no image/,
      );
      expect(problem([q({ options: [opt("a", "A", pic), opt("b")] })])).toMatch(
        /only type "images"/,
      );
    });

    it("contradictions and misplaced members are named", () => {
      expect(problem([q({ type: "single", multiple: true })])).toMatch(
        /"single" but has multiple: true/,
      );
      expect(problem([q({ type: "multiple", multiple: false })])).toMatch(
        /"multiple" but has multiple: false/,
      );
      expect(problem([q({ min: 1 })])).toMatch(/one choice/);
      expect(problem([{ id: "t", question: "?", type: "longText", allowOther: true }])).toMatch(
        /allowOther/,
      );
      expect(problem([{ id: "t", question: "?", type: "text", options: opts(2) }])).toMatch(
        /no options/,
      );
    });

    it("defaults must be options, min and max must fit the offered choices (an Other is one more)", () => {
      expect(problem([q({ default: "zzz" })])).toMatch(/"zzz".*not one of its options/);
      expect(problem([q({ type: "multiple", default: ["a", "zzz"] })])).toMatch(/"zzz"/);
      expect(problem([q({ default: ["a", "b"] })])).toMatch(/one choice/);
      expect(problem([q({ type: "multiple", max: 4 })])).toMatch(/offers 3/);
      expect(problem([q({ type: "multiple", max: 4, allowOther: true })])).toBeUndefined();
      expect(problem([q({ type: "multiple", min: 3, max: 2 })])).toMatch(/above its max/);
      expect(problem([q({ type: "multiple", default: ["a", "b"], max: 1 })])).toMatch(
        /defaults, above its max/,
      );
    });
  });

  describe("answering a typed question and the free box", () => {
    const text = (over: Rec = {}) => ({ id: "t", question: "Your name?", type: "text", ...over });
    const s = form([text(), text({ id: "u", type: "longText", required: false }), q({ id: "o" })], {
      freeInput: { label: "More?" },
    });
    const [typed, long, pick] = s.questions;
    if (!typed || !long || !pick) throw new Error("fixture");

    it("a typed question is answered by words, and an empty or blank one is not", () => {
      expect(isAnswered(typed)).toBe(false);
      expect(isAnswered(typed, st({ text: "   " }))).toBe(false);
      expect(isAnswered(typed, st({ text: "Ada" }))).toBe(true);
      expect(chosenCount(typed, st({ text: "Ada" }))).toBe(1);
    });

    it("sends `text` trimmed and cut at the type's limit, with empty values, only when there are words", () => {
      const answers = buildAnswers(
        s,
        state({
          t: st({ text: "  Ada Lovelace  " }),
          u: st({ text: "y".repeat(MAX_LONG_TEXT + 10) }),
          o: st({ values: ["b"] }),
        }),
      );
      expect(answers).toEqual([
        { id: "t", values: [], text: "Ada Lovelace" },
        { id: "u", values: [], text: "y".repeat(MAX_LONG_TEXT) },
        { id: "o", values: ["b"] },
      ]);
      expect(
        buildAnswers(s, state({ t: st({ text: "x".repeat(MAX_TEXT + 5) }) }))[0]?.text,
      ).toHaveLength(MAX_TEXT);
      expect(buildAnswers(s, state({}))[0]).toEqual({ id: "t", values: [] });
      // words typed in a question with options are not sent: only a typed question has `text`
      expect(buildAnswers(s, state({ o: st({ text: "left over", values: ["a"] }) }))[2]).toEqual({
        id: "o",
        values: ["a"],
      });
    });

    it("the note is the free box's words, trimmed and cut, and only when the form has a box", () => {
      const done = state({ t: st({ text: "Ada" }), o: st({ values: ["a"] }) });
      expect(answerContext(s, done, "  by Friday  ").note).toBe("by Friday");
      expect(answerContext(s, done, "x".repeat(MAX_NOTE + 7)).note).toHaveLength(MAX_NOTE);
      expect("note" in answerContext(s, done, "   ")).toBe(false);
      expect("note" in answerContext(s, done)).toBe(false);
      // a form without a free box has no note, whatever the state says
      const bare = form([q()]);
      expect("note" in answerContext(bare, state({}), "ignored")).toBe(false);
    });

    it("the answer has the shape {answers: [{id, values, other?, text?}], note?}", () => {
      const shaped = form(
        [
          text(),
          text({ id: "u", type: "longText", required: false }),
          q({ id: "o", allowOther: true }),
        ],
        { freeInput: {} },
      );
      expect(
        answerContext(
          shaped,
          state({ t: st({ text: "Ada" }), o: st({ otherOn: true, other: "Delta" }) }),
          "hi",
        ),
      ).toEqual({
        answers: [
          { id: "t", values: [], text: "Ada" },
          { id: "u", values: [] },
          { id: "o", values: [], other: "Delta" },
        ],
        note: "hi",
      });
    });
  });

  describe("min, max and what lets the form go", () => {
    const many = only({
      type: "multiple",
      min: 2,
      max: 3,
      options: [opt("a"), opt("b"), opt("c"), opt("d")],
    });

    it("a required question needs `min` choices, and never more than `max`", () => {
      expect(isSatisfied(many)).toBe(false);
      expect(isSatisfied(many, st({ values: ["a"] }))).toBe(false);
      expect(isSatisfied(many, st({ values: ["a", "b"] }))).toBe(true);
      expect(isSatisfied(many, st({ values: ["a", "b", "c"] }))).toBe(true);
      expect(isSatisfied(many, st({ values: ["a", "b", "c", "d"] }))).toBe(false);
    });

    it("an optional question lets the form go empty, but not short of `min`", () => {
      const optional = only({
        type: "multiple",
        required: false,
        min: 2,
        options: [opt("a"), opt("b"), opt("c")],
      });
      expect(isSatisfied(optional)).toBe(true);
      expect(isSatisfied(optional, st({ values: ["a"] }))).toBe(false);
      expect(isSatisfied(optional, st({ values: ["a", "b"] }))).toBe(true);
    });

    it("an Other with words is a choice that counts", () => {
      const withOther = only({
        type: "multiple",
        min: 2,
        allowOther: true,
        options: [opt("a"), opt("b")],
      });
      expect(chosenCount(withOther, st({ values: ["a"], otherOn: true, other: "x" }))).toBe(2);
      expect(isSatisfied(withOther, st({ values: ["a"], otherOn: true, other: "x" }))).toBe(true);
      expect(isSatisfied(withOther, st({ values: ["a"], otherOn: true, other: " " }))).toBe(false);
    });

    it("`missing` counts the questions that hold the form back", () => {
      const s = form([q({ id: "a" }), q({ id: "m", type: "multiple", min: 2 })]);
      expect(missing(s, state({}))).toBe(2);
      expect(missing(s, state({ a: st({ values: ["a"] }), m: st({ values: ["a"] }) }))).toBe(1);
      expect(missing(s, state({ a: st({ values: ["a"] }), m: st({ values: ["a", "b"] }) }))).toBe(
        0,
      );
    });
  });

  describe("defaults", () => {
    it("start the form with them chosen, in the options that exist, and one for a single choice", () => {
      const s = form([
        q({ id: "one", default: "b" }),
        q({ id: "many", type: "multiple", default: ["c", "a", "gone"] }),
        { id: "t", question: "Name?", type: "text", default: "Ada" },
        q({ id: "none" }),
      ]);
      const start = initialState(s);
      expect(start.get("one")?.values).toEqual(["b"]);
      expect(start.get("many")?.values).toEqual(["c", "a"]);
      expect(start.get("t")?.text).toBe("Ada");
      expect(start.has("none")).toBe(false);
      // sent like any choice, in the options' order
      expect(buildAnswers(s, start).map((a) => [a.id, a.values, a.text])).toEqual([
        ["one", ["b"], undefined],
        ["many", ["a", "c"], undefined],
        ["t", [], "Ada"],
        ["none", [], undefined],
      ]);
      expect(missing(s, start)).toBe(1);
    });
  });

  describe("reading an answer back", () => {
    it("reads `text` and the context's `note`", () => {
      expect(readAnswers({ answers: [{ id: "t", values: [], text: "Ada" }], note: "n" })).toEqual([
        { id: "t", values: [], text: "Ada" },
      ]);
      expect(readAnswers({ answers: [{ id: "t", values: [], text: 3 }] })).toBeNull();
      expect(readNote({ answers: [], note: "by Friday" })).toBe("by Friday");
      expect(readNote({ note: "   " })).toBeUndefined();
      expect(readNote({ note: 4 })).toBeUndefined();
      expect(readNote(null)).toBeUndefined();
    });

    it("shows what was typed under the question it answered", () => {
      const s = form([{ id: "t", question: "Your name?", type: "text" }, q({ id: "o" })]);
      expect(
        answerLines(
          [
            { id: "t", values: [], text: "Ada" },
            { id: "o", values: ["a"] },
          ],
          s.questions,
        ),
      ).toEqual([
        { id: "t", question: "Your name?", chosen: [], text: "Ada" },
        { id: "o", question: "Which?", chosen: ["Alpha"] },
      ]);
    });

    it("finds the whole form of a surface, with its free box's label", () => {
      const found = choicesOf(
        [
          { version: "v0.9.1", createSurface: { surfaceId: "s1", catalogId: "https://x.test/c" } },
          {
            version: "v0.9.1",
            updateComponents: {
              surfaceId: "s1",
              components: [
                {
                  id: "pick",
                  component: "Choices",
                  questions: [q()],
                  freeInput: { label: "More?" },
                },
              ],
            },
          },
        ],
        "s1",
        "pick",
      );
      expect(found?.freeInput?.label).toBe("More?");
      expect(found?.questions).toHaveLength(1);
    });
  });
});
