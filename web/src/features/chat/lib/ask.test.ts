import { describe, expect, it } from "vitest";
import { ASK_STATES } from "./agui/vymalo";
import { ASK_WORD, askNote, askWord } from "./ask";

describe("the words of an ask", () => {
  it("every state has its own words, so that none is told by colour alone", () => {
    expect(Object.keys(ASK_WORD).sort()).toEqual([...ASK_STATES].sort());
    expect(new Set(Object.values(ASK_WORD)).size).toBe(ASK_STATES.length);
    expect(ASK_WORD.running).toBe("Working");
    expect(ASK_WORD.failed).toBe("Failed");
    expect(ASK_WORD.timed_out).toBe("Timed out");
  });
});

describe("the words of a line", () => {
  it("are the state's, and an ask nobody runs any more is stopped, or waiting with its turn", () => {
    expect(askWord("running", "running")).toBe("Working");
    expect(askWord("running", "canceled")).toBe("Stopped");
    expect(askWord("running", "waiting")).toBe("Waiting");
    expect(askWord("failed", "failed")).toBe("Failed");
    expect(askWord("completed", "completed")).toBe("Answered");
  });
});

describe("the note a line keeps", () => {
  it("is the question an agent asked back, for input_required and auth_required", () => {
    expect(askNote({ state: "input_required", question: "Which branch?" })).toEqual({
      kind: "question",
      text: "Which branch?",
    });
    expect(askNote({ state: "auth_required", question: "Sign in at the portal" })).toEqual({
      kind: "question",
      text: "Sign in at the portal",
    });
    expect(askNote({ state: "input_required" })).toBeUndefined();
    expect(askNote({ state: "input_required", question: "  " })).toBeUndefined();
  });

  it("is the reason, for failed, rejected, canceled and timed_out", () => {
    for (const state of ["failed", "rejected", "canceled", "timed_out"] as const) {
      expect(askNote({ state, error: "scripted failure" })).toEqual({
        kind: "error",
        text: "scripted failure",
      });
      expect(askNote({ state })).toBeUndefined();
    }
  });

  it("is nothing for an ask that runs or answered, whatever else it carries", () => {
    expect(askNote({ state: "running", error: "x", question: "y" })).toBeUndefined();
    expect(askNote({ state: "completed", error: "x", question: "y" })).toBeUndefined();
  });
});
