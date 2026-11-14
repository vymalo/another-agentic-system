import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";

/*
 * Quote into the composer (docs/api/thread-markdown-v1.md, "Client recipe for Copy and Quote"): the words of a message, or of a
 * selection inside one, as a Markdown block quote the person can edit before sending. Nothing links the quote to the message it came
 * from: it is the person's own text, and the API never hears of it.
 *
 * Pure, apart from `pickSelection`, which reads the page's selection.
 */

const LF = (text: string): string => text.replace(/\r\n?/g, "\n");

/** The most a quote holds, in UTF-16 units: the composer has no limit of its own, so Quote keeps one (the page says so). */
export const MAX_QUOTE = 20_000;

/** What Quote says when it cut a quote. */
export const QUOTE_CUT = "The quote was cut at 20,000 characters.";

/** What Quote says when a selection spans messages. */
export const QUOTE_ACROSS = "Select text inside one message to quote it.";

/**
 * The text a quote holds, at most `max` characters: a longer one is cut at the last whole line that fits (the first line is cut
 * itself when it alone is longer than `max`).
 */
export function limitQuote(text: string, max = MAX_QUOTE): { text: string; cut: boolean } {
  const body = LF(text);
  if (body.length <= max) return { text: body, cut: false };
  const kept: string[] = [];
  let size = 0;
  for (const line of body.split("\n")) {
    if (size + line.length + (kept.length > 0 ? 1 : 0) > max) break;
    size += line.length + (kept.length > 0 ? 1 : 0);
    kept.push(line);
  }
  return { text: kept.length > 0 ? kept.join("\n") : body.slice(0, max), cut: true };
}

/** `> ` before every line (a bare `>` on a blank one), no blank line around the block. */
export function quoteBlock(text: string): string {
  const lines = LF(text)
    .replace(/^(?:[ \t]*\n)+/, "")
    .replace(/\s+$/, "")
    .split("\n");
  return lines.map((line) => (line === "" ? ">" : `> ${line}`)).join("\n");
}

