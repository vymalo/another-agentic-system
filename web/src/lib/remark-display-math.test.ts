import { describe, expect, it } from "vitest";
import { MAX_FORMULA_CHARS, remarkDisplayMath } from "./remark-display-math";

/*
 * The plugin runs after remark-math and reads its nodes: `math` (a `$$` fence of its own lines) and `inlineMath`
 * (`$$...$$` inside a paragraph). The trees here are the ones remark-math builds (mdast-util-math 3.0.0), written
 * by hand; `markdown-text.dom.test.tsx` runs the real parser through the real renderer.
 */

type Node = {
  type: string;
  value?: string;
  children?: Node[];
  position?: { start: { offset: number }; end: { offset: number } };
  data?: { hName?: string; hProperties?: { className?: string[] }; hChildren?: Node[] };
};

const text = (value: string): Node => ({ type: "text", value });

function inline(source: string, at: number): Node {
  const value = source.slice(2, -2).trim();
  return {
    type: "inlineMath",
    value,
    position: { start: { offset: at }, end: { offset: at + source.length } },
    data: { hName: "code", hProperties: { className: ["language-math", "math-inline"] } },
  };
}

function flow(value: string, source: string, at = 0): Node {
  return {
    type: "math",
    value,
    position: { start: { offset: at }, end: { offset: at + source.length } },
    data: {
      hName: "pre",
      hChildren: [
        {
          type: "element",
          tagName: "code",
          properties: { className: ["language-math", "math-display"] },
          children: [{ type: "text", value }],
        } as unknown as Node,
      ],
    },
  };
}

const run = (children: Node[], file: string) => {
  const tree: Node = { type: "root", children };
  remarkDisplayMath()(tree, { value: file });
  return tree.children ?? [];
};

describe("remarkDisplayMath", () => {
  it("keeps a fenced block that is closed", () => {
    const file = "$$\nx^2\n$$";
    const [node] = run([flow("x^2", file)], file);
    expect(node?.type).toBe("math");
  });

  it("turns a fence that is never closed into text (a streaming answer has not reached the end of it)", () => {
    const file = "$$\n\\frac{a}{";
    const [node] = run([flow("\\frac{a}{", file)], file);
    expect(node).toMatchObject({ type: "paragraph" });
    expect(node?.children?.[0]).toMatchObject({ type: "text", value: file });
  });

  it("keeps a closed fence inside a quote or a list item", () => {
    const file = "> $$\n> x\n> $$";
    expect(run([flow("x", file)], file)[0]?.type).toBe("math");
  });

  it("turns `$$` inside a sentence back into the words that were written", () => {
    const file = "In bash, $$ is the pid; print $$ twice.";
    const at = file.indexOf("$$");
    const end = file.lastIndexOf("$$") + 2;
    const found = inline(file.slice(at, end), at);
    const [paragraph] = run(
      [
        {
          type: "paragraph",
          children: [text("In bash, "), found, text(" twice.")],
        },
      ],
      file,
    );
    expect(paragraph?.type).toBe("paragraph");
    const kinds = paragraph?.children?.map((c) => c.type);
    expect(kinds).toEqual(["text", "text", "text"]);
    expect(paragraph?.children?.map((c) => c.value).join("")).toBe(
      "In bash, $$ is the pid; print $$ twice.",
    );
  });

  it("keeps `$$...$$` that is a paragraph by itself, as display math", () => {
    const file = "$$E = mc^2$$";
    const [node] = run([{ type: "paragraph", children: [inline(file, 0)] }], file);
    expect(node?.type).toBe("math");
    expect(node?.value).toBe("E = mc^2");
    expect(node?.data?.hName).toBe("pre");
    expect(node?.data?.hChildren?.[0]).toMatchObject({
      properties: { className: ["language-math", "math-display"] },
    });
  });

  it("reads through the spaces around a formula that is a paragraph by itself", () => {
    const file = "$$ x $$ ";
    const [node] = run([{ type: "paragraph", children: [inline("$$ x $$", 0), text(" ")] }], file);
    expect(node?.type).toBe("math");
  });

  it("does not make display math of a formula beside words", () => {
    const file = "so $$x^2$$ here";
    const [paragraph] = run(
      [{ type: "paragraph", children: [text("so "), inline("$$x^2$$", 3), text(" here")] }],
      file,
    );
    expect(paragraph?.type).toBe("paragraph");
    expect(paragraph?.children?.map((c) => c.type)).toEqual(["text", "text", "text"]);
  });

  it("reaches formulas inside emphasis, links and lists", () => {
    const file = "a *b $$c$$ d*";
    const found = inline("$$c$$", 5);
    const [paragraph] = run(
      [
        {
          type: "paragraph",
          children: [text("a "), { type: "emphasis", children: [text("b "), found, text(" d")] }],
        },
      ],
      file,
    );
    const emphasis = paragraph?.children?.[1];
    expect(emphasis?.children?.map((c) => c.type)).toEqual(["text", "text", "text"]);
    expect(emphasis?.children?.[1]?.value).toBe("$$c$$");
  });

  it("shows a fenced formula over the limit as a block of code, never as math", () => {
    const big = "x".repeat(MAX_FORMULA_CHARS + 1);
    const file = `$$\n${big}\n$$`;
    const [node] = run([flow(big, file)], file);
    expect(node).toMatchObject({ type: "code", value: big });
    expect(node?.data).toBeUndefined();
  });

  it("shows a one-line formula over the limit as a block of code too", () => {
    const big = "y".repeat(MAX_FORMULA_CHARS + 1);
    const file = `$$${big}$$`;
    const [node] = run([{ type: "paragraph", children: [inline(file, 0)] }], file);
    expect(node).toMatchObject({ type: "code", value: big });
  });

  it("keeps a formula at the limit", () => {
    const ok = "z".repeat(MAX_FORMULA_CHARS);
    const file = `$$\n${ok}\n$$`;
    expect(run([flow(ok, file)], file)[0]?.type).toBe("math");
  });
});
