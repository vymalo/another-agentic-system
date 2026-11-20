import { describe, expect, it } from "vitest";
import { REFUSAL_CODES, refusalWords } from "./refusals";

/*
 * What a person is told when a subagent is refused (docs/api/chat-api.yaml: `startChildSession`,
 * `closeChildSession`, `runAgent` on a child thread). The words are ours, one for each code the contract
 * gives; a code we do not know falls back to the server's own `detail`. The agent's own words (a failed
 * start's `reason`) are drawn as text elsewhere and never come through here.
 */

describe("refusalWords", () => {
  it("has a code for every refusal the contract names", () => {
    expect([...REFUSAL_CODES].sort()).toEqual(
      [
        "bad_description",
        "child_mentions",
        "child_thread",
        "no_child_sessions",
        "no_context",
        "not_child_thread",
        "session_closed",
        "session_closes_on_stop",
        "too_many_sessions",
        "too_many_starts",
        "unknown_helper",
      ].sort(),
    );
  });

  it.each([...REFUSAL_CODES])("says %s in words of its own, not the server's", (code) => {
    const words = refusalWords(code, "SERVER DETAIL");
    expect(words).not.toContain("SERVER DETAIL");
    expect(words).not.toContain(code);
    expect(words.length).toBeGreaterThan(10);
    expect(words).toMatch(/[.]$/);
  });

  it("says the words of every code differently", () => {
    const all = REFUSAL_CODES.map((code) => refusalWords(code, undefined));
    expect(new Set(all).size).toBe(all.length);
  });

  it("calls a helper session a subagent, as the screens do", () => {
    for (const code of REFUSAL_CODES) {
      expect(refusalWords(code, undefined).toLowerCase()).not.toContain("helper session");
    }
  });

  it("falls back to the server's detail for a code it does not know", () => {
    expect(refusalWords("something_new", "The new thing is not allowed.")).toBe(
      "The new thing is not allowed.",
    );
  });

  it("falls back to the detail when there is no code", () => {
    expect(refusalWords(undefined, "the thread is working")).toBe("the thread is working");
  });

  it("falls back to a plain sentence when there is neither", () => {
    expect(refusalWords(undefined, undefined)).toBe("The request was refused. Try again.");
    expect(refusalWords("something_new", "")).toBe("The request was refused. Try again.");
  });

  it("does not take a code that only looks like one: a prototype member is no code", () => {
    expect(refusalWords("constructor", "detail")).toBe("detail");
    expect(refusalWords("__proto__", "detail")).toBe("detail");
    expect(refusalWords("toString", "detail")).toBe("detail");
  });
});

describe("refusalWords with a detail of white space only", () => {
  it("is no detail", () => {
    expect(refusalWords(undefined, "   ")).toBe("The request was refused. Try again.");
    expect(refusalWords("something_new", "\n\t")).toBe("The request was refused. Try again.");
  });
});
