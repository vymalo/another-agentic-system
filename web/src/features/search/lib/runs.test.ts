import { describe, expect, it } from "vitest";
import { runsOf } from "./runs";

const strings = (runs: ReturnType<typeof runsOf>) =>
  runs.map((r) => (r.mark ? `[${r.text}]` : r.text));

describe("runsOf", () => {
  it("cuts the text at the matches, in order", () => {
    const text = "Fix the login page and the login form";
    const runs = runsOf({
      text,
      matches: [
        { start: 8, end: 13 },
        { start: 27, end: 32 },
      ],
    });
    expect(strings(runs)).toEqual(["Fix the ", "[login]", " page and the ", "[login]", " form"]);
    expect(runs.map((r) => r.text).join("")).toBe(text);
  });

  it("marks a word that ends exactly at the end of the snippet, with no plain run after it", () => {
    expect(strings(runsOf({ text: "fix login", matches: [{ start: 4, end: 9 }] }))).toEqual([
      "fix ",
      "[login]",
    ]);
  });

  it("is the whole text as one plain run when nothing matched", () => {
    expect(strings(runsOf({ text: "plain", matches: [] }))).toEqual(["plain"]);
  });

  it("counts UTF-16 code units: an astral character is two", () => {
    // 😀 is U+1F600: two units, so "café" starts at 3, not 2
    const text = "😀 café au lait";
    const start = text.indexOf("café");
    expect(start).toBe(3);
    const runs = runsOf({ text, matches: [{ start, end: start + 4 }] });
    expect(strings(runs)).toEqual(["😀 ", "[café]", " au lait"]);
  });

  it("keeps a combining mark inside its word", () => {
    // "e" + U+0301 is one letter drawn with two code points: the match covers both
    const text = "un café noir";
    const start = text.indexOf("cafe");
    const runs = runsOf({ text, matches: [{ start, end: start + 5 }] });
    expect(strings(runs)).toEqual(["un ", "[café]", " noir"]);
  });

  it("never marks inside a surrogate pair", () => {
    const text = "a😀b";
    // 2 is between the two halves of the emoji
    expect(strings(runsOf({ text, matches: [{ start: 0, end: 2 }] }))).toEqual(["a😀b"]);
    expect(strings(runsOf({ text, matches: [{ start: 1, end: 3 }] }))).toEqual(["a", "[😀]", "b"]);
  });

  it("skips offsets that are out of order, empty, negative-length or past the end, and still draws the whole text", () => {
    const text = "alpha beta gamma";
    const runs = runsOf({
      text,
      matches: [
        { start: 6, end: 10 },
        { start: 0, end: 5 }, // before the last one
        { start: 11, end: 11 }, // empty
        { start: 11, end: 99 }, // past the end
      ],
    });
    expect(strings(runs)).toEqual(["alpha ", "[beta]", " gamma"]);
  });

  it("gives every run a distinct key", () => {
    const runs = runsOf({
      text: "ab ab ab",
      matches: [
        { start: 0, end: 2 },
        { start: 3, end: 5 },
      ],
    });
    expect(new Set(runs.map((r) => r.at)).size).toBe(runs.length);
  });

  it("leaves markup as text", () => {
    const text = "<b>bold</b> & more";
    const runs = runsOf({ text, matches: [{ start: 3, end: 7 }] });
    expect(runs.map((r) => r.text).join("")).toBe(text);
  });
});
