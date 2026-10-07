import { describe, expect, it } from "vitest";
import { MESSAGE_LIMIT, splitFailure } from "./failure-text";

describe("splitFailure", () => {
  it("keeps a short single line as the message and has no details", () => {
    expect(splitFailure("scripted failure")).toEqual({
      message: "scripted failure",
      details: "",
    });
    expect(splitFailure("  padded  \n")).toEqual({ message: "padded", details: "" });
  });

  it("takes the first line as the message and the rest as written, indentation and frames included", () => {
    const text = [
      "yarn check failed: 3 errors",
      "",
      "src/a.ts:1:1 - error TS1",
      "",
      "1   const x: number = 'a';",
      "          ~",
      "",
    ].join("\n");
    expect(splitFailure(text)).toEqual({
      message: "yarn check failed: 3 errors",
      details: "src/a.ts:1:1 - error TS1\n\n1   const x: number = 'a';\n          ~",
    });
  });

  it("skips blank lines before the message and reads CRLF", () => {
    expect(splitFailure("\n\r\nfirst\r\nsecond\r\n    third\r\n")).toEqual({
      message: "first",
      details: "second\n    third",
    });
  });

  it("cuts one very long first line and keeps the whole text behind the details", () => {
    const long = "x".repeat(MESSAGE_LIMIT + 50);
    const { message, details } = splitFailure(`${long}\nnext`);
    expect(message).toBe(`${"x".repeat(MESSAGE_LIMIT)}…`);
    expect(details).toBe(`${long}\nnext`);
    expect(splitFailure(long).details).toBe(long);
  });

  it("does not cut inside a character", () => {
    const text = "😀".repeat(MESSAGE_LIMIT + 5);
    const { message } = splitFailure(text);
    expect([...message]).toHaveLength(MESSAGE_LIMIT + 1);
    expect(message.endsWith("…")).toBe(true);
  });

  it("says nothing for nothing", () => {
    expect(splitFailure("")).toEqual({ message: "", details: "" });
    expect(splitFailure("  \n \n")).toEqual({ message: "", details: "" });
  });
});
