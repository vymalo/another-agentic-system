import { describe, expect, it } from "vitest";
import { saidPart, statusPart, stepPart } from "@/features/chat/components/steps/testing";
import { ACTIVITY, activityPartName } from "./agui/vymalo";
import { textRoles, tickerLine } from "./working";

const roles = (parts: readonly object[], running = false) => [
  ...textRoles(parts as never, running).entries(),
];

const artifact = {
  type: "data",
  name: activityPartName(ACTIVITY.artifact),
  data: { name: "branch", text: '{"branch":"b"}', mimeType: "application/json" },
};

describe("which text is the answer (ADR 0031)", () => {
  it("trusts the mark: working text is working and the answer is the answer, in a turn that is over", () => {
    const parts = [
      statusPart("working"),
      ...saidPart("I'll look.", "working"),
      stepPart("a", "completed"),
      ...saidPart("Now the tests.", "working"),
      stepPart("b", "completed"),
      ...saidPart("It is fixed.", "answer"),
    ];
    expect(roles(parts)).toEqual([
      [2, "working"],
      [5, "working"],
      [8, "answer"],
    ]);
  });

  it("reads text that is not marked by the rule: the last of a turn that is over is the answer", () => {
    const parts = [
      statusPart("working"),
      textPartOf("First."),
      stepPart("a", "completed"),
      textPartOf("Second."),
      stepPart("b", "completed"),
      textPartOf("Third."),
    ];
    expect(roles(parts)).toEqual([
      [1, "working"],
      [3, "working"],
      [5, "answer"],
    ]);
  });

  it("is the last text even when a step, an artifact or a check comes after it", () => {
    const parts = [
      textPartOf("Done, the pull request is open."),
      artifact,
      stepPart("a", "completed"),
    ];
    expect(roles(parts)).toEqual([[0, "answer"]]);
  });

  it("a turn of one text is its answer, marked or not, and a marked answer wins over a later unmarked text", () => {
    expect(roles([textPartOf("Hello.")])).toEqual([[0, "answer"]]);
    expect(roles([...saidPart("Hello.", "answer")])).toEqual([[1, "answer"]]);
    expect(roles([...saidPart("The result.", "answer"), textPartOf("There it is.")])).toEqual([
      [1, "answer"],
      [2, "working"],
    ]);
  });

  it("a turn has one answer: a later announced answer replaces an earlier one, which is working text", () => {
    const parts = [
      statusPart("working"),
      ...saidPart("The first try.", "answer"),
      stepPart("a", "completed"),
      ...saidPart("The answer, again.", "answer"),
      ...saidPart("Done.", "working"),
    ];
    expect(roles(parts)).toEqual([
      [2, "working"],
      [5, "answer"],
      [7, "working"],
    ]);
    // the same while the turn runs
    expect(roles(parts, true)).toEqual([
      [2, "working"],
      [5, "answer"],
      [7, "working"],
    ]);
  });

  it("a turn whose every text is working has no answer, and a blank text has no role", () => {
    expect(roles([...saidPart("One.", "working"), ...saidPart("Two.", "working")])).toEqual([
      [1, "working"],
      [3, "working"],
    ]);
    expect(roles([textPartOf("  \n"), textPartOf("Words.")])).toEqual([[1, "answer"]]);
  });

  it("a mark it does not know is no mark", () => {
    const parts = [
      { type: "data", name: "vymalo.purpose", data: { purpose: "thinking" } },
      textPartOf("Words."),
    ];
    expect(roles(parts)).toEqual([[1, "answer"]]);
  });

  describe("while the turn runs", () => {
    it("shows unmarked text as a draft of the answer until a step starts after it", () => {
      const parts = [statusPart("working"), textPartOf("Let me look.")];
      expect(roles(parts, true)).toEqual([[1, "answer"]]);
      expect(roles([...parts, stepPart("a", "running")], true)).toEqual([[1, "working"]]);
    });

    it("folds it for a status that says what the agent does, not for the statuses that come with words", () => {
      const parts = [textPartOf("Words.")];
      expect(roles([...parts, statusPart("working", "Preparing the workspace")], true)).toEqual([
        [0, "working"],
      ]);
      expect(roles([...parts, statusPart("completed", "Words.")], true)).toEqual([[0, "answer"]]);
    });

    it("does not fold it for an artifact: the words are still the last thing the agent said", () => {
      expect(roles([textPartOf("Pushed."), artifact], true)).toEqual([[0, "answer"]]);
    });

    it("keeps the mark of text that has one, whatever follows it", () => {
      const parts = [
        ...saidPart("Answer for now.", "answer"),
        stepPart("a", "running"),
        ...saidPart("I'll check.", "working"),
      ];
      expect(roles(parts, true)).toEqual([
        [1, "answer"],
        [4, "working"],
      ]);
    });
  });
});

function textPartOf(text: string) {
  return { type: "text", text };
}

describe("the ticker line", () => {
  it("is the last line of the note, plain, on one line", () => {
    expect(tickerLine("First thought.\n\nNow I'll run `npm test` and **fix** it.")).toBe(
      "Now I'll run npm test and fix it.",
    );
  });

  it("keeps the words of a link and drops a list marker and a heading's hashes", () => {
    expect(tickerLine("- see [the docs](https://example.com/x) first")).toBe("see the docs first");
    expect(tickerLine("## Plan")).toBe("Plan");
    expect(tickerLine("keeps_snake_case intact")).toBe("keeps_snake_case intact");
  });

  it("is cut with an ellipsis, and empty for words that say nothing", () => {
    const long = tickerLine("word ".repeat(100));
    expect(long.length).toBeLessThanOrEqual(160);
    expect(long.endsWith("…")).toBe(true);
    expect(tickerLine("  \n ")).toBe("");
    expect(tickerLine("``")).toBe("");
  });
});
