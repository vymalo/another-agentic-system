/*
 * Quote into the composer (docs/api/message-text-v1.md, "Client recipe"): the words of a message, or of a selection inside one,
 * as a Markdown block quote the person can edit before sending. Nothing links the quote to the message it came from: it is the
 * person's own text, and the API never hears of it.
 *
 * Pure, apart from `pickSelection`, which reads the page's selection.
 */

const LF = (text: string): string => text.replace(/\r\n?/g, "\n");

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

/**
 * The box's text with `block` put in at `caret` (a text index; `null` appends), as a paragraph of its own: a blank line before it
 * unless it is first, and one after it. Returns the new text and where the caret goes, right after the block.
 */
export function insertQuote(
  current: string,
  caret: number | null,
  block: string,
): { text: string; caret: number } {
  const at = caret === null ? current.length : Math.min(Math.max(caret, 0), current.length);
  const before = current.slice(0, at);
  const after = current.slice(at);
  const lead =
    before === "" || before.endsWith("\n\n") ? "" : before.endsWith("\n") ? "\n" : "\n\n";
  const put = `${lead}${block}\n\n`;
  return { text: `${before}${put}${after}`, caret: before.length + put.length };
}

/** The element a message is drawn in: the person's bubble, an agent's turn, and a turn that only holds answers. */
export const MESSAGE_ROOT = '[data-slot="user-message"], [data-slot="agent-turn"]';

/** What the person has selected, as far as quoting a message goes. */
export type Picked =
  | { kind: "none" }
  | { kind: "across" }
  | { kind: "text"; text: string }
  | { kind: "code"; code: string; language?: string };

/** The element a node is in (a text node's parent). */
const elementOf = (node: Node | null): Element | null =>
  node === null ? null : node.nodeType === 1 ? (node as Element) : node.parentElement;

/**
 * What the selection says about message `root` (the one whose Quote was pressed):
 * - nothing selected, or a selection that touches no message (the composer, the page's chrome), or one inside another single
 *   message: `none`, and the whole message is quoted;
 * - a selection that touches two or more messages: `across`, and nothing is quoted (which part is meant is not clear);
 * - a selection inside `root`: its text exactly as the page shows it, no markup; and `code` when it lies inside one code block.
 */
export function pickSelection(root: Element, selection: Selection | null): Picked {
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return { kind: "none" };
  const range = selection.getRangeAt(0);
  const touched = [...document.querySelectorAll(MESSAGE_ROOT)].filter((m) =>
    range.intersectsNode(m),
  );
  if (touched.length === 0) return { kind: "none" };
  if (touched.length > 1) return { kind: "across" };
  if (touched[0] !== root) return { kind: "none" };
  const text = selection.toString();
  if (text.trim() === "") return { kind: "none" };
  const pre = elementOf(range.commonAncestorContainer)?.closest("pre");
  if (pre && root.contains(pre)) {
    const label = pre.previousElementSibling?.querySelector(".aui-code-header-language");
    const language = label?.textContent?.trim();
    return { kind: "code", code: text, ...(language ? { language } : {}) };
  }
  return { kind: "text", text };
}

/** What Quote says when a selection spans messages. */
export const QUOTE_ACROSS = "Select text inside one message to quote it.";
