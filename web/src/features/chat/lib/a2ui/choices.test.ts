import { describe, expect, it } from "vitest";
import {
  ANSWER_ACTION,
  type AnswerState,
  answerContext,
  answerLines,
  buildAnswers,
  type ChoicesSpec,
  duplicateIn,
  EMPTY,
  isAnswered,
  MAX_OTHER,
  missing,
  type QuestionState,
  questionsOf,
  readAnswers,
  readChoices,
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
