import { describe, expect, it } from "vitest";
import { insertQuote, limitQuote, MAX_QUOTE, quoteBlock, quoteCode, sourceBlocks } from "./quote";

describe("quoteBlock", () => {
  it("puts > before every line and a bare > on a blank one", () => {
    expect(quoteBlock("a\n\nb")).toBe("> a\n>\n> b");
  });
  it("trims the blank lines around the words and reads any line ending", () => {
    expect(quoteBlock("\r\n\r\nline\r\nnext\r\n\r\n")).toBe("> line\n> next");
  });
  it("keeps what the words say as they are: no markup is added or removed", () => {
    expect(quoteBlock("# not a heading here\n- a *b*")).toBe("> # not a heading here\n> - a *b*");
  });
});

describe("quoteCode", () => {
  it("fences code inside the quote, with its language", () => {
    expect(quoteCode("let a = 1;\n", "rust")).toBe("> ```rust\n> let a = 1;\n> ```");
  });
  it("uses a fence longer than any run of backticks in the code", () => {
    expect(quoteCode("a ``` b", undefined)).toBe("> ````\n> a ``` b\n> ````");
  });
  it("leaves out a language label that is not a plain tag", () => {
    expect(quoteCode("x", "a b`")).toBe("> ```\n> x\n> ```");
  });
});

describe("insertQuote", () => {
  const q = "> hi";
  it("is the whole text in an empty box, the caret after the quote", () => {
    expect(insertQuote("", null, q)).toEqual({ text: "> hi\n\n", caret: 6 });
  });
  it("appends after the person's words as a paragraph of its own", () => {
    expect(insertQuote("Look at this", null, q).text).toBe("Look at this\n\n> hi\n\n");
    expect(insertQuote("Look\n", null, q).text).toBe("Look\n\n> hi\n\n");
    expect(insertQuote("Look\n\n", null, q).text).toBe("Look\n\n> hi\n\n");
  });
  it("goes in at the caret and keeps the rest after it", () => {
    const out = insertQuote("before after", 6, q);
    expect(out.text).toBe("before\n\n> hi\n\n after");
    expect(out.text.slice(out.caret)).toBe(" after");
  });
  it("clamps a caret that is outside the text", () => {
    expect(insertQuote("ab", 99, q).text).toBe("ab\n\n> hi\n\n");
    expect(insertQuote("ab", -3, q).text).toBe("> hi\n\nab");
  });
});

describe("the size of a quote", () => {
  it("keeps a text that fits", () => {
    expect(limitQuote("a\nb", 10)).toEqual({ text: "a\nb", cut: false });
  });
  it("cuts a longer one at the last whole line that fits, and says it was cut", () => {
    expect(limitQuote("aaa\nbbb\nccc", 8)).toEqual({ text: "aaa\nbbb", cut: true });
    expect(limitQuote("aaa\nbbb\nccc", 7)).toEqual({ text: "aaa\nbbb", cut: true });
    expect(limitQuote("aaa\nbbb\nccc", 6)).toEqual({ text: "aaa", cut: true });
  });
  it("cuts the first line itself when it alone is too long", () => {
    expect(limitQuote("abcdefgh", 4)).toEqual({ text: "abcd", cut: true });
  });
  it("is 20,000 characters", () => {
    expect(MAX_QUOTE).toBe(20_000);
    expect(limitQuote("x".repeat(20_001)).cut).toBe(true);
    expect(limitQuote("x".repeat(20_000)).cut).toBe(false);
  });
});

describe("where a quote goes in the box", () => {
  it("never splits a word: a caret inside one moves to its end", () => {
    expect(insertQuote("hi", 1, "> q").text).toBe("hi\n\n> q\n\n");
    expect(insertQuote("one two", 5, "> q").text).toBe("one two\n\n> q\n\n");
    expect(insertQuote("one two", 3, "> q").text).toBe("one\n\n> q\n\n two");
    expect(insertQuote("one\ntwo", 5, "> q").text).toBe("one\ntwo\n\n> q\n\n");
  });
  it("keeps the first quote when a second is added after it", () => {
    const first = insertQuote("", null, "> one");
    const second = insertQuote(first.text, first.caret, "> two");
    expect(second.text).toBe("> one\n\n> two\n\n");
    expect(second.caret).toBe(second.text.length);
  });
});

describe("the blocks a selection is quoted from", () => {
  it("are the top-level blocks of the source, with their offsets", () => {
    const source = "# T\n\npara one\nstill\n\n```js\ncode\n```\n\n$$\nx\n$$";
    const spans = sourceBlocks(source).map((b) => source.slice(b.start, b.end));
    expect(spans).toEqual(["# T", "para one\nstill", "```js\ncode\n```", "$$\nx\n$$"]);
  });
});