/** A quote of code: a fenced block inside the block quote, its fence longer than any run of backticks in the code. */
export function quoteCode(code: string, language?: string): string {
  const body = LF(code).replace(/\n+$/, "");
  const longest = Math.max(0, ...(body.match(/`+/g) ?? []).map((run) => run.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  const tag = language && /^[A-Za-z0-9_+#.-]{1,40}$/.test(language) ? language : "";
  return quoteBlock(`${fence}${tag}\n${body}\n${fence}`);
}

const isSpace = (ch: string | undefined): boolean => ch === " " || ch === "\t" || ch === "\n";

/**
 * The box's text with `block` put in at `caret` (a text index; `null` appends), as a paragraph of its own: a blank line before it
 * unless it is first, and one after it. A caret inside a word moves to the end of that word, so a word is never split. A quote
 * already in the box stays. Returns the new text and where the caret goes, right after the block.
 */
export function insertQuote(
  current: string,
  caret: number | null,
  block: string,
): { text: string; caret: number } {
  let at = caret === null ? current.length : Math.min(Math.max(caret, 0), current.length);
  while (at > 0 && at < current.length && !isSpace(current[at - 1]) && !isSpace(current[at])) at++;
  const before = current.slice(0, at);
  const after = current.slice(at);
  const lead =
    before === "" || before.endsWith("\n\n") ? "" : before.endsWith("\n") ? "\n" : "\n\n";
  const put = `${lead}${block}\n\n`;
  return { text: `${before}${put}${after}`, caret: before.length + put.length };
}

/** The element a message is drawn in: the person's bubble, an agent's turn, and a turn that only holds answers. */
export const MESSAGE_ROOT = '[data-slot="user-message"], [data-slot="agent-turn"]';

/** The header of a code block (its language label and Copy button): never part of what a selection quotes. */
export const CODE_HEADER = ".aui-code-header-root";

/** What the person has selected, as far as quoting a message goes. */
export type Picked =
  | { kind: "none" }
  | { kind: "across" }
  | { kind: "text"; text: string }
  | { kind: "code"; code: string; language?: string }
  /** The selection touches drawn math: `source` is the source of the blocks that cover it, or null when that cannot be told. */
  | { kind: "math"; source: string | null };

/** The element a node is in (a text node's parent). */
const elementOf = (node: Node | null): Element | null =>
  node === null ? null : node.nodeType === 1 ? (node as Element) : node.parentElement;

/** The start and end offsets of the top-level blocks of a source, read as CommonMark with GFM; nothing for an empty source. */
export function sourceBlocks(source: string): { start: number; end: number }[] {
  const tree = fromMarkdown(source, { extensions: [gfm()], mdastExtensions: [gfmFromMarkdown()] });
  return tree.children.flatMap((node) => {
    const start = node.position?.start.offset;
    const end = node.position?.end.offset;
    return start === undefined || end === undefined ? [] : [{ start, end }];
  });
}

/**
 * The source of the smallest run of whole top-level blocks that covers `range`, or null when that cannot be told: the rendered
 * blocks of the message (the children of its `.aui-md` boxes, a code block's header not counted) must be as many as the source's,
 * one for one, or the drawn page and the source do not line up (an interface line, raw HTML, a list that was cut).
 */
export function sourceOfBlocks(root: Element, range: Range, source: string): string | null {
  const rendered = [...root.querySelectorAll(".aui-md")].flatMap((box) =>
    [...box.children].filter((el) => !el.matches(CODE_HEADER)),
  );
  const blocks = sourceBlocks(source);
  if (blocks.length === 0 || blocks.length !== rendered.length) return null;
  const hit = rendered.flatMap((el, i) => (range.intersectsNode(el) ? [i] : []));
  const first = hit[0];
  const last = hit[hit.length - 1];
  if (first === undefined || last === undefined) return null;
  return source.slice(blocks[first]?.start, blocks[last]?.end);
}

/**
 * What the selection says about message `root` (the one whose Quote was pressed), `source` being that message's source text:
 * - nothing selected, or a selection that touches no message (the composer, the page's chrome), or one inside another single
 *   message: `none`, and the whole message is quoted;
 * - a selection that touches two or more messages: `across`, and nothing is quoted (which part is meant is not clear);
 * - a selection inside `root` that touches drawn math: `math`, quoted from the source and not the drawn text, which holds a
 *   formula's hidden MathML as well and would write it twice;
 * - any other selection inside `root`: its text exactly as the page shows it, no markup, without the label and the Copy button of a
 *   code block's header; and `code` when it lies inside one code block.
 */
export function pickSelection(root: Element, selection: Selection | null, source: string): Picked {
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return { kind: "none" };
  const range = selection.getRangeAt(0);
  const touched = [...document.querySelectorAll(MESSAGE_ROOT)].filter((m) =>
    range.intersectsNode(m),
  );
  if (touched.length === 0) return { kind: "none" };
  if (touched.length > 1) return { kind: "across" };
  if (touched[0] !== root) return { kind: "none" };
  if ([...root.querySelectorAll(".katex")].some((k) => range.intersectsNode(k))) {
    return { kind: "math", source: sourceOfBlocks(root, range, source) };
  }
  // a selection that starts or ends in a code block's header leaves the header out
  const clean = range.cloneRange();
  for (const header of root.querySelectorAll(CODE_HEADER)) {
    const pre = header.nextElementSibling;
    if (header.contains(clean.startContainer) && pre) clean.setStart(pre, 0);
    else if (header.contains(clean.endContainer)) clean.setEndBefore(header);
  }
  if (clean.collapsed) return { kind: "none" };
  selection.removeAllRanges();
  selection.addRange(clean);
  const text = selection.toString();
  selection.removeAllRanges();
  selection.addRange(range);
  if (text.trim() === "") return { kind: "none" };
  const pre = elementOf(clean.commonAncestorContainer)?.closest("pre");
  if (pre && root.contains(pre)) {
    const label = pre.previousElementSibling?.querySelector(".aui-code-header-language");
    const language = label?.textContent?.trim();
    return { kind: "code", code: text, ...(language ? { language } : {}) };
  }
  return { kind: "text", text };
}
