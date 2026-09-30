import { describe, expect, it } from "vitest";
import { keyed, pluralFindings, shortCommit, sourceLabel, truncate } from "./findings";

describe("shortening untrusted text", () => {
  it("leaves short text alone and cuts long text with an ellipsis", () => {
    expect(truncate("abc", 3)).toEqual({ text: "abc", cut: false });
    expect(truncate("abcd", 3)).toEqual({ text: "abc…", cut: true });
    expect(truncate("ab  cd", 4)).toEqual({ text: "ab…", cut: true });
  });

  it("never cuts a surrogate pair in half", () => {
    const grin = "😀"; // two UTF-16 units
    expect(truncate(`a${grin}b`, 2)).toEqual({ text: "a…", cut: true });
    expect(truncate(`${grin}${grin}`, 3)).toEqual({ text: `${grin}…`, cut: true });
  });

  it("a commit is the first seven digits of a hash, and anything else is cut", () => {
    expect(shortCommit("0000000000000000000000000000000000000001")).toBe("0000000");
    expect(shortCommit("abc1234")).toBe("abc1234");
    expect(shortCommit("<script>alert(1)</script>")).toBe("<script>aler…");
    expect(shortCommit("not a hash")).toBe("not a hash");
  });

  it("the sources are named, and an unknown one is shown as it came, shortened", () => {
    expect(sourceLabel("ci")).toBe("CI");
    expect(sourceLabel("agent_checks")).toBe("Agent checks");
    expect(sourceLabel("verifier")).toBe("Verifier");
    expect(sourceLabel("security_scan")).toBe("security scan");
    expect(sourceLabel("x".repeat(100))).toBe(`${"x".repeat(40)}…`);
  });

  it("keys are unique even when a finding repeats", () => {
    const keys = keyed(["a", "b", "a", "a"]).map((k) => k.key);
    expect(new Set(keys).size).toBe(4);
  });

  it("counts findings in words", () => {
    expect(pluralFindings(0)).toBe("0 findings");
    expect(pluralFindings(1)).toBe("1 finding");
    expect(pluralFindings(2)).toBe("2 findings");
  });
});
