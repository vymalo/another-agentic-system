import { describe, expect, it } from "vitest";
import { insertQuote, quoteBlock, quoteCode } from "./quote";

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
