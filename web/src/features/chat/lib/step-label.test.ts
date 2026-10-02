import { describe, expect, it } from "vitest";
import { formatBytes, inputPreview, notKept, PREVIEW_MAX, toolName } from "./step-label";

describe("toolName", () => {
  it("reads server__tool as the tool in words, from the server", () => {
    expect(toolName("search__web_search")).toEqual({ title: "Web search", server: "search" });
    expect(toolName("github__create-issue")).toEqual({ title: "Create issue", server: "github" });
    expect(toolName("fs__read.file")).toEqual({ title: "Read file", server: "fs" });
  });

  it("leaves every other label as it is", () => {
    for (const label of [
      "run_checks",
      "npm test",
      "Preparing the workspace",
      "a__b c",
      "__x",
      "x__",
      "OpenCode",
    ]) {
      expect(toolName(label)).toEqual({ title: label });
    }
  });
});

describe("inputPreview", () => {
  it("says what the call was about: the usual argument first, else the first text", () => {
    expect(inputPreview({ limit: 3, query: "Stephane Segning" })).toBe("Stephane Segning");
    expect(inputPreview({ n: 1, text: "hello there" })).toBe("hello there");
    expect(inputPreview({ url: "https://example.org/a", query: "q1" })).toBe("q1");
  });

  it("is one short line", () => {
    expect(inputPreview({ query: "a\n  b\t c" })).toBe("a b c");
    const long = inputPreview({ query: "x".repeat(200) }) as string;
    expect(long).toHaveLength(PREVIEW_MAX);
    expect(long.endsWith("…")).toBe(true);
  });

  it("has nothing to say of a credential, an input that was not kept, text it cannot find or no input", () => {
    expect(inputPreview({ api_key: "[redacted]" })).toBeUndefined();
    expect(inputPreview({ query: "[redacted]", n: 3 })).toBeUndefined();
    expect(inputPreview({ _cut: true, bytes: 18432 })).toBeUndefined();
    expect(inputPreview({ n: 3, ok: true, nested: { query: "deep" } })).toBeUndefined();
    expect(inputPreview({ query: "   " })).toBeUndefined();
    expect(inputPreview(undefined)).toBeUndefined();
  });
});

describe("sizes", () => {
  it("formats bytes for a person", () => {
    expect(formatBytes(0)).toBe("0 bytes");
    expect(formatBytes(1)).toBe("1 byte");
    expect(formatBytes(512)).toBe("512 bytes");
    expect(formatBytes(1024)).toBe("1 KiB");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(41_808)).toBe("41 KiB");
    expect(formatBytes(2 * 1024 * 1024)).toBe("2 MiB");
    expect(formatBytes(-1)).toBe("0 bytes");
    expect(formatBytes(Number.NaN)).toBe("0 bytes");
  });

  it("counts what a cut output left out: all of it less what is kept, in bytes", () => {
    expect(notKept({ text: "abc", truncated: true, bytes: 1000 })).toBe(997);
    // two bytes a character
    expect(notKept({ text: "éé", truncated: true, bytes: 10 })).toBe(6);
    expect(notKept({ text: "abc", truncated: true, bytes: 2 })).toBe(0);
    expect(notKept({ text: "abc" })).toBeUndefined();
    expect(notKept({ text: "abc", truncated: true })).toBeUndefined();
  });
});
