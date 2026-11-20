import { describe, expect, it } from "vitest";
import { MAX_FORMULA_CHARS } from "@/lib/remark-display-math";
import { chunkSpeech, SPEECH_CHUNK_CHARS, speechText } from "./speech-text";

/*
 * What Read aloud says of an answer: the words the page shows, not the Markdown it was written in. Headings and
 * emphasis are their words, code is its content, a link is its label, an image is its alt text, a table is read cell by
 * cell, raw HTML is read as it was written and display math is the word "formula" (a single dollar is a price).
 *
 * The rules are the contract's (`docs/api/message-text-v1.md`) where it has one — raw HTML as written, a single dollar a
 * price, the two shapes of display math — and ADR 0078's choices where it has not: a link's label, an image's alt text,
 * a table cell by cell, and a formula said as "formula". The parse is the renderer's own (`remark-math` with
 * `singleDollarTextMath: false`, then `remarkDisplayMath`), so what is read is what the page draws. Pure, and
 * deterministic.
 */

const cases: [name: string, markdown: string, said: string][] = [
  ["a heading", "# The plan", "The plan"],
  ["a second-level heading", "## Steps\n\nDo the thing.", "Steps\nDo the thing."],
  ["bold and italic", "**bold** and _italic_ and ~~gone~~", "bold and italic and gone"],
  ["inline code", "run `cargo test` first", "run cargo test first"],
  ["markdown inside inline code is its content", "`**not bold**`", "**not bold**"],
  ["a fenced block", "```rust\nfn main() {}\n```", "fn main() {}"],
  ["a fence with no language", "```\nplain words\n```", "plain words"],
  ["a fence whose language the web does not know", "```nope\nnot coloured\n```", "not coloured"],
  ["a link is its label, never its URL", "[the docs](https://example.com/a?b=c)", "the docs"],
  ["a reference link is its label", "[the docs][d]\n\n[d]: https://example.com/", "the docs"],
  ["an image is its alt text", "![a diagram](https://example.com/x.png)", "a diagram"],
  ["an image with no alt text is the word image", "![](https://example.com/x.png)", "image"],
  ["a list is its items", "- one\n- two", "one\ntwo"],
  ["an ordered list is its items", "1. first\n2. second", "first\nsecond"],
  ["a blockquote is its words", "> quoted words", "quoted words"],
  ["a quote of a list is its items", "> - one\n> - two", "one\ntwo"],
  ["a table is read cell by cell", "| a | b |\n|---|---|\n| 1 | 2 |", "a, b\n1, 2"],
  ["raw HTML is read as it was written", "<script>alert(1)</script>", "<script>alert(1)</script>"],
  ["an inline tag is read as it was written", "a <b>bold</b> word", "a <b>bold</b> word"],
  ["display math is the word formula", "$$\nx^2\n$$", "formula"],
  ["a short formula on one line is the word formula", "$$x^2$$", "formula"],
  [
    "a formula between two paragraphs",
    "Before.\n\n$$\n\\pi\n$$\n\nAfter.",
    "Before.\nformula\nAfter.",
  ],
  ["a single dollar is a price", "It costs $5 or $10 a month.", "It costs $5 or $10 a month."],
  ["dollars beside text are text", "pay $5 $$ or $10 $$ tomorrow", "pay $5 $$ or $10 $$ tomorrow"],
  [
    "a $$ in a sentence is text",
    "In bash, $$ is the pid; print $$ twice.",
    "In bash, $$ is the pid; print $$ twice.",
  ],
  // the page draws both of these as text, so the voice says the text and never the TeX
  ["two inline formulas beside text are text", "$$x$$ and $$y$$", "$$x$$ and $$y$$"],
  [
    "an inline formula between words is text",
    "the value of $$x^2$$ here",
    "the value of $$x^2$$ here",
  ],
  // the page draws a formula and then a sentence: a formula is one block, and the next line is another
  [
    "a display formula followed by a sentence on the next line",
    "$$\nE = mc^2\n$$\nwhere E is energy.",
    "formula\nwhere E is energy.",
  ],
  // over the renderer's limit a formula is drawn as a code block, so it is read as its content
  [
    "a formula over the renderer's limit is its content",
    `$$\n${"x".repeat(MAX_FORMULA_CHARS + 1)}\n$$`,
    "x".repeat(MAX_FORMULA_CHARS + 1),
  ],
  // a surface is drawn, not said: `turnSource` puts one `[Interface: …]` line where it was
  [
    "a surface placeholder is not a word",
    "Before.\n\n[Interface: Choices]\n\nAfter.",
    "Before.\nAfter.",
  ],
  ["an answer that is only a surface says nothing", "[Interface: Cards, Mermaid]", ""],
  ["an answer that is only a rule says nothing", "---", ""],
  // a fence that is never closed is text, read as it was written
  ["a fence that is never closed is text", "$$\nx", "$$ x"],
  ["$$ inside a fence is code", "```\n$$\n```", "$$"],
  ["a line break in a paragraph is a space", "one\ntwo", "one two"],
  ["a horizontal rule says nothing", "one\n\n---\n\ntwo", "one\ntwo"],
  ["an empty answer says nothing", "   \n\n", ""],
  ["a definition alone says nothing", "[d]: https://example.com/", ""],
];

