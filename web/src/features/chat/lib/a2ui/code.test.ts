import { describe, expect, it } from "vitest";
import { diffLines, exitStatus, FOLD_LINES, foldOutput, visibleControls } from "./code";

describe("diffLines: what each line of a unified diff is", () => {
  it("classifies a file header, a hunk, an addition, a removal and a context line", () => {
    const lines = diffLines(
      ["diff --git a/x b/x", "--- a/x", "+++ b/x", "@@ -1 +1 @@", "-a", "+b", " same"].join("\n"),
    );
    expect(lines.map((l) => l.kind)).toEqual([
      "header",
      "header",
      "header",
      "hunk",
      "remove",
      "add",
      "context",
    ]);
  });

  it("keeps the text of a line without its sign", () => {
    expect(diffLines("@@ -1 +1 @@\n+hello\n-bye").map((l) => l.text)).toEqual([
      "@@ -1 +1 @@",
      "hello",
      "bye",
    ]);
  });

  it("a removed line that begins with two dashes is a removal, inside its hunk", () => {
    const lines = diffLines(
      ["--- a/x", "+++ b/x", "@@ -1,2 +1,1 @@", "--- a comment", " kept"].join("\n"),
    );
    expect(lines.map((l) => l.kind)).toEqual(["header", "header", "hunk", "remove", "context"]);
  });

  it("a second file's header after a hunk is a header again", () => {
    const lines = diffLines(
      [
        "--- a/x",
        "+++ b/x",
        "@@ -1 +1 @@",
        "-a",
        "+b",
        "--- a/y",
        "+++ b/y",
        "@@ -1 +1 @@",
        "-c",
        "+d",
      ].join("\n"),
    );
    expect(lines.map((l) => l.kind)).toEqual([
      "header",
      "header",
      "hunk",
      "remove",
      "add",
      "header",
      "header",
      "hunk",
      "remove",
      "add",
    ]);
  });

  it("CRLF line endings leave no carriage return in the text", () => {
    expect(diffLines("+a\r\n-b\r\n").map((l) => l.text)).toEqual(["a", "b", ""]);
  });
});

describe("exitStatus", () => {
  it("zero is ok, another code is failed with that code, and none is unknown", () => {
    expect(exitStatus(0)).toEqual({ kind: "ok", code: 0 });
    expect(exitStatus(2)).toEqual({ kind: "failed", code: 2 });
    expect(exitStatus(-9)).toEqual({ kind: "failed", code: -9 });
    expect(exitStatus(undefined)).toEqual({ kind: "unknown" });
  });
});

describe("foldOutput: a long output folds after FOLD_LINES lines", () => {
  it("is shown whole when it has no more lines than that", () => {
    const short = Array.from({ length: FOLD_LINES }, (_, i) => `line ${i}`).join("\n");
    expect(foldOutput(short)).toEqual({ lines: FOLD_LINES, folded: false, shown: short });
  });

  it("shows the first FOLD_LINES lines of a longer one and counts all of them", () => {
    const long = Array.from({ length: FOLD_LINES + 5 }, (_, i) => `line ${i}`).join("\n");
    const folded = foldOutput(long);
    expect(folded.folded).toBe(true);
    expect(folded.lines).toBe(FOLD_LINES + 5);
    expect(folded.shown.split("\n")).toHaveLength(FOLD_LINES);
    expect(folded.shown.startsWith("line 0\n")).toBe(true);
  });

  it("an empty output is one line of nothing, not folded", () => {
    expect(foldOutput("")).toEqual({ lines: 1, folded: false, shown: "" });
  });
});

describe("visibleControls: a character that hides text is shown in words, a tab and a line break stay", () => {
  it("shows each control and direction character as its code point, and keeps the tab and the line break", () => {
    expect(visibleControls("a\u202Eb\tc\nd\u0007e\u2067")).toBe(
      "a\u27e8U+202E\u27e9b\tc\nd\u27e8U+0007\u27e9e\u27e8U+2067\u27e9",
    );
  });

  it("leaves plain code as it is", () => {
    const code = "const s = 'ünï ✓ 🙂';\n\tfoo();";
    expect(visibleControls(code)).toBe(code);
  });
});
