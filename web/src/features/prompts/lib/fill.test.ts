import { describe, expect, it } from "vitest";
import type { ApiPromptSummary } from "@/lib/api/types";
import { fillFailureOf, fillProblemOf, matchingPrompts, promptQueryOf, specOf } from "./fill";

const summary = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "",
  variableCount: 0,
  own: true,
  revision: "1",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

const prompt = (over: Record<string, unknown> = {}) => ({
  id: "p-aaaaaaaaaaaa",
  name: "review",
  title: "Review a change",
  description: "",
  body: "Review {{focus}}.",
  variables: [
    {
      id: "focus",
      question: "What focus?",
      options: [
        { value: "security", label: "Security" },
        { value: "speed", label: "Speed" },
      ],
    },
  ],
  own: true,
  revision: "2",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

describe("the / trigger", () => {
  it("opens on one /name from the first character of the message", () => {
    expect(promptQueryOf("/")).toEqual({ query: "" });
    expect(promptQueryOf("/rev")).toEqual({ query: "rev" });
    expect(promptQueryOf("/review-2")).toEqual({ query: "review-2" });
  });

  it("does not open on a slash that is not the start, or on a message with white space", () => {
    expect(promptQueryOf(" /rev")).toBeNull();
    expect(promptQueryOf("ask /rev")).toBeNull();
    expect(promptQueryOf("/rev now")).toBeNull();
    expect(promptQueryOf("/rev\n")).toBeNull();
    expect(promptQueryOf("")).toBeNull();
  });
});

describe("matching", () => {
  const list = [
    summary({ id: "p-1", name: "review", title: "Review a change" }),
    summary({ id: "p-2", name: "release-notes", title: "Release notes" }),
    summary({ id: "p-3", name: "standup", title: "Write my review notes" }),
    summary({ id: "p-4", name: "cafe", title: "Café checklist" }),
    summary({ id: "p-5", name: "zz-review", title: "ZZ" }),
    summary({ id: "p-6", name: "review-x", title: "RX" }),
  ];

  it("lists every prompt, by name then id, for an empty query", () => {
    expect(matchingPrompts(list, "").map((p) => p.name)).toEqual([
      "cafe",
      "release-notes",
      "review",
      "review-x",
      "standup",
      "zz-review",
    ]);
  });

  it("puts the name first: exact, then prefix, then a word of it, then anywhere", () => {
    expect(matchingPrompts(list, "review").map((p) => p.name)).toEqual([
      "review",
      "review-x",
      "zz-review",
      "standup",
    ]);
    expect(matchingPrompts(list, "rel").map((p) => p.name)).toEqual(["release-notes"]);
    expect(matchingPrompts(list, "notes").map((p) => p.name)).toEqual(["release-notes", "standup"]);
  });

  it("finds a title, and folds accents", () => {
    expect(matchingPrompts(list, "change").map((p) => p.name)).toEqual(["review"]);
    expect(matchingPrompts(list, "cafe").map((p) => p.name)).toEqual(["cafe"]);
    expect(matchingPrompts(list, "CAFÉ").map((p) => p.name)).toEqual(["cafe"]);
  });

  it("breaks a tie by id", () => {
    const same = [
      summary({ id: "p-2", name: "one", title: "Same" }),
      summary({ id: "p-1", name: "one", title: "Same" }),
    ];
    expect(matchingPrompts(same, "one").map((p) => p.id)).toEqual(["p-1", "p-2"]);
  });

  it("answers nothing when nothing matches", () => {
    expect(matchingPrompts(list, "zzz")).toEqual([]);
  });
});

describe("the form of a prompt", () => {
  it("is a Choices spec of its variables, in order", () => {
    const spec = specOf(prompt());
    expect(spec?.questions.map((q) => q.id)).toEqual(["focus"]);
    expect(spec?.questions[0]?.options.map((o) => o.label)).toEqual(["Security", "Speed"]);
  });

  it("is an empty form for a prompt with no variables", () => {
    const spec = specOf(prompt({ variables: [] }));
    expect(spec?.questions).toEqual([]);
  });

  it("is not offered when a kind cannot be drawn", () => {
    expect(
      specOf(
        prompt({
          variables: [
            {
              id: "picture",
              question: "Which picture?",
              type: "images",
              options: [{ value: "a", label: "A", image: { sha256: "a".repeat(64), alt: "A" } }],
            },
          ],
        }),
      ),
    ).toBeUndefined();
    expect(
      specOf(
        prompt({
          variables: [{ id: "future", question: "A later kind?", type: "voice" }],
        }),
      ),
    ).toBeUndefined();
  });
});

describe("what a refused fill says", () => {
  const spec = specOf(prompt());

  it("reads a problem's code, question and rule", () => {
    expect(
      fillProblemOf({ code: "invalid_answer", question: "focus", rule: "required" }, 422),
    ).toEqual({ status: 422, code: "invalid_answer", question: "focus", rule: "required" });
    expect(fillProblemOf(null, 503)).toEqual({ status: 503 });
  });

  it("offers the current version for a stale revision, and keeps the answers", () => {
    const failure = fillFailureOf({ status: 409, code: "stale_revision" }, spec, 32768);
    expect(failure.stale).toBe(true);
    expect(failure.message).toContain("changed elsewhere");
    expect(failure.message).toContain("Load the current version");
    expect(failure.message).toContain("still here");
  });

  it("says a prompt is gone, and that the list is read again", () => {
    const failure = fillFailureOf({ status: 404 }, spec, 32768);
    expect(failure.gone).toBe(true);
    expect(failure.message).toBe("This prompt no longer exists, or is no longer shared with you.");
  });

  it("names the question and the rule the server refused", () => {
    const failure = fillFailureOf(
      { status: 422, code: "invalid_answer", question: "focus", rule: "not_offered" },
      spec,
      32768,
    );
    expect(failure.question).toBe("focus");
    expect(failure.message).toContain("What focus?");
    expect(failure.message).toContain("not one of the choices");
  });

  it("says when the answers as a whole do not fit", () => {
    const failure = fillFailureOf(
      { status: 422, code: "invalid_answer", question: "*", rule: "unexpected" },
      spec,
      32768,
    );
    expect(failure.question).toBeUndefined();
    expect(failure.message).toContain("do not fit");
  });

  it("says when the filled text would be too long, with the limit in KiB", () => {
    const failure = fillFailureOf({ status: 422, code: "rendered_too_long" }, spec, 32768);
    expect(failure.message).toContain("32 KiB");
    expect(failure.message).toContain("Shorten");
  });

  it("says a store that cannot answer is worth trying again", () => {
    expect(fillFailureOf({ status: 503 }, spec, 32768).message).toContain("Try again");
    expect(fillFailureOf({ status: 0 }, spec, 32768).message).toContain("Try again");
  });

  it("says a session that ended, and roles that do not allow it", () => {
    expect(fillFailureOf({ status: 401 }, spec, 32768).message).toContain("Sign in");
    expect(fillFailureOf({ status: 403 }, spec, 32768).message).toContain("roles");
    expect(fillFailureOf({ status: 429 }, spec, 32768).message).toContain("Too many");
  });
});