describe("speechText", () => {
  for (const [name, markdown, said] of cases) {
    it(`reads ${name}`, () => {
      expect(speechText(markdown)).toBe(said);
    });
  }

  it("is the same text every time, for the same answer", () => {
    const answer = "# Title\n\n**bold** and `code`\n\n- one\n- two\n\n$$\nx\n$$";
    expect(speechText(answer)).toBe(speechText(answer));
  });

  it("never leaves the marks of the Markdown in what is read", () => {
    const said = speechText(
      "## Steps\n\n**Run** `cargo test`, see [the docs](https://example.com/).\n\n```rust\nfn main() {}\n```",
    );
    expect(said).not.toMatch(/[#*`~]|\]\(|https?:\/\//);
  });
});

const words = (n: number, word = "word") => Array.from({ length: n }, () => word).join(" ");

describe("chunkSpeech", () => {
  it("says nothing of an empty text", () => {
    expect(chunkSpeech("")).toEqual([]);
    expect(chunkSpeech("   \n\n ")).toEqual([]);
  });

  it("keeps a short answer whole", () => {
    expect(chunkSpeech("One sentence. Another one.")).toEqual(["One sentence. Another one."]);
  });

  it("cuts a long answer at its sentences, under the bound", () => {
    const sentence = "This sentence is here to fill the space a real answer takes up.";
    const text = Array.from({ length: 12 }, () => sentence).join(" ");
    const chunks = chunkSpeech(text);
    expect(chunks.length).toBeGreaterThan(1);
    for (const chunk of chunks) expect(chunk.length).toBeLessThanOrEqual(SPEECH_CHUNK_CHARS);
    expect(chunks.join(" ")).toBe(text);
  });

  it("cuts a text with no sentence punctuation at its words, keeping every one", () => {
    const text = words(200);
    const chunks = chunkSpeech(text);
    expect(chunks.length).toBeGreaterThan(1);
    for (const chunk of chunks) expect(chunk.length).toBeLessThanOrEqual(SPEECH_CHUNK_CHARS);
    expect(chunks.join(" ")).toBe(text);
  });

  it("cuts a single word longer than the bound rather than saying nothing", () => {
    const chunks = chunkSpeech("x".repeat(SPEECH_CHUNK_CHARS + 40));
    expect(chunks.map((c) => c.length)).toEqual([SPEECH_CHUNK_CHARS, 40]);
  });

  it("reads a block of code as its lines, still in order", () => {
    const code = [
      "fn total(prices: &[u32]) -> u32 {",
      "    // sum in cents, so 5 dollars is 500",
      "    prices.iter().sum()",
      "}",
    ].join("\n");
    const chunks = chunkSpeech(`${code}\n\nThe rule is above.`);
    expect(chunks.join(" ")).toBe(
      "fn total(prices: &[u32]) -> u32 { // sum in cents, so 5 dollars is 500 prices.iter().sum() } The rule is above.",
    );
    for (const chunk of chunks) expect(chunk.length).toBeLessThanOrEqual(SPEECH_CHUNK_CHARS);
  });

  it("takes a bound of its own", () => {
    expect(chunkSpeech("One. Two. Three.", 8)).toEqual(["One.", "Two.", "Three."]);
  });
});
