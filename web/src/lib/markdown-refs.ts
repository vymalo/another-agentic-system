import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";

/*
 * The links and images of a Markdown text, read from the same tree the chat's renderer builds (GFM
 * included), so what is found here is what is drawn there: a URL inside a code span or a code block is
 * neither, and a reference (`[text][label]`, `![alt][label]`) is resolved through its definition. Pure.
 * One walker for everything that needs to know what an agent's words point at (the Sources tab's links,
 * the images that mean a shared file).
 */

export type MdNode = {
  type: string;
  url?: string;
  value?: string;
  alt?: string | null;
  identifier?: string;
  children?: MdNode[];
};

export type MdRef = {
  kind: "link" | "image";
  /** The URL written at the place; undefined for a reference, whose URL is its definition's. */
  url?: string;
  /** The label of a reference. */
  identifier?: string;
  node: MdNode;
};

export type MdRefs = {
  /** Every link and image, in the order they are written (an image inside a link is both). */
  refs: MdRef[];
  /** `[label]: url`, by label: the first definition of a label wins, as in the renderer. */
  definitions: ReadonlyMap<string, string>;
};

export function markdownRefs(markdown: string): MdRefs {
  const tree = fromMarkdown(markdown, {
    extensions: [gfm()],
    mdastExtensions: [gfmFromMarkdown()],
  }) as unknown as MdNode;
  const definitions = new Map<string, string>();
  const refs: MdRef[] = [];
  const walk = (node: MdNode) => {
    if (node.type === "definition" && node.identifier && node.url !== undefined) {
      if (!definitions.has(node.identifier)) definitions.set(node.identifier, node.url);
    }
    if (node.type === "link" && node.url !== undefined) {
      refs.push({ kind: "link", url: node.url, node });
    } else if (node.type === "linkReference" && node.identifier) {
      refs.push({ kind: "link", identifier: node.identifier, node });
    } else if (node.type === "image" && node.url !== undefined) {
      refs.push({ kind: "image", url: node.url, node });
    } else if (node.type === "imageReference" && node.identifier) {
      refs.push({ kind: "image", identifier: node.identifier, node });
    }
    for (const child of node.children ?? []) walk(child);
  };
  walk(tree);
  return { refs, definitions };
}

/** Where a link or an image points: what is written, or its definition's URL; undefined for a label with none. */
export function refUrl(ref: MdRef, definitions: ReadonlyMap<string, string>): string | undefined {
  return ref.url ?? (ref.identifier !== undefined ? definitions.get(ref.identifier) : undefined);
}

/** The plain text under a node: text, inline code and image descriptions. */
export function textOf(node: MdNode): string {
  const own =
    node.type === "text" || node.type === "inlineCode"
      ? (node.value ?? "")
      : node.type === "image"
        ? (node.alt ?? "")
        : "";
  return own + (node.children ?? []).map(textOf).join("");
}
