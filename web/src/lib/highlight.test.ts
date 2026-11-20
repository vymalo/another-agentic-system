import { describe, expect, it } from "vitest";
import {
  highlightCode,
  languageFor,
  MAX_CODE_CHARS,
  MAX_CODE_LINES,
  withinLimits,
} from "./highlight";

describe("languageFor", () => {
  it("names a bundled language, in any case, and its usual aliases", () => {
    expect(languageFor("rust")).toBe("rust");
    expect(languageFor("Rust")).toBe("rust");
    expect(languageFor("rs")).toBe("rust");
    expect(languageFor("ts")).toBe("typescript");
    expect(languageFor("sh")).toBe("bash");
    expect(languageFor("dockerfile")).toBe("dockerfile");
  });

  it("is null for a language the web does not bundle, for none and for a made-up name", () => {
    expect(languageFor("brainfuck")).toBeNull();
    expect(languageFor("")).toBeNull();
    expect(languageFor("text")).toBeNull();
    expect(languageFor("constructor")).toBeNull();
    expect(languageFor("__proto__")).toBeNull();
  });

  it("is null for YAML and TOML, whose grammars have no verified licence (third-party-notices.md)", () => {
    for (const name of ["yaml", "yml", "toml"]) expect(languageFor(name)).toBeNull();
  });
});

describe("withinLimits", () => {
  it("accepts a block up to the limits and refuses one over either", () => {
    expect(withinLimits("")).toBe(true);
    expect(withinLimits("x".repeat(MAX_CODE_CHARS))).toBe(true);
    expect(withinLimits("x".repeat(MAX_CODE_CHARS + 1))).toBe(false);
    const lines = (n: number) => Array.from({ length: n }, () => "x").join("\n");
    expect(withinLimits(lines(MAX_CODE_LINES))).toBe(true);
    expect(withinLimits(`${lines(MAX_CODE_LINES)}\n`)).toBe(true);
    expect(withinLimits(lines(MAX_CODE_LINES + 1))).toBe(false);
  });
});

describe("highlightCode", () => {
  const code = 'fn main() {\n    println!("hi");\n}\n';

  it("keeps every character of the code, line by line, and colours it for both schemes", async () => {
    const lines = await highlightCode(code, "rust");
    expect(lines).not.toBeNull();
    const text = (lines ?? []).map((line) => line.map((t) => t.content).join("")).join("\n");
    expect(text).toBe(code.replace(/\n$/, ""));
    const keyword = (lines ?? []).flat().find((t) => t.content === "fn");
    expect(keyword?.style["--shiki-light"]).toMatch(/^#[0-9a-f]{6}/i);
    expect(keyword?.style["--shiki-dark"]).toMatch(/^#[0-9a-f]{6}/i);
    expect(keyword?.style["--shiki-light"]).not.toBe(keyword?.style["--shiki-dark"]);
  });

  it("answers null, not an error, for a language it does not have", async () => {
    expect(await highlightCode("+++++", "brainfuck")).toBeNull();
  });
});
