import { describe, expect, it } from "vitest";
import { checkStartDescription, codePointLength, MAX_START_DESCRIPTION } from "./description";

/*
 * The description of a started subagent, as the server judges it (`check_start_description`,
 * orchestrator/crates/app/src/app/sessions.rs): trimmed, 1 to 80 characters counted in code points and
 * never UTF-16 units, one line, with no control character and no Unicode format character.
 */

describe("codePointLength", () => {
  it("counts code points, not UTF-16 units", () => {
    expect(codePointLength("abc")).toBe(3);
    expect(codePointLength("😀")).toBe(1);
    expect(codePointLength("😀😀")).toBe(2);
    expect(codePointLength("é")).toBe(1);
  });

  it("counts a lone surrogate as one", () => {
    expect(codePointLength("\ud83d")).toBe(1);
  });
});

describe("checkStartDescription", () => {
  it("is 80", () => {
    expect(MAX_START_DESCRIPTION).toBe(80);
  });

  it("takes one line and hands it back trimmed", () => {
    expect(checkStartDescription("  Find the writers  ")).toEqual({
      ok: true,
      value: "Find the writers",
      length: 16,
    });
  });

  it("keeps the spaces inside", () => {
    expect(checkStartDescription("a  b")).toMatchObject({ ok: true, value: "a  b" });
  });

  it("trims what Unicode calls white space, as the server does", () => {
    expect(checkStartDescription("  x　")).toMatchObject({ ok: true, value: "x" });
  });

  it("refuses nothing and blank text as blank", () => {
    expect(checkStartDescription("")).toEqual({ ok: false, reason: "blank" });
    expect(checkStartDescription("   \n\t ")).toEqual({ ok: false, reason: "blank" });
  });

  it("allows exactly 80 code points and refuses 81", () => {
    expect(checkStartDescription("a".repeat(80))).toMatchObject({ ok: true, length: 80 });
    expect(checkStartDescription("a".repeat(81))).toEqual({
      ok: false,
      reason: "too_long",
      length: 81,
    });
  });

  it("counts an emoji as one character: 80 of them are allowed, though they are 160 UTF-16 units", () => {
    const eighty = "😀".repeat(80);
    expect(eighty.length).toBe(160);
    expect(checkStartDescription(eighty)).toMatchObject({ ok: true, length: 80 });
    expect(checkStartDescription("😀".repeat(81))).toMatchObject({ ok: false, reason: "too_long" });
  });

  it("is two lines refused: a newline inside is a control character", () => {
    expect(checkStartDescription("one\ntwo")).toEqual({ ok: false, reason: "unreadable" });
    expect(checkStartDescription("one\r\ntwo")).toEqual({ ok: false, reason: "unreadable" });
    expect(checkStartDescription("tab\there")).toEqual({ ok: false, reason: "unreadable" });
  });

  it.each([
    ["NUL", "a\u0000b"],
    ["DEL", "a\u007fb"],
    ["a C1 control", "a\u009bb"],
  ])("refuses %s", (_label, text) => {
    expect(checkStartDescription(text)).toEqual({ ok: false, reason: "unreadable" });
  });

  it.each([
    ["a soft hyphen", "a­b"],
    ["a zero width space", "a​b"],
    ["a right-to-left mark", "a‏b"],
    ["a bidirectional override", "a‮b"],
    ["a word joiner", "a⁠b"],
    ["a bidirectional isolate", "a⁦b"],
    ["a byte order mark inside", "a﻿b"],
  ])("refuses %s", (_label, text) => {
    expect(checkStartDescription(text)).toEqual({ ok: false, reason: "unreadable" });
  });

  it("does not trim a byte order mark at the edge: the server refuses it", () => {
    expect(checkStartDescription("﻿abc")).toEqual({ ok: false, reason: "unreadable" });
  });

  it("judges an unreadable description before its length, as the server does", () => {
    expect(checkStartDescription(`${"a".repeat(100)}\u0000`)).toEqual({
      ok: false,
      reason: "unreadable",
    });
  });

  it("takes text from any script", () => {
    expect(checkStartDescription("Trouver les auteurs 作者を探す")).toMatchObject({ ok: true });
  });
});

describe("the edges of the characters the server refuses or trims", () => {
  it("refuses the last bidirectional isolate, U+2069", () => {
    expect(checkStartDescription("a\u2069b")).toEqual({ ok: false, reason: "unreadable" });
  });

  it("refuses the last bidirectional embedding mark, U+202E, and the first, U+202A", () => {
    expect(checkStartDescription("a\u202eb")).toEqual({ ok: false, reason: "unreadable" });
    expect(checkStartDescription("a\u202ab")).toEqual({ ok: false, reason: "unreadable" });
  });

  it("trims U+0085 and U+1680, which Rust's trim cuts", () => {
    expect(checkStartDescription("\u0085x\u0085")).toMatchObject({ ok: true, value: "x" });
    expect(checkStartDescription("\u1680x\u1680")).toMatchObject({ ok: true, value: "x" });
  });
});
