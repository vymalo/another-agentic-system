import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { mathFromMarkdown } from "mdast-util-math";
import { gfm } from "micromark-extension-gfm";
import { math } from "micromark-extension-math";
import { remarkDisplayMath } from "@/lib/remark-display-math";

/*
 * The words Read aloud says of an answer: the page as it reads, not the Markdown it was written in. A person who presses
 * "Read aloud" hears the words they see — a heading is its words, `**bold**` is "bold", a fenced block is its code and
 * never its fence or its language, a link is its label and never its URL, an image is its alt text (or "image"), a table
 * is read cell by cell, and a formula is the word "formula" — because TeX read out letter by letter is noise.
 *
 * **The tree is the renderer's own**, not a second reading of Markdown: `remark-math` with `singleDollarTextMath: false`
 * (`$5 or $10` is a price) and then `remarkDisplayMath`, called with the source so its offset-based rules work
 * (`src/lib/remark-display-math.ts`). What the page draws is therefore what is read: `$$x$$ and $$y$$` beside other
 * words is text and is read as text, a formula on lines of its own is "formula", a formula over the renderer's limit is a
 * code block and is read as its content, and a `$$` fence a streaming answer has not closed yet is text.
 *
 * **Raw HTML is read as it was written** (`<script>alert(1)</script>` is those characters): the contract shows it as text
 * and never runs it, and stripping it would make a hostile answer read as nothing.
 *
 * **A `[Interface: …]` line is a placeholder, not a word**: `turnSource` puts one where a surface was drawn, for Copy and
 * Quote. The page draws the surface, and the voice has nothing to say about it.
 *
 * Pure: the same answer is the same words every time, which is what its tests hold.
 */

type Node = {
  type: string;
  value?: string;
  alt?: string | null;
  children?: Node[];
  position?: { start: { offset?: number }; end: { offset?: number } };
};

/** Runs of whitespace are one space to a voice: a line break in a paragraph is not a pause a person can hear. */
const squash = (text: string): string => text.replace(/\s+/g, " ").trim();

/** One `[Interface: …]` line, as `turnSource` writes it (`features/chat/lib/turn-source.ts`). */
const PLACEHOLDER = /^\[Interface: [^\]\n]*\]$/;

/**
 * Any inline math the renderer left in place is the text the page shows it as, `$$…$$` and all, never its TeX.
 * `remarkDisplayMath` already makes this rewrite; it is made here too, so the reading does not depend on the order the
 * two run in.
 */
function literalInlineMath(node: Node, source: string): void {
  for (const [at, child] of (node.children ?? []).entries()) {
    if (child.type === "inlineMath") {
      const start = child.position?.start.offset;
      const end = child.position?.end.offset;
      const shown =
        start === undefined || end === undefined
          ? `$$${child.value ?? ""}$$`
          : source.slice(start, end);
      const children = node.children as Node[];
      children[at] = { type: "text", value: shown };
    } else {
      literalInlineMath(child, source);
    }
  }
}

/** What an inline node says: its words, the code itself, the label of a link, the alt text of an image. */
function inline(node: Node): string {
  switch (node.type) {
    case "text":
    case "inlineCode":
    case "html":
      return node.value ?? "";
    case "image":
    case "imageReference":
      return node.alt?.trim() ? node.alt : "image";
    case "break":
      return " ";
    case "emphasis":
    case "strong":
    case "delete":
    case "link":
    case "linkReference":
      return (node.children ?? []).map(inline).join("");
    default:
      return (node.children ?? []).map(inline).join("");
  }
}

/** What a block node says, as its own line (or lines, for code and a table). */
function block(node: Node): string {
  switch (node.type) {
    case "paragraph": {
      const said = squash((node.children ?? []).map(inline).join(""));
      // a surface's placeholder is where it was drawn, not words the page says
      return PLACEHOLDER.test(said) ? "" : said;
    }
    case "heading":
      return squash((node.children ?? []).map(inline).join(""));
    case "code":
      // the code itself: no fence, no language tag (and a formula over the limit is drawn as one)
      return (node.value ?? "").replace(/^\n+|\n+$/g, "");
    case "math":
      return "formula";
    case "html":
      return squash(node.value ?? "");
    case "table":
      return (node.children ?? [])
        .map((row) =>
          (row.children ?? [])
            .map((cell) => squash((cell.children ?? []).map(inline).join("")))
            .join(", "),
        )
        .join("\n");
    case "list":
      return (node.children ?? []).map((item) => blocks(item.children ?? []).join("\n")).join("\n");
    case "blockquote":
      return blocks(node.children ?? []).join("\n");
    // a definition is a URL's label, and a thematic break is a line: neither is a word to say
    case "definition":
    case "thematicBreak":
      return "";
    default:
      return blocks(node.children ?? []).join("\n");
  }
}

/** Every block of a tree, one line each; a block that says nothing is left out. */
function blocks(nodes: readonly Node[]): string[] {
  return nodes.flatMap((node) => {
    const said = block(node);
    return said === "" ? [] : said.split("\n").filter((line) => line.trim() !== "");
  });
}

/**
 * What Read aloud says of an answer's Markdown: one line per block, sentences as they were written, no Markdown marks
 * and no URL. Empty when the answer says nothing a voice can read (a surface alone, a rule alone).
 */
export function speechText(markdown: string): string {
  const tree = fromMarkdown(markdown, {
    // the renderer's own math, so a formula the page draws is a formula here and text stays text
    extensions: [gfm(), math({ singleDollarTextMath: false })],
    mdastExtensions: [gfmFromMarkdown(), mathFromMarkdown()],
  }) as unknown as Node;
  // the renderer's own rule for what counts as display math, with the source it needs for its offsets
  remarkDisplayMath()(tree, { value: markdown });
  literalInlineMath(tree, markdown);
  return blocks(tree.children ?? []).join("\n");
}

/** How much of an answer one utterance holds. A few hundred characters is a sentence or two: short enough to start
 * speaking at once, long enough not to pause between every sentence. */
export const SPEECH_CHUNK_CHARS = 240;

/** Breaks a piece of text into lines of at most `max` characters, at its spaces (a word longer than that is cut). */
function wrap(piece: string, max: number): string[] {
  const out: string[] = [];
  let rest = piece;
  while (rest.length > max) {
    const cut = rest.lastIndexOf(" ", max);
    if (cut <= 0) {
      out.push(rest.slice(0, max));
      rest = rest.slice(max).trimStart();
    } else {
      out.push(rest.slice(0, cut));
      rest = rest.slice(cut + 1);
    }
  }
  if (rest !== "") out.push(rest);
  return out;
}

/**
 * An answer cut into the utterances that are spoken one after another, in order: at its sentence ends and its line
 * breaks, then at its words, so a long answer is never cut off (a browser says an utterance and stops at its end; an
 * answer of 20,000 characters is not one utterance). Stop cancels the whole queue.
 */
export function chunkSpeech(text: string, max = SPEECH_CHUNK_CHARS): string[] {
  const body = text.replace(/\r\n?/g, "\n").trim();
  if (body === "") return [];
  const pieces = body
    .split(/(?<=[.!?…])\s+|\n+/)
    .flatMap((piece) => wrap(piece.trim(), max))
    .filter((piece) => piece !== "");
  const chunks: string[] = [];
  let current = "";
  for (const piece of pieces) {
    if (current === "") {
      current = piece;
    } else if (current.length + 1 + piece.length <= max) {
      current = `${current} ${piece}`;
    } else {
      chunks.push(current);
      current = piece;
    }
  }
  if (current !== "") chunks.push(current);
  return chunks;
}
