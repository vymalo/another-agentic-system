import { describe, expect, it } from "vitest";
import { deleteFailureOf, failureOf, parseAt } from "./failures";

const ctx = { limit: 100, maxVariables: 8, name: "review", body: "", ids: [] as string[] };

/** Every `code` the contract gives `createPrompt` and `updatePrompt` (docs/api/prompts-v1.md "Errors, and which wins"). */
const SAVE_CODES: { status: number; code: string }[] = [
  { status: 400, code: "bad_request" },
  { status: 400, code: "bad_name" },
  { status: 400, code: "bad_title" },
  { status: 400, code: "bad_description" },
  { status: 400, code: "bad_body" },
  { status: 400, code: "bad_reference" },
  { status: 400, code: "bad_variable" },
  { status: 400, code: "undeclared_variable" },
  { status: 400, code: "unused_variable" },
  { status: 400, code: "bad_audience" },
  { status: 403, code: "forbidden" },
  { status: 403, code: "no_access" },
  { status: 409, code: "name_taken" },
  { status: 409, code: "too_many_prompts" },
  { status: 409, code: "stale_revision" },
  { status: 415, code: "unsupported" },
  { status: 404, code: "" },
  { status: 503, code: "" },
];

describe("parseAt: where a save is refused", () => {
  it("names the members of the prompt", () => {
    for (const field of ["name", "title", "description", "body", "audience"] as const) {
      expect(parseAt(field)).toEqual({ field });
    }
    expect(parseAt("variables")).toEqual({ field: "questions" });
  });

  it("names a question and the member of it", () => {
    expect(parseAt("variables.2")).toEqual({ field: "question", index: 2, path: [] });
    expect(parseAt("variables.2.help")).toEqual({ field: "question", index: 2, path: ["help"] });
    expect(parseAt("variables.0.options.3.label")).toEqual({
      field: "question",
      index: 0,
      path: ["options", "3", "label"],
    });
  });

  it("an unknown at falls back to the form", () => {
    for (const at of [
      undefined,
      "",
      "variables.x",
      "variables.-1",
      "nope",
      "variables.1.",
      "body.1",
    ]) {
      expect(parseAt(at), String(at)).toEqual({ field: "form" });
    }
  });
});

