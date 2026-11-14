import type { Element, ElementContent, Root } from "hast";

/*
 * KaTeX draws a formula twice: as MathML (what a screen reader reads) and as aria-hidden HTML (what the eye reads).
 * Nothing names the formula as a whole, though. This plugin, which runs after `rehype-katex`, gives each formula
 * `role="math"` and, as its name, the TeX source the MathML carries in its `annotation`, and makes a display
 * formula, whose box scrolls sideways when it is wider than the column, reachable by keyboard.
 */

const MAX_LABEL = 500;

const classes = (node: Element): string[] => {
  const value = node.properties?.className;
  return Array.isArray(value) ? value.map(String) : [];
};

function annotation(node: ElementContent): string | null {
  if (node.type !== "element") return null;
  if (node.tagName === "annotation") {
    return node.children.map((c) => (c.type === "text" ? c.value : "")).join("");
  }
  for (const child of node.children) {
    const found = annotation(child);
    if (found !== null) return found;
  }
  return null;
}

function label(node: Element) {
  const tex = annotation(node)?.replace(/\s+/g, " ").trim();
  if (!tex) return;
  node.properties = {
    ...node.properties,
    role: "math",
    ariaLabel: tex.length > MAX_LABEL ? `${tex.slice(0, MAX_LABEL)}…` : tex,
  };
}

function walk(node: Root | Element) {
  for (const child of node.children) {
    if (child.type !== "element") continue;
    const names = classes(child);
    if (names.includes("katex-display")) {
      label(child);
      child.properties = { ...child.properties, tabIndex: 0 };
    } else if (names.includes("katex")) {
      label(child);
    } else {
      walk(child);
    }
  }
}

export function rehypeMathLabel() {
  return (tree: Root) => walk(tree);
}
