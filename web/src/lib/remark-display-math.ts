/*
 * What counts as math in an answer (web/README.md "Code and math in answers"). It runs after `remark-math`, which on its
 * own reads `$$` anywhere as the start of a formula ("In bash, $$ is the pid; print $$ twice." is one formula and
 * "pay $5 $$ or $10 $$ tomorrow" a broken one). Here:
 *
 * - **Display math is `$$` on a line of its own, opening and closing**, with the formula between the two lines (or the
 *   whole `$$...$$` as a paragraph by itself, which is what a model writes when the formula is short).
 * - `$$...$$` beside any other text is **text**, exactly as it was written.
 * - A `$$` fence that is never closed is text too: a streaming answer has not reached its end, and a formula that stops
 *   half way would flash an error.
 * - A formula over `MAX_FORMULA_CHARS` is a block of code, not given to KaTeX: KaTeX took 1.9 s and made 4 MB of DOM
 *   for 50 000 characters, and a streaming answer would do that again on every piece of text.
 *
 * `$$` in inline code and in a fence is code to the parser and never reaches this plugin.
 */

export const MAX_FORMULA_CHARS = 5000;

type Node = {
  type: string;
  value?: string;
  lang?: string | null;
  meta?: string | null;
  children?: Node[];
  position?: { start: { offset?: number }; end: { offset?: number } };
  data?: unknown;
};

const CLOSING_FENCE = /^[ \t>]*\$\$+[ \t]*$/;

/** The text of the source that a node was read from, or null when the parser gave no offsets. */
function sourceOf(node: Node, file: string | undefined): string | null {
  const start = node.position?.start.offset;
  const end = node.position?.end.offset;
  if (file === undefined || start === undefined || end === undefined) return null;
  return file.slice(start, end);
}

const text = (value: string): Node => ({ type: "text", value });

const code = (value: string): Node => ({ type: "code", lang: null, meta: null, value });

/** A `math` node as `mdast-util-math` makes it, so `rehype-katex` draws it as display math. */
const display = (value: string): Node => ({
  type: "math",
  meta: null,
  value,
  data: {
    hName: "pre",
    hChildren: [
      {
        type: "element",
        tagName: "code",
        properties: { className: ["language-math", "math-display"] },
        children: [{ type: "text", value }],
      },
    ],
  },
});

const blank = (node: Node) => node.type === "text" && (node.value ?? "").trim() === "";

/** The one formula that is all a paragraph holds, or null. */
function soleFormula(paragraph: Node): Node | null {
  const content = (paragraph.children ?? []).filter((child) => !blank(child));
  const [only] = content;
  return content.length === 1 && only?.type === "inlineMath" ? only : null;
}

function closed(node: Node, file: string | undefined): boolean {
  const source = sourceOf(node, file);
  if (source === null) return true;
  const lines = source.split(/\r\n|\r|\n/);
  const last = lines[lines.length - 1] ?? "";
  return lines.length >= 2 && CLOSING_FENCE.test(last);
}

function rewrite(parent: Node, file: string | undefined) {
  const children = parent.children;
  if (!children) return;
  for (let at = 0; at < children.length; at++) {
    const child = children[at];
    if (!child) continue;
    if (child.type === "math") {
      if (!closed(child, file)) {
        const source = sourceOf(child, file) ?? `$$\n${child.value ?? ""}`;
        children[at] = { type: "paragraph", children: [text(source)] };
      } else if ((child.value ?? "").length > MAX_FORMULA_CHARS) {
        children[at] = code(child.value ?? "");
      }
    } else if (child.type === "paragraph") {
      const formula = soleFormula(child);
      if (formula) {
        const value = formula.value ?? "";
        children[at] = value.length > MAX_FORMULA_CHARS ? code(value) : display(value);
      } else {
        rewrite(child, file);
      }
    } else if (child.type === "inlineMath") {
      children[at] = text(sourceOf(child, file) ?? `$$${child.value ?? ""}$$`);
    } else {
      rewrite(child, file);
    }
  }
}

export function remarkDisplayMath() {
  return (tree: Node, file: { value?: unknown }) => {
    rewrite(tree, typeof file.value === "string" ? file.value : undefined);
  };
}
