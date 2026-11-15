import { describe, expect, it } from "vitest";
import type { ApiPrompt, ApiPromptSummary } from "@/lib/api/types";
import { toForm } from "./editor";
import {
  asksWords,
  buildChange,
  buildCreate,
  byteLength,
  fieldsOf,
  isFrozen,
  type PromptFields,
  sharedLine,
  slugName,
  sortPrompts,
  splitPrompts,
} from "./library";

const summary = (over: Partial<ApiPromptSummary>): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "a",
  title: "A",
  description: "",
  variableCount: 0,
  own: true,
  revision: "1",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const prompt = (over: Partial<ApiPrompt> = {}): ApiPrompt => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "Reviews a change for one concern",
  body: "Review for {{focus}}.",
  variables: [
    {
      id: "focus",
      question: "Focus?",
      options: [
        { value: "sec", label: "Security" },
        { value: "speed", label: "Speed" },
      ],
    },
  ],
  own: true,
  audience: [],
  revision: "7",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

describe("sortPrompts", () => {
  it("sorts by name then id, which is what the contract tells a client to do", () => {
    const list = [
      summary({ id: "p-bbbbbbbbbbbb", name: "zeta" }),
      summary({ id: "p-dddddddddddd", name: "alpha" }),
      summary({ id: "p-cccccccccccc", name: "alpha" }),
    ];
    expect(sortPrompts(list).map((p) => p.id)).toEqual([
      "p-cccccccccccc",
      "p-dddddddddddd",
      "p-bbbbbbbbbbbb",
    ]);
  });

  it("splits what the person owns from what is shared with them, each sorted", () => {
    const split = splitPrompts([
      summary({ id: "p-1", name: "b", own: false }),
      summary({ id: "p-2", name: "z" }),
      summary({ id: "p-3", name: "a" }),
      summary({ id: "p-4", name: "a", own: false }),
    ]);
    expect(split.own.map((p) => p.id)).toEqual(["p-3", "p-2"]);
    expect(split.shared.map((p) => p.id)).toEqual(["p-4", "p-1"]);
  });
});

describe("isFrozen", () => {
  it("is true only for an own, shared prompt when the permission is known to be missing", () => {
    const shared = summary({ audience: ["team"] });
    expect(isFrozen(shared, false)).toBe(true);
    expect(isFrozen(shared, true)).toBe(false);
    // `me` unknown: the server decides
    expect(isFrozen(shared, null)).toBe(false);
    expect(isFrozen(summary({ audience: [] }), false)).toBe(false);
    expect(isFrozen(summary({}), false)).toBe(false);
    expect(isFrozen(summary({ own: false }), false)).toBe(false);
  });
});

describe("slugName", () => {
  it("keeps at most 32 of [a-z0-9-] and drops leading hyphens", () => {
    expect(slugName("Review a change")).toBe("review-a-change");
    expect(slugName("  --Hello, World!  ")).toBe("hello-world");
    expect(slugName("Café crème")).toBe("cafe-creme");
    expect(slugName("🙂🙂")).toBe("");
    const long = slugName("word ".repeat(20));
    expect(long.length).toBeLessThanOrEqual(32);
    expect(long).toMatch(/^[a-z0-9][a-z0-9-]*[a-z0-9]$/);
  });
});

describe("what a card says", () => {
  it("says who a prompt is shared with, as the server sorted it", () => {
    expect(sharedLine(undefined)).toBeNull();
    expect(sharedLine([])).toBeNull();
    expect(sharedLine(["*"])).toBe("Shared with: Everyone");
    expect(sharedLine(["prompt-team", "user"])).toBe("Shared with: prompt-team, user");
  });

  it("says how many questions a prompt asks", () => {
    expect(asksWords(0)).toBe("Asks no questions");
    expect(asksWords(1)).toBe("Asks 1 question");
    expect(asksWords(2)).toBe("Asks 2 questions");
  });

  it("counts the bytes of the text as UTF-8, not characters", () => {
    expect(byteLength("abc")).toBe(3);
    expect(byteLength("é")).toBe(2);
    expect(byteLength("😀")).toBe(4);
  });
});

describe("buildCreate", () => {
  const fields = (over: Partial<PromptFields> = {}): PromptFields => ({
    title: "  Review  ",
    name: " review ",
    description: "",
    body: "Hello\n",
    questions: [],
    ...over,
  });

  it("trims the title and the name, keeps the body as typed and leaves out what is empty", () => {
    expect(buildCreate(fields())).toStrictEqual({
      name: "review",
      title: "Review",
      body: "Hello\n",
    });
  });

  it("sends a description and the questions when there are any, and never an audience", () => {
    const body = buildCreate(
      fields({
        description: " Reviews ",
        questions: toForm([{ id: "a", question: "A?", type: "text" }]),
      }),
    );
    expect(body).toStrictEqual({
      name: "review",
      title: "Review",
      description: "Reviews",
      body: "Hello\n",
      variables: [{ id: "a", question: "A?", type: "text" }],
    });
    expect("audience" in body).toBe(false);
  });
});

describe("buildChange", () => {
  it("is null when nothing changed, so no request is sent", () => {
    const was = prompt();
    expect(buildChange(was, fieldsOf(was))).toBeNull();
  });

  it("is null when only white space at the ends of a title or description changed", () => {
    const was = prompt();
    expect(
      buildChange(was, {
        ...fieldsOf(was),
        title: ` ${was.title} `,
        description: ` ${was.description}`,
      }),
    ).toBeNull();
  });

  it("sends only what changed, with the revision it was read at", () => {
    const was = prompt();
    expect(
      buildChange(was, { ...fieldsOf(was), title: "Review it", body: "Review for {{focus}} now." }),
    ).toStrictEqual({
      title: "Review it",
      body: "Review for {{focus}} now.",
      revision: "7",
    });
  });

  it("empties a description with an empty string, which is a change", () => {
    const was = prompt();
    expect(buildChange(was, { ...fieldsOf(was), description: "" })).toStrictEqual({
      description: "",
      revision: "7",
    });
  });

  it("sends variables only when they differ from what was read", () => {
    const was = prompt();
    const same = fieldsOf(was);
    expect(buildChange(was, { ...same, title: "Other" })).not.toHaveProperty("variables");
    const edited = fieldsOf(was);
    const first = edited.questions[0];
    if (!first || first.kept) throw new Error("editable");
    first.fields.question = "What focus?";
    const body = buildChange(was, edited);
    expect(body?.variables).toStrictEqual([{ ...was.variables[0], question: "What focus?" }]);
    expect(body).not.toHaveProperty("title");
  });

  it("sends an empty list when the last question is removed", () => {
    const was = prompt();
    expect(buildChange(was, { ...fieldsOf(was), questions: [] })).toStrictEqual({
      variables: [],
      revision: "7",
    });
  });

  it("never sends an audience, and a question this page cannot edit goes back unchanged", () => {
    const scale = { id: "depth", question: "Depth?", type: "scale", to: 5 } as const;
    const was = prompt({
      body: "{{focus}} {{depth}}",
      variables: [...prompt().variables, scale],
      audience: ["team"],
    });
    const body = buildChange(was, { ...fieldsOf(was), title: "New" });
    expect(body).toStrictEqual({ title: "New", revision: "7" });
    expect(body).not.toHaveProperty("audience");
  });
});