describe("failureOf: the words for a refused save", () => {
  it("every code of createPrompt and updatePrompt has words, and none is empty or quotes a value", () => {
    for (const { status, code } of SAVE_CODES) {
      const f = failureOf({ status, ...(code ? { code } : {}), at: "name" }, ctx);
      expect(f.message.length, code || String(status)).toBeGreaterThan(10);
      expect(f.message).not.toContain("undefined");
    }
  });

  it("an unknown code says something true: the save was not accepted, and the status decides the rest", () => {
    const refused = failureOf({ status: 422, code: "from_the_future" }, ctx);
    expect(refused.message).toMatch(/could not be saved|was not accepted/i);
    expect(refused.reread).toBe(false);
    const down = failureOf({ status: 502, code: "from_the_future" }, ctx);
    expect(down.reread).toBe(true);
    expect(down.message).toMatch(/may or may not/);
    const lost = failureOf({ status: 0 }, ctx);
    expect(lost.reread).toBe(true);
  });

  it("at variables.2.options.1.label points at that field of that question", () => {
    const f = failureOf(
      { status: 400, code: "bad_variable", at: "variables.2.options.1.label" },
      ctx,
    );
    expect(f.target).toEqual({ field: "question", index: 2, path: ["options", "1", "label"] });
    expect(f.message).toContain("Question 3");
    expect(f.message).toContain("option 2's label");
  });

  it("a bad_variable with no member names the question", () => {
    const f = failureOf({ status: 400, code: "bad_variable", at: "variables.0" }, ctx);
    expect(f.message).toBe("Question 1 cannot be saved as it is.");
  });

  it("at exactly variables is the count of questions", () => {
    const f = failureOf({ status: 400, code: "bad_variable", at: "variables" }, ctx);
    expect(f.target).toEqual({ field: "questions" });
    expect(f.message).toBe("A prompt has at most 8 questions.");
  });

  it("bad_reference selects the two braces at the offset", () => {
    const f = failureOf({ status: 400, code: "bad_reference", at: "body", offset: 6 }, ctx);
    expect(f.target).toEqual({ field: "body" });
    expect(f.select).toEqual({ start: 6, end: 8 });
  });

  it("undeclared_variable selects the blank at the offset and names its id, read from the body", () => {
    const body = "Check {{focus}} and {{depth}}";
    const f = failureOf(
      { status: 400, code: "undeclared_variable", at: "body", offset: 20 },
      { ...ctx, body },
    );
    expect(f.select).toEqual({ start: 20, end: 29 });
    expect(f.message).toContain("{{depth}}");
    expect(f.message).toContain("Add a question named depth");
  });

  it("undeclared_variable counts in UTF-16 units, as the contract does", () => {
    const body = "😀 {{x}}";
    const f = failureOf(
      { status: 400, code: "undeclared_variable", at: "body", offset: 3 },
      { ...ctx, body },
    );
    expect(body.slice(f.select?.start, f.select?.end)).toBe("{{x}}");
  });

  it("undeclared_variable without a readable blank still says it and selects nothing wrong", () => {
    const f = failureOf({ status: 400, code: "undeclared_variable", at: "body", offset: 99 }, ctx);
    expect(f.message).toMatch(/not one of the questions/);
    expect(f.select).toBeUndefined();
  });

  it("unused_variable names the question's own id", () => {
    const f = failureOf(
      { status: 400, code: "unused_variable", at: "variables.1" },
      { ...ctx, ids: ["a", "focus"] },
    );
    expect(f.target).toEqual({ field: "question", index: 1, path: [] });
    expect(f.message).toContain("{{focus}}");
  });

  it("name_taken names the name and is a refusal at the name", () => {
    const f = failureOf({ status: 409, code: "name_taken", at: "name" }, ctx);
    expect(f.target).toEqual({ field: "name" });
    expect(f.message).toContain("/review");
    expect(f.reread).toBe(false);
  });

  it("stale_revision keeps the person's text and offers the current version", () => {
    const f = failureOf({ status: 409, code: "stale_revision" }, ctx);
    expect(f.stale).toBe(true);
    expect(f.reread).toBe(true);
    expect(f.message).toContain("Load the current version");
    expect(f.message).toContain("still here");
  });

  it("too_many_prompts says the limit and rereads", () => {
    const f = failureOf({ status: 409, code: "too_many_prompts" }, { ...ctx, limit: 3 });
    expect(f.message).toContain("You have 3 prompts");
    expect(f.reread).toBe(true);
  });

  it("forbidden says to stop sharing first", () => {
    const f = failureOf({ status: 403, code: "forbidden" }, ctx);
    expect(f.forbidden).toBe(true);
    expect(f.message).toContain("Stop sharing");
  });

  it("a 404 says the prompt is gone and rereads", () => {
    const f = failureOf({ status: 404 }, ctx);
    expect(f.message).toBe("This prompt no longer exists.");
    expect(f.reread).toBe(true);
  });
});

describe("failureOf: each code goes to its own field", () => {
  it.each([
    ["bad_name", { field: "name" }],
    ["name_taken", { field: "name" }],
    ["bad_title", { field: "title" }],
    ["bad_description", { field: "description" }],
    ["bad_body", { field: "body" }],
    ["bad_reference", { field: "body" }],
    // this page has no audience field (a prompt is only ever private from here): the page shows it with the form's words
    ["bad_audience", { field: "audience" }],
  ] as const)("%s", (code, target) => {
    const f = failureOf({ status: code === "name_taken" ? 409 : 400, code, at: "form" }, ctx);
    expect(f.target).toEqual(target);
  });

  it("a refusal with no code says what the status says", () => {
    expect(failureOf({ status: 401 }, ctx).message).toBe("Your session has ended. Sign in again.");
    expect(failureOf({ status: 403 }, ctx).message).toBe("Your roles do not allow this.");
    expect(failureOf({ status: 429 }, ctx).message).toBe(
      "Too many requests. Try again in a moment.",
    );
    expect(failureOf({ status: 409 }, ctx).message).toMatch(/did not accept it/);
    expect(deleteFailureOf({ status: 401 }).message).toBe("Your session has ended. Sign in again.");
    expect(deleteFailureOf({ status: 429 }).message).toBe(
      "Too many requests. Try again in a moment.",
    );
  });
});

describe("deleteFailureOf", () => {
  it("a stale delete says the list was read again", () => {
    const f = deleteFailureOf({ status: 409, code: "stale_revision" });
    expect(f.message).toContain("changed since you opened this page");
    expect(f.reread).toBe(true);
    expect(f.gone).toBe(false);
  });

  it("a 404 counts as gone", () => {
    expect(deleteFailureOf({ status: 404 }).gone).toBe(true);
  });

  it("a 503 says the outcome is not known", () => {
    const f = deleteFailureOf({ status: 503 });
    expect(f.message).toMatch(/may or may not/);
    expect(f.reread).toBe(true);
  });
});
