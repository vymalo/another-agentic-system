import { describe, expect, it } from "vitest";
import { highlightCode, languageFor } from "./highlight";

describe("languageFor", () => {
  it("names a bundled language, in any case, and its usual aliases", () => {
    expect(languageFor("rust")).toBe("rust");
    expect(languageFor("Rust")).toBe("rust");
    expect(languageFor("rs")).toBe("rust");
    expect(languageFor("ts")).toBe("typescript");
    expect(languageFor("sh")).toBe("bash");
    expect(languageFor("yml")).toBe("yaml");
  });

  it("is null for a language the web does not bundle, for none and for a made-up name", () => {
    expect(languageFor("brainfuck")).toBeNull();
    expect(languageFor("")).toBeNull();
    expect(languageFor("text")).toBeNull();
    expect(languageFor("constructor")).toBeNull();
    expect(languageFor("__proto__")).toBeNull();
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
